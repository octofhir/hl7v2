#![doc = include_str!("../../docs/schema.md")]
pub mod fields;
mod grammar;

use crate::wire::{self, Message, Presence, Span};
pub use grammar::{Occurrence, StructureSchema, View};
use std::{borrow::Cow, fmt};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Definition,
    AmbiguousGrammar,
    Limit,
    Header,
    Version,
    VersionVariant,
    UnknownEvent,
    AmbiguousEvent,
    UnknownStructure,
    StructureConflict,
    UnsupportedProfile,
    MissingSegment,
    UnexpectedSegment,
    Wire(wire::ErrorKind),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    pub span: Span,
    /// One-based wire path, with zero for unspecified coordinates.
    pub path: [usize; 5],
    /// Definition index inside the selected structure; independent of wire positions.
    pub rule: Option<usize>,
}
impl Error {
    fn new(kind: ErrorKind) -> Self {
        Self {
            kind,
            span: 0..0,
            path: [0; 5],
            rule: None,
        }
    }
    fn at(message: &Message, path: &[usize], kind: ErrorKind) -> Self {
        let mut error = Self::new(kind);
        error.path[..path.len()].copy_from_slice(path);
        error.span = message.value(path).span().unwrap_or(0..0);
        error
    }
    fn rule(mut self, index: usize) -> Self {
        self.rule = Some(index);
        self
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "HL7 schema {:?} at bytes {}..{} (rule {:?})",
            self.kind, self.span.start, self.span.end, self.rule
        )
    }
}
impl std::error::Error for Error {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Occurs {
    pub min: usize,
    pub max: Option<usize>,
}
impl Occurs {
    pub const ONCE: Self = Self {
        min: 1,
        max: Some(1),
    };
    pub const OPTIONAL: Self = Self {
        min: 0,
        max: Some(1),
    };
    pub const MANY: Self = Self { min: 0, max: None };
}
#[derive(Clone, Copy)]
pub enum Content<'a> {
    Segment([u8; 3]),
    TypedSegment { id: [u8; 3], profile: &'a str },
    Group(&'a [Rule<'a>]),
}
#[derive(Clone, Copy)]
pub struct Rule<'a> {
    pub name: &'a str,
    pub occurs: Occurs,
    pub content: Content<'a>,
}
impl<'a> Rule<'a> {
    pub const fn typed_segment(
        name: &'a str,
        id: [u8; 3],
        profile: &'a str,
        occurs: Occurs,
    ) -> Self {
        Self {
            name,
            occurs,
            content: Content::TypedSegment { id, profile },
        }
    }
    pub const fn segment(name: &'a str, id: [u8; 3], occurs: Occurs) -> Self {
        Self {
            name,
            occurs,
            content: Content::Segment(id),
        }
    }
    pub const fn group(name: &'a str, children: &'a [Rule<'a>], occurs: Occurs) -> Self {
        Self {
            name,
            occurs,
            content: Content::Group(children),
        }
    }
}
pub struct Structure<'a> {
    pub name: &'a str,
    pub rules: &'a [Rule<'a>],
}
pub struct Binding<'a> {
    pub message: &'a str,
    pub event: &'a str,
    pub structure: &'a str,
}
#[derive(Clone, Copy)]
pub struct CompileLimits {
    pub max_rules: usize,
    pub max_depth: usize,
    pub max_work: usize,
}
impl Default for CompileLimits {
    fn default() -> Self {
        Self {
            max_rules: 8192,
            max_depth: 32,
            max_work: 1_000_000,
        }
    }
}
#[derive(Clone, Copy)]
pub struct MatchLimits {
    pub max_occurrences: usize,
    pub max_steps: usize,
}
impl Default for MatchLimits {
    fn default() -> Self {
        Self {
            max_occurrences: 262_144,
            max_steps: 1_000_000,
        }
    }
}

