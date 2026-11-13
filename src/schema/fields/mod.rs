#![doc = include_str!("../../../docs/validation.md")]
mod conditions;
mod run;
use conditions::CompiledCondition;
pub use conditions::{Condition, Predicate, Requirement};
use std::borrow::Cow;

use super::{Catalog, CompileLimits, Error, ErrorKind, Occurs, identifier};
use crate::wire::{Message, Presence};
pub use run::{Issue, IssueKind, Report, ValidationLimits};

#[derive(Clone, Copy)]
pub enum Primitive {
    Bytes,
    Ascii,
    Utf8,
    Number,
    Date,
    DateTime,
}
#[derive(Clone, Copy)]
pub enum DatatypeKind<'a> {
    Primitive(Primitive),
    Composite(&'a [Element<'a>]),
    Timestamp,
    /// A closed set of exact decoded UTF-8 strings; no trimming or case folding.
    Codes(&'a [&'a str]),
}
pub struct Datatype<'a> {
    pub name: &'a str,
    pub kind: DatatypeKind<'a>,
}
#[derive(Clone, Copy)]
pub enum TypeRef<'a> {
    Fixed(&'a str),
    ByField {
        selector: usize,
        allowed: &'a [&'a str],
    },
}
#[derive(Clone, Copy)]
pub enum Length {
    WireBytes(usize),
    ScalarBytes(usize),
    Utf8Characters(usize),
}
#[derive(Clone, Copy)]
pub struct Element<'a> {
    pub position: usize,
    pub datatype: TypeRef<'a>,
    pub occurs: Occurs,
    pub nullable: bool,
    pub length: Option<Length>,
}
impl<'a> Element<'a> {
    pub const fn optional(position: usize, datatype: &'a str) -> Self {
        Self {
            position,
            datatype: TypeRef::Fixed(datatype),
            occurs: Occurs::OPTIONAL,
            nullable: false,
            length: None,
        }
    }
    pub const fn required(position: usize, datatype: &'a str) -> Self {
        Self {
            occurs: Occurs::ONCE,
            ..Self::optional(position, datatype)
        }
    }
    pub const fn by_field(
        position: usize,
        selector: usize,
        allowed: &'a [&'a str],
        occurs: Occurs,
    ) -> Self {
        Self {
            position,
            datatype: TypeRef::ByField { selector, allowed },
            occurs,
            nullable: false,
            length: None,
        }
    }
}
pub struct SegmentDefinition<'a> {
    pub name: &'a str,
    pub id: [u8; 3],
    pub fields: &'a [Element<'a>],
}

enum Ref {
    Fixed(usize),
    ByField {
        selector: usize,
        allowed: Vec<usize>,
    },
}
struct Slot {
    position: usize,
    datatype: Ref,
    occurs: Occurs,
    nullable: bool,
    length: Option<Length>,
}
enum Kind<'a> {
    Primitive(Primitive),
    Composite(Vec<Slot>),
    Timestamp,
    Codes(Vec<Cow<'a, str>>),
}
#[cfg(feature = "schema-package")]
impl Kind<'_> {
    fn into_owned(self) -> Kind<'static> {
        match self {
            Self::Primitive(p) => Kind::Primitive(p),
            Self::Composite(slots) => Kind::Composite(slots),
            Self::Timestamp => Kind::Timestamp,
            Self::Codes(values) => Kind::Codes(
                values
                    .into_iter()
                    .map(|s| Cow::Owned(s.into_owned()))
                    .collect(),
            ),
        }
    }
}
struct Type<'a> {
    name: Cow<'a, str>,
    kind: Kind<'a>,
}
struct Segment<'a> {
    name: Cow<'a, str>,
    id: [u8; 3],
    fields: Vec<Slot>,
    conditions: Vec<CompiledCondition<'a>>,
}
/// Immutable caller-defined datatypes and named segment profiles; no bundled standards data.
pub struct FieldSchema<'a> {
    types: Vec<Type<'a>>,
    segments: Vec<Segment<'a>>,
}

