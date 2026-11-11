use super::{FieldSchema, Kind, Length, Primitive, Ref, Slot};
use crate::{
    schema::View,
    wire::{self, Message, Presence, Span},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueKind {
    Required,
    Null,
    Cardinality,
    Length,
    Encoding,
    Unmodeled,
    Profile,
    Selector { target: usize },
    UnknownDatatype { target: usize },
    Wire(wire::ErrorKind),
    Lexical(wire::ErrorKind),
    Limit,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub kind: IssueKind,
    pub span: Span,
    pub path: [usize; 5],
    pub presence: Presence,
}
impl Issue {
    pub(super) fn at(message: &Message, path: &[usize], kind: IssueKind) -> Self {
        let value = message.value(path);
        let mut coordinates = [0; 5];
        coordinates[..path.len()].copy_from_slice(path);
        let segment = path.first().and_then(|n| message.segment(*n));
        let span = value.span().unwrap_or_else(|| {
            if path.len() == 1 {
                return segment.map_or(0..0, |s| s.span());
            }
            for length in (2..path.len()).rev() {
                if let Some(parent) = message.value(&path[..length]).span() {
                    return parent.end..parent.end;
                }
            }
            segment.map_or(0..0, |s| {
                let end = s.span().end;
                end..end
            })
        });
        let presence = if path.len() == 1 && segment.is_some() {
            Presence::Value
        } else {
            value.presence()
        };
        Self {
            kind,
            span,
            path: coordinates,
            presence,
        }
    }
    pub(super) fn wire(
        message: &Message,
        path: &[usize],
        error: wire::Error,
        lexical: bool,
    ) -> Self {
        let mut issue = Self::at(
            message,
            path,
            if lexical {
                IssueKind::Lexical(error.kind)
            } else {
                IssueKind::Wire(error.kind)
            },
        );
        if !lexical || error.kind == wire::ErrorKind::Timestamp {
            issue.span = error.span;
        }
        issue
    }
}
#[derive(Clone, Copy)]
pub struct ValidationLimits {
    pub max_steps: usize,
    pub max_bytes: usize,
    pub max_issues: usize,
}
impl Default for ValidationLimits {
    fn default() -> Self {
        Self {
            max_steps: 1_000_000,
            max_bytes: 32 * 1024 * 1024,
            max_issues: 1024,
        }
    }
}
#[derive(Debug)]
pub struct Report {
    pub issues: Vec<Issue>,
    pub omitted: usize,
    pub complete: bool,
    pub steps: usize,
    pub bytes: usize,
}
impl Report {
    pub fn is_valid(&self) -> bool {
        self.complete && self.issues.is_empty() && self.omitted == 0
    }
}
struct Stop;
type RunResult = Result<(), Stop>;
struct Runner<'s, 'm> {
    schema: &'s FieldSchema<'s>,
    message: &'m Message,
    limits: ValidationLimits,
    report: Report,
}
impl FieldSchema<'_> {
    /// Validate every segment leaf using its explicit grammar profile. Structural matching is separate.
    pub fn validate(&self, view: &View<'_, '_>, limits: ValidationLimits) -> Report {
        let mut runner = Runner::new(self, view.message(), limits);
        for occurrence in view.segments() {
            if let Some(segment) = occurrence.segment() {
                if let Some(profile) = occurrence.profile() {
                    if runner.segment(profile, segment.occurrence()).is_err() {
                        break;
                    }
                } else {
                    let path = [segment.occurrence()];
                    if runner.enter(&path, 0).is_err() {
                        break;
                    }
                    runner.emit(&path, IssueKind::Unmodeled);
                }
            }
        }
        runner.report
    }
    /// Validate a named profile against one global source segment occurrence.
    pub fn validate_segment(
        &self,
        message: &Message,
        profile: &str,
        occurrence: usize,
        limits: ValidationLimits,
    ) -> Report {
        let mut runner = Runner::new(self, message, limits);
        let _ = runner.segment(profile, occurrence);
        runner.report
    }
}
impl<'s, 'm> Runner<'s, 'm> {
    fn new(schema: &'s FieldSchema<'s>, message: &'m Message, limits: ValidationLimits) -> Self {
        Self {
            schema,
            message,
            limits,
            report: Report {
                issues: Vec::new(),
                omitted: 0,
                complete: true,
                steps: 0,
                bytes: 0,
            },
        }
    }
    fn push(&mut self, issue: Issue) {
        if self.report.issues.len() < self.limits.max_issues {
            self.report.issues.push(issue);
        } else {
            self.report.omitted = self.report.omitted.saturating_add(1);
        }
    }
    fn emit(&mut self, path: &[usize], kind: IssueKind) {
        self.push(Issue::at(self.message, path, kind));
    }
    fn enter(&mut self, path: &[usize], bytes: usize) -> RunResult {
        if self.report.steps >= self.limits.max_steps
            || bytes > self.limits.max_bytes.saturating_sub(self.report.bytes)
        {
            self.report.complete = false;
            self.emit(path, IssueKind::Limit);
            return Err(Stop);
        }
        self.report.steps += 1;
        self.report.bytes += bytes;
        Ok(())
    }
    fn segment(&mut self, profile: &str, number: usize) -> RunResult {
        self.enter(&[number], 0)?;
        let Some(definition) = self.schema.profile(profile).filter(|d| {
            self.message
                .segment(number)
                .is_some_and(|s| s.id() == &d.id)
        }) else {
            self.emit(&[number], IssueKind::Profile);
            return Ok(());
        };
        let Some(segment) = self.message.segment(number) else {
            return Ok(());
        };
        for position in 1..=segment.field_count() {
            let path = [number, position];
            self.enter(&path, 0)?;
            if definition
                .fields
                .binary_search_by_key(&position, |s| s.position)
                .is_err()
                && populated(segment.field(position).presence())
            {
                self.emit(&path, IssueKind::Unmodeled);
            }
        }
        for slot in &definition.fields {
            let path = [number, slot.position];
            let value = self.message.value(&path);
            if matches!(value.presence(), Presence::Missing | Presence::Empty) {
                self.enter(&path, 0)?;
                if slot.occurs.min > 0 {
                    self.emit(&path, IssueKind::Required);
                }
                continue;
            }
            let opaque = definition.id == *b"MSH" && slot.position <= 2;
            let count = if opaque { 1 } else { value.child_count() };
            self.enter(&path, 0)?;
            if count < slot.occurs.min || slot.occurs.max.is_some_and(|max| count > max) {
                self.emit(&path, IssueKind::Cardinality);
            }
            if slot.occurs.max == Some(0) {
                continue;
            }
            let mut datatype = None;
            let mut resolved = false;
            for repetition in 1..=count {
                let coordinates = [number, slot.position, repetition];
                let path = &coordinates[..if opaque { 2 } else { 3 }];
                self.enter(path, 0)?;
                if !resolved && self.message.value(path).presence() == Presence::Value {
                    if let Ref::ByField { selector, .. } = slot.datatype {
                        let selector_path = [number, selector];
                        self.enter(
                            &selector_path,
                            self.message
                                .value(&selector_path)
                                .raw()
                                .map_or(0, <[u8]>::len),
                        )?;
                    }
                    datatype = match self.schema.resolve(self.message, number, slot) {
                        Ok(i) => Some(i),
                        Err(issue) => {
                            self.push(issue);
                            None
                        }
                    };
                    resolved = true;
                }
                self.value(path, slot, datatype)?;
            }
        }
        Ok(())
    }
    fn value(&mut self, path: &[usize], slot: &Slot, datatype: Option<usize>) -> RunResult {
        let value = self.message.value(path);
        self.enter(path, value.raw().map_or(0, <[u8]>::len))?;
        match value.presence() {
            Presence::Missing | Presence::Empty => {
                if slot.occurs.min > 0 {
                    self.emit(path, IssueKind::Required);
                }
                return Ok(());
            }
            Presence::Null => {
                if slot.occurs.max == Some(0) {
                    self.emit(path, IssueKind::Cardinality);
                } else if !slot.nullable {
                    self.emit(path, IssueKind::Null);
                }
                return Ok(());
            }
            Presence::Value => {}
        }
        if slot.occurs.max == Some(0) {
            self.emit(path, IssueKind::Cardinality);
            return Ok(());
        }
        if let Some(Length::WireBytes(max)) = slot.length
            && value.raw().is_some_and(|b| b.len() > max)
        {
            self.emit(path, IssueKind::Length);
        }
        let Some(datatype) = datatype else {
            return Ok(());
        };
        match &self.schema.types[datatype].kind {
            Kind::Primitive(primitive) => {
                let bytes = match value.decode() {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        self.push(Issue::wire(self.message, path, e, false));
                        return Ok(());
                    }
                };
                match slot.length {
                    Some(Length::ScalarBytes(max)) if bytes.len() > max => {
                        self.emit(path, IssueKind::Length)
                    }
                    Some(Length::Utf8Characters(max)) => match std::str::from_utf8(&bytes) {
                        Ok(text) if text.chars().count() > max => {
                            self.emit(path, IssueKind::Length)
                        }
                        Err(_) => {
                            self.emit(path, IssueKind::Encoding);
                            return Ok(());
                        }
                        _ => {}
                    },
                    _ => {}
                }
                let error = match primitive {
                    Primitive::Bytes => None,
                    Primitive::Ascii if !bytes.is_ascii() => {
                        self.emit(path, IssueKind::Encoding);
                        None
                    }
                    Primitive::Utf8 if std::str::from_utf8(&bytes).is_err() => {
                        self.emit(path, IssueKind::Encoding);
                        None
                    }
                    Primitive::Ascii | Primitive::Utf8 => None,
                    Primitive::Number => wire::Decimal::parse(&bytes).err(),
                    Primitive::Date => wire::Date::parse(&bytes).err(),
                    Primitive::DateTime => wire::DateTime::parse(&bytes).err(),
                };
                if let Some(error) = error {
                    self.push(Issue::wire(self.message, path, error, true));
                }
            }
            Kind::Timestamp => {
                if let Err(error) = self.message.timestamp(path) {
                    let span = error.span.clone();
                    let mut issue = Issue::wire(self.message, path, error, true);
                    issue.span = span; // Timestamp errors already refer to original wire bytes.
                    self.push(issue);
                }
            }
            Kind::Composite(slots) => {
                if path.len() >= 5 {
                    self.emit(path, IssueKind::Profile);
                    return Ok(());
                }
                let mut coordinates = [0; 5];
                coordinates[..path.len()].copy_from_slice(path);
                for position in 1..=value.child_count() {
                    coordinates[path.len()] = position;
                    let child = &coordinates[..path.len() + 1];
                    self.enter(child, 0)?;
                    if slots
                        .binary_search_by_key(&position, |s| s.position)
                        .is_err()
                        && populated(value.child(position).presence())
                    {
                        self.emit(child, IssueKind::Unmodeled);
                    }
                }
                for slot in slots {
                    coordinates[path.len()] = slot.position;
                    let child = &coordinates[..path.len() + 1];
                    let datatype = match slot.datatype {
                        Ref::Fixed(i) => Some(i),
                        _ => None,
                    };
                    self.value(child, slot, datatype)?;
                }
            }
        }
        Ok(())
    }
}
fn populated(presence: Presence) -> bool {
    matches!(presence, Presence::Value | Presence::Null)
}