/// Compiled caller-supplied definitions for one exact release; no bundled standards catalog.
pub struct Catalog<'a> {
    version: &'a str,
    structures: Vec<StructureSchema<'a>>,
    bindings: Vec<&'a Binding<'a>>,
}
impl<'a> Catalog<'a> {
    pub fn compile(
        version: &'a str,
        structures: &'a [Structure<'a>],
        bindings: &'a [Binding<'a>],
        limits: CompileLimits,
    ) -> Result<Self, Error> {
        if version.is_empty()
            || version.len() > 32
            || !version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.')
            || limits.max_depth == 0
            || limits.max_depth > 64
            || limits.max_rules == 0
            || limits.max_work == 0
            || structures.is_empty()
        {
            return Err(Error::new(ErrorKind::Definition));
        }
        if structures.len() > limits.max_rules || bindings.len() > limits.max_rules {
            return Err(Error::new(ErrorKind::Limit));
        }
        // Reject oversized names before sorting can compare unbounded string prefixes.
        if structures
            .iter()
            .any(|s| !identifier(s.name) || s.name.bytes().any(|b| b.is_ascii_lowercase()))
        {
            return Err(Error::new(ErrorKind::Definition));
        }
        let mut names: Vec<_> = structures.iter().collect();
        names.sort_unstable_by_key(|s| s.name);
        if names.windows(2).any(|pair| pair[0].name == pair[1].name) {
            return Err(Error::new(ErrorKind::Definition));
        }
        let mut rules = limits.max_rules;
        let mut work = limits.max_work;
        let structures = names
            .into_iter()
            .map(|s| grammar::compile(s, limits.max_depth, &mut rules, &mut work))
            .collect::<Result<Vec<_>, _>>()?;
        let mut catalog = Self {
            version,
            structures,
            bindings: Vec::with_capacity(bindings.len()),
        };
        for binding in bindings {
            if !code(binding.message.as_bytes())
                || !code(binding.event.as_bytes())
                || catalog.structure(binding.structure).is_none()
            {
                return Err(Error::new(ErrorKind::Definition));
            }
            catalog.bindings.push(binding);
        }
        catalog
            .bindings
            .sort_unstable_by_key(|b| (b.message, b.event, b.structure));
        if catalog.bindings.windows(2).any(|p| {
            (p[0].message, p[0].event, p[0].structure) == (p[1].message, p[1].event, p[1].structure)
        }) {
            return Err(Error::new(ErrorKind::Definition));
        }
        Ok(catalog)
    }
    pub fn version(&self) -> &str {
        self.version
    }
    fn structure(&self, name: &str) -> Option<&StructureSchema<'a>> {
        self.structures
            .binary_search_by_key(&name, |s| s.name())
            .ok()
            .map(|n| &self.structures[n])
    }
    /// Resolve MSH-12.1, then an explicit compatible MSH-9.3 or an unambiguous event binding.
    pub fn resolve(&self, message: &Message) -> Result<&StructureSchema<'a>, Error> {
        for field in [9, 12] {
            let value = message.value(&[1, field]);
            if value.child_count() != 1 || value.child(1).child_count() > 3 {
                return Err(Error::at(message, &[1, field], ErrorKind::Header));
            }
        }
        let version = token(message, &[1, 12, 1, 1], 32)?;
        if version.as_ref() != self.version.as_bytes() {
            return Err(Error::at(message, &[1, 12, 1, 1], ErrorKind::Version));
        }
        for component in [2, 3] {
            let path = [1, 12, 1, component];
            if !matches!(
                message.value(&path).presence(),
                Presence::Missing | Presence::Empty
            ) {
                return Err(Error::at(message, &path, ErrorKind::VersionVariant));
            }
        }
        if !matches!(
            message.value(&[1, 21]).presence(),
            Presence::Missing | Presence::Empty
        ) {
            return Err(Error::at(message, &[1, 21], ErrorKind::UnsupportedProfile));
        }
        let code_path = [1, 9, 1, 1];
        let event_path = [1, 9, 1, 2];
        let message_code = token(message, &code_path, 3)?;
        let event = token(message, &event_path, 3)?;
        if !code(&message_code) {
            return Err(Error::at(message, &code_path, ErrorKind::Header));
        }
        if !code(&event) {
            return Err(Error::at(message, &event_path, ErrorKind::Header));
        }
        let start = self.bindings.partition_point(|b| {
            (b.message.as_bytes(), b.event.as_bytes()) < (message_code.as_ref(), event.as_ref())
        });
        let bindings = &self.bindings[start..];
        let end = bindings.partition_point(|b| {
            (b.message.as_bytes(), b.event.as_bytes()) == (message_code.as_ref(), event.as_ref())
        });
        let bindings = &bindings[..end];
        let path = [1, 9, 1, 3];
        if matches!(
            message.value(&path).presence(),
            Presence::Missing | Presence::Empty
        ) {
            let binding = match bindings {
                [binding] => binding,
                [] => return Err(Error::at(message, &event_path, ErrorKind::UnknownEvent)),
                _ => return Err(Error::at(message, &path, ErrorKind::AmbiguousEvent)),
            };
            return self
                .structure(binding.structure)
                .ok_or_else(|| Error::new(ErrorKind::Definition));
        }
        let name = token(message, &path, 64)?;
        if !name
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || *b == b'_')
        {
            return Err(Error::at(message, &path, ErrorKind::Header));
        }
        let name =
            std::str::from_utf8(&name).map_err(|_| Error::at(message, &path, ErrorKind::Header))?;
        let structure = self
            .structure(name)
            .ok_or_else(|| Error::at(message, &path, ErrorKind::UnknownStructure))?;
        if !bindings.iter().any(|b| b.structure == name) {
            return Err(Error::at(message, &path, ErrorKind::StructureConflict));
        }
        Ok(structure)
    }
    pub fn group<'s, 'm>(
        &'s self,
        message: &'m Message,
        limits: MatchLimits,
    ) -> Result<View<'s, 'm>, Error> {
        self.resolve(message)?.group(message, limits)
    }
}
fn code(bytes: &[u8]) -> bool {
    bytes.len() == 3
        && bytes
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}
fn identifier(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && name.as_bytes()[0].is_ascii_alphabetic()
}
fn token<'m>(message: &'m Message, path: &[usize], max: usize) -> Result<Cow<'m, [u8]>, Error> {
    let value = message.value(path);
    if value.presence() != Presence::Value || value.raw().is_some_and(|b| b.len() > max * 5) {
        return Err(Error::at(message, path, ErrorKind::Header));
    }
    let bytes = value.decode().map_err(|e| {
        let mut error = Error::at(message, path, ErrorKind::Wire(e.kind));
        error.span = e.span;
        error
    })?;
    if bytes.is_empty() || bytes.len() > max {
        return Err(Error::at(message, path, ErrorKind::Header));
    }
    Ok(bytes)
}