impl<'a> FieldSchema<'a> {
    #[cfg(feature = "schema-package")]
    pub(super) fn identity(&self, hash: &mut super::package::Identity) {
        let conditional = self.format_version() >= 3;
        hash.number(self.types.len());
        for datatype in &self.types {
            hash.text(&datatype.name);
            match &datatype.kind {
                Kind::Primitive(primitive) => {
                    hash.tag(0);
                    hash.tag(match primitive {
                        Primitive::Bytes => 0,
                        Primitive::Ascii => 1,
                        Primitive::Utf8 => 2,
                        Primitive::Number => 3,
                        Primitive::Date => 4,
                        Primitive::DateTime => 5,
                    });
                }
                Kind::Timestamp => hash.tag(1),
                Kind::Codes(values) => {
                    hash.tag(3);
                    hash.number(values.len());
                    for value in values {
                        hash.text(value);
                    }
                }
                Kind::Composite(slots) => {
                    hash.tag(2);
                    identity_slots(slots, hash);
                }
            }
        }
        hash.number(self.segments.len());
        for segment in &self.segments {
            hash.text(&segment.name);
            hash.bytes(&segment.id);
            identity_slots(&segment.fields, hash);
            if conditional {
                hash.number(segment.conditions.len());
                for condition in &segment.conditions {
                    condition.identity(hash);
                }
            }
        }
    }
    #[cfg(feature = "schema-package")]
    pub(super) fn into_owned(self) -> FieldSchema<'static> {
        FieldSchema {
            types: self
                .types
                .into_iter()
                .map(|t| Type {
                    name: Cow::Owned(t.name.into_owned()),
                    kind: t.kind.into_owned(),
                })
                .collect(),
            segments: self
                .segments
                .into_iter()
                .map(|s| Segment {
                    name: Cow::Owned(s.name.into_owned()),
                    id: s.id,
                    fields: s.fields,
                    conditions: s
                        .conditions
                        .into_iter()
                        .map(CompiledCondition::into_owned)
                        .collect(),
                })
                .collect(),
        }
    }
    #[cfg(feature = "schema-package")]
    pub(super) fn write_file(
        &self,
        w: &mut super::package::file::Writer,
    ) -> Result<(), super::package::FileError> {
        let conditional = self.format_version() >= 3;
        w.count(self.types.len())?;
        for t in &self.types {
            w.text(&t.name)?;
            match &t.kind {
                Kind::Primitive(p) => {
                    w.tag(0)?;
                    w.tag(match p {
                        Primitive::Bytes => 0,
                        Primitive::Ascii => 1,
                        Primitive::Utf8 => 2,
                        Primitive::Number => 3,
                        Primitive::Date => 4,
                        Primitive::DateTime => 5,
                    })?;
                }
                Kind::Timestamp => w.tag(1)?,
                Kind::Codes(values) => {
                    w.tag(3)?;
                    w.count(values.len())?;
                    for value in values {
                        w.text(value)?;
                    }
                }
                Kind::Composite(slots) => {
                    w.tag(2)?;
                    self.write_slots(slots, w)?;
                }
            }
        }
        w.count(self.segments.len())?;
        for s in &self.segments {
            w.text(&s.name)?;
            w.raw(&s.id)?;
            self.write_slots(&s.fields, w)?;
            if conditional {
                w.count(s.conditions.len())?;
                for condition in &s.conditions {
                    condition.write_file(w)?;
                }
            }
        }
        Ok(())
    }
    #[cfg(feature = "schema-package")]
    fn write_slots(
        &self,
        slots: &[Slot],
        w: &mut super::package::file::Writer,
    ) -> Result<(), super::package::FileError> {
        w.count(slots.len())?;
        for s in slots {
            w.count(s.position)?;
            w.occurs(s.occurs)?;
            w.tag(u8::from(s.nullable))?;
            match s.length {
                None => w.tag(0)?,
                Some(length) => {
                    let (tag, n) = match length {
                        Length::WireBytes(n) => (1, n),
                        Length::ScalarBytes(n) => (2, n),
                        Length::Utf8Characters(n) => (3, n),
                    };
                    w.tag(tag)?;
                    w.count(n)?;
                }
            }
            match &s.datatype {
                Ref::Fixed(i) => {
                    w.tag(0)?;
                    w.text(&self.types[*i].name)?;
                }
                Ref::ByField { selector, allowed } => {
                    w.tag(1)?;
                    w.count(*selector)?;
                    w.count(allowed.len())?;
                    for i in allowed {
                        w.text(&self.types[*i].name)?;
                    }
                }
            }
        }
        Ok(())
    }
    #[cfg(feature = "schema-package")]
    pub(super) fn format_version(&self) -> u64 {
        if self.segments.iter().any(|s| !s.conditions.is_empty()) {
            3
        } else if self.types.iter().any(|t| matches!(t.kind, Kind::Codes(_))) {
            2
        } else {
            1
        }
    }
    pub fn compile(
        types: &'a [Datatype<'a>],
        segments: &'a [SegmentDefinition<'a>],
        limits: CompileLimits,
    ) -> Result<Self, Error> {
        Self::compile_with_conditions(types, segments, &[], limits)
    }
    pub fn compile_with_conditions(
        types: &'a [Datatype<'a>],
        segments: &'a [SegmentDefinition<'a>],
        conditions: &'a [Condition<'a>],
        limits: CompileLimits,
    ) -> Result<Self, Error> {
        if limits.max_depth == 0 || limits.max_depth > 64 || limits.max_work == 0 {
            return Err(definition());
        }
        let mut budget = limits.max_rules;
        let mut work = limits.max_work;
        take(&mut budget, types.len())?;
        take(&mut budget, segments.len())?;
        take(&mut work, types.len())?;
        take(&mut work, segments.len())?;
        if types.iter().any(|t| !identifier(t.name))
            || segments.iter().any(|s| {
                !identifier(s.name)
                    || !s
                        .id
                        .iter()
                        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
            })
        {
            return Err(definition());
        }
        let mut type_defs: Vec<_> = types.iter().collect();
        type_defs.sort_unstable_by_key(|t| t.name);
        let mut segment_defs: Vec<_> = segments.iter().collect();
        segment_defs.sort_unstable_by_key(|s| s.name);
        if type_defs.windows(2).any(|p| p[0].name == p[1].name)
            || segment_defs.windows(2).any(|p| p[0].name == p[1].name)
        {
            return Err(definition());
        }
        let names: Vec<_> = type_defs.iter().map(|t| t.name).collect();
        let mut compiled = Vec::with_capacity(type_defs.len());
        for datatype in type_defs {
            let kind = match datatype.kind {
                DatatypeKind::Primitive(p) => Kind::Primitive(p),
                DatatypeKind::Timestamp => Kind::Timestamp,
                DatatypeKind::Codes(values) => {
                    if values.is_empty() {
                        return Err(definition());
                    }
                    take(&mut budget, values.len())?;
                    take(&mut work, values.len())?;
                    for value in values {
                        if value.len() > 4096 {
                            return Err(Error::new(ErrorKind::Limit));
                        }
                        take(&mut work, value.len())?;
                        if value.is_empty() || value.chars().any(char::is_control) {
                            return Err(definition());
                        }
                    }
                    let mut values: Vec<_> = values.iter().map(|&s| Cow::Borrowed(s)).collect();
                    values.sort_unstable();
                    if values.windows(2).any(|p| p[0] == p[1]) {
                        return Err(definition());
                    }
                    Kind::Codes(values)
                }
                DatatypeKind::Composite(elements) => {
                    if elements.is_empty() {
                        return Err(definition());
                    }
                    Kind::Composite(slots(
                        elements,
                        false,
                        &names,
                        limits.max_rules,
                        &mut budget,
                        &mut work,
                    )?)
                }
            };
            compiled.push(Type {
                name: datatype.name.into(),
                kind,
            });
        }
        let mut states = vec![0; compiled.len()];
        for i in 0..compiled.len() {
            depth(i, &compiled, &mut states, limits.max_depth, &mut work)?;
        }
        let mut result = Self {
            types: compiled,
            segments: Vec::with_capacity(segment_defs.len()),
        };
        for segment in segment_defs {
            let fields = slots(
                segment.fields,
                true,
                &names,
                limits.max_rules,
                &mut budget,
                &mut work,
            )?;
            for slot in &fields {
                if let Ref::ByField { selector, .. } = &slot.datatype {
                    let index = fields
                        .binary_search_by_key(selector, |s| s.position)
                        .map_err(|_| definition())?;
                    let source = &fields[index];
                    if source.occurs.max != Some(1)
                        || !matches!(source.datatype, Ref::Fixed(i) if matches!(result.types[i].kind, Kind::Primitive(Primitive::Bytes | Primitive::Ascii | Primitive::Utf8) | Kind::Codes(_)))
                    {
                        return Err(definition());
                    }
                }
                if segment.id == *b"MSH"
                    && slot.position <= 2
                    && (!matches!(slot.datatype, Ref::Fixed(i) if matches!(result.types[i].kind, Kind::Primitive(_) | Kind::Codes(_)))
                        || slot.occurs.max != Some(1))
                {
                    return Err(definition());
                }
                check_length(slot, &result.types)?;
            }
            result.segments.push(Segment {
                name: segment.name.into(),
                id: segment.id,
                fields,
                conditions: Vec::new(),
            });
        }
        for datatype in &result.types {
            if let Kind::Composite(slots) = &datatype.kind {
                for slot in slots {
                    check_length(slot, &result.types)?;
                }
            }
        }
        conditions::compile(&mut result, conditions, &mut budget, &mut work)?;
        Ok(result)
    }
    /// Verify every grammar leaf has an explicit, matching field profile before activation.
    pub fn check_catalog(&self, catalog: &Catalog<'_>) -> Result<(), Error> {
        for structure in &catalog.structures {
            for (id, profile) in structure.profiles() {
                let segment = profile
                    .and_then(|p| self.profile(p))
                    .ok_or_else(definition)?;
                if segment.id != id {
                    return Err(definition());
                }
            }
        }
        Ok(())
    }
    /// Resolve a declared field's exact type; dynamic selectors are read from this same segment.
    pub fn datatype(
        &self,
        message: &Message,
        profile: &str,
        path: &[usize; 2],
    ) -> Result<&str, Issue> {
        let segment = self
            .profile(profile)
            .filter(|s| {
                message
                    .segment(path[0])
                    .is_some_and(|actual| actual.id() == &s.id)
            })
            .ok_or_else(|| Issue::at(message, &path[..1], IssueKind::Profile))?;
        let field = segment
            .fields
            .binary_search_by_key(&path[1], |s| s.position)
            .map_err(|_| Issue::at(message, path, IssueKind::Unmodeled))?;
        self.resolve(message, path[0], &segment.fields[field])
            .map(|i| self.types[i].name.as_ref())
    }
    fn profile(&self, name: &str) -> Option<&Segment<'a>> {
        self.segments
            .binary_search_by(|s| s.name.as_ref().cmp(name))
            .ok()
            .map(|i| &self.segments[i])
    }
    fn resolve(&self, message: &Message, segment: usize, slot: &Slot) -> Result<usize, Issue> {
        match &slot.datatype {
            Ref::Fixed(i) => Ok(*i),
            Ref::ByField { selector, allowed } => {
                let path = [segment, *selector];
                let value = message.value(&path);
                if value.presence() != Presence::Value {
                    return Err(Issue::at(
                        message,
                        &path,
                        IssueKind::Selector {
                            target: slot.position,
                        },
                    ));
                }
                if value.raw().is_some_and(|b| b.len() > 320) {
                    return Err(Issue::at(
                        message,
                        &path,
                        IssueKind::UnknownDatatype {
                            target: slot.position,
                        },
                    ));
                }
                let bytes = value
                    .decode()
                    .map_err(|e| Issue::wire(message, &path, e, false))?;
                let index = self
                    .types
                    .binary_search_by(|t| t.name.as_bytes().cmp(&bytes))
                    .ok()
                    .filter(|i| allowed.binary_search(i).is_ok());
                index.ok_or_else(|| {
                    Issue::at(
                        message,
                        &path,
                        IssueKind::UnknownDatatype {
                            target: slot.position,
                        },
                    )
                })
            }
        }
    }
}
#[cfg(feature = "schema-package")]
fn identity_slots(slots: &[Slot], hash: &mut super::package::Identity) {
    hash.number(slots.len());
    for slot in slots {
        hash.number(slot.position);
        hash.occurs(slot.occurs);
        hash.tag(u8::from(slot.nullable));
        match slot.length {
            None => hash.tag(0),
            Some(length) => {
                let (tag, maximum) = match length {
                    Length::WireBytes(n) => (1, n),
                    Length::ScalarBytes(n) => (2, n),
                    Length::Utf8Characters(n) => (3, n),
                };
                hash.tag(tag);
                hash.number(maximum);
            }
        }
        match &slot.datatype {
            Ref::Fixed(index) => {
                hash.tag(0);
                hash.number(*index);
            }
            Ref::ByField { selector, allowed } => {
                hash.tag(1);
                hash.number(*selector);
                hash.number(allowed.len());
                for index in allowed {
                    hash.number(*index);
                }
            }
        }
    }
}
fn definition() -> Error {
    Error::new(ErrorKind::Definition)
}
fn take(budget: &mut usize, count: usize) -> Result<(), Error> {
    *budget = budget
        .checked_sub(count)
        .ok_or_else(|| Error::new(ErrorKind::Limit))?;
    Ok(())
}
fn slots(
    elements: &[Element<'_>],
    field: bool,
    names: &[&str],
    maximum: usize,
    budget: &mut usize,
    work: &mut usize,
) -> Result<Vec<Slot>, Error> {
    take(budget, elements.len())?;
    take(work, elements.len())?;
    let mut result = Vec::with_capacity(elements.len());
    for element in elements {
        if element.position == 0
            || element.position > maximum
            || element
                .occurs
                .max
                .is_some_and(|max| max < element.occurs.min)
            || (!field && (element.occurs.min > 1 || element.occurs.max.is_none_or(|max| max > 1)))
        {
            return Err(definition());
        }
        let lookup = |name: &str| names.binary_search(&name).map_err(|_| definition());
        let datatype = match element.datatype {
            TypeRef::Fixed(name) => Ref::Fixed(lookup(name)?),
            TypeRef::ByField { selector, allowed } => {
                if !field || selector == 0 || selector == element.position || allowed.is_empty() {
                    return Err(definition());
                }
                take(budget, allowed.len())?;
                take(work, allowed.len())?;
                let mut types = allowed
                    .iter()
                    .map(|name| lookup(name))
                    .collect::<Result<Vec<_>, _>>()?;
                types.sort_unstable();
                if types.windows(2).any(|p| p[0] == p[1]) {
                    return Err(definition());
                }
                Ref::ByField {
                    selector,
                    allowed: types,
                }
            }
        };
        result.push(Slot {
            position: element.position,
            datatype,
            occurs: element.occurs,
            nullable: element.nullable,
            length: element.length,
        });
    }
    result.sort_unstable_by_key(|s| s.position);
    if result.windows(2).any(|p| p[0].position == p[1].position) {
        return Err(definition());
    }
    Ok(result)
}
fn depth(
    index: usize,
    types: &[Type<'_>],
    states: &mut [u8],
    remaining: usize,
    work: &mut usize,
) -> Result<u8, Error> {
    take(work, 1)?;
    if states[index] >= 2 {
        let cached = states[index] - 2;
        return if usize::from(cached) < remaining {
            Ok(cached)
        } else {
            Err(Error::new(ErrorKind::Limit))
        };
    }
    if states[index] == 1 {
        return Err(definition());
    }
    if remaining == 0 {
        return Err(Error::new(ErrorKind::Limit));
    }
    states[index] = 1;
    let mut maximum = match types[index].kind {
        Kind::Primitive(_) | Kind::Codes(_) => 0,
        _ => 1,
    };
    if let Kind::Composite(slots) = &types[index].kind {
        for slot in slots {
            if let Ref::Fixed(child) = slot.datatype {
                maximum = maximum.max(1 + depth(child, types, states, remaining - 1, work)?);
            }
        }
    }
    if maximum > 2 {
        return Err(definition());
    }
    states[index] = maximum + 2;
    Ok(maximum)
}
fn check_length(slot: &Slot, types: &[Type<'_>]) -> Result<(), Error> {
    if matches!(
        slot.length,
        Some(Length::ScalarBytes(_) | Length::Utf8Characters(_))
    ) {
        let scalar = |i: usize| matches!(types[i].kind, Kind::Primitive(_) | Kind::Codes(_));
        let valid = match &slot.datatype {
            Ref::Fixed(i) => scalar(*i),
            Ref::ByField { allowed, .. } => allowed.iter().all(|i| scalar(*i)),
        };
        if !valid {
            return Err(definition());
        }
    }
    Ok(())
}
