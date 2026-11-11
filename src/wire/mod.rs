//! Schema-free ER7. Paths are one-based: [segment, field, repetition, component, subcomponent].
//! Segment numbers count all segments in wire order, including MSH. Missing path suffixes
//! select the containing value; for example, [2, 5, 1, 2] selects a second component.

mod edit;
mod escape;
mod parse;
#[doc = include_str!("../../docs/scalars.md")]
pub mod scalar;

pub use edit::{Edit, Editor};
pub(crate) use escape::{append, decode as decode_scalar, encode as encode_scalar};
pub use scalar::{Date, DateTime, Decimal, Precision};
use std::{borrow::Cow, fmt, ops::Range, sync::Arc};

pub type Span = Range<usize>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Header,
    Delimiters,
    Segment,
    LineEnding,
    Limit,
    Missing,
    Null,
    Empty,
    Number,
    Date,
    DateTime,
    Composite,
    Escape,
    InvalidHex,
    Path,
    EditConflict,
    InteriorUnset,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    pub span: Span,
}

impl Error {
    fn new(kind: ErrorKind, span: Span) -> Self {
        Self { kind, span }
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ER7 {:?} at bytes {}..{}",
            self.kind, self.span.start, self.span.end
        )
    }
}
impl std::error::Error for Error {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    Missing,
    Empty,
    Null,
    Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticKind {
    LineEnding,
    MissingTerminator,
    UnterminatedEscape,
    InvalidHex,
    UnsupportedEscape,
    Truncation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub kind: DiagnosticKind,
    pub span: Span,
    /// Zero entries denote an unspecified position; populated positions are one-based.
    pub path: [usize; 5],
}

#[derive(Debug, Clone, Copy)]
pub struct ParseOptions {
    pub compatible_line_endings: bool,
    pub max_bytes: usize,
    pub max_segments: usize,
    pub max_nodes: usize,
    pub max_diagnostics: usize,
}
impl Default for ParseOptions {
    fn default() -> Self {
        Self {
            compatible_line_endings: false,
            max_bytes: 8 * 1024 * 1024,
            max_segments: 16_384,
            max_nodes: 262_144,
            max_diagnostics: 1024,
        }
    }
}

/// Message-local ASCII punctuation. Encoding order is component, repetition, escape, subcomponent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Delimiters {
    pub field: u8,
    pub encoding: [u8; 4],
    pub truncation: Option<u8>,
}
impl Delimiters {
    fn separators(self) -> [u8; 4] {
        [
            self.field,
            self.encoding[1],
            self.encoding[0],
            self.encoding[3],
        ]
    }
}

struct Node {
    span: Span,
    children: Range<usize>,
}
struct SegmentData {
    id: [u8; 3],
    span: Span,
    end: usize,
    fields: Range<usize>,
}
struct Index {
    delimiters: Delimiters,
    nodes: Vec<Node>,
    segments: Vec<SegmentData>,
    diagnostics: Vec<Diagnostic>,
    omitted: usize,
    unterminated: Option<Span>,
}

/// Original bytes and a flat positional index, both shared by clones.
#[derive(Clone)]
pub struct Message {
    bytes: Arc<[u8]>,
    index: Arc<Index>,
    options: ParseOptions,
}
impl fmt::Debug for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Message")
            .field("bytes", &self.bytes.len())
            .field("segments", &self.index.segments.len())
            .finish_non_exhaustive()
    }
}

impl Message {
    pub fn parse(bytes: impl AsRef<[u8]>) -> Result<Self, Error> {
        Self::parse_with_options(bytes, ParseOptions::default())
    }
    pub fn parse_with_options(
        bytes: impl AsRef<[u8]>,
        options: ParseOptions,
    ) -> Result<Self, Error> {
        let bytes = bytes.as_ref();
        if bytes.len() > options.max_bytes {
            return Err(Error::new(ErrorKind::Limit, 0..bytes.len()));
        }
        Self::from_shared(Arc::from(bytes), options)
    }
    pub fn from_shared(bytes: Arc<[u8]>, options: ParseOptions) -> Result<Self, Error> {
        let index = Arc::new(parse::build(&bytes, options)?);
        Ok(Self {
            bytes,
            index,
            options,
        })
    }
    pub fn original(&self) -> &[u8] {
        &self.bytes
    }
    pub fn delimiters(&self) -> Delimiters {
        self.index.delimiters
    }
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.index.diagnostics
    }
    pub fn omitted_diagnostics(&self) -> usize {
        self.index.omitted
    }
    pub fn node_count(&self) -> usize {
        self.index.nodes.len()
    }
    pub fn segments(&self) -> impl ExactSizeIterator<Item = Segment<'_>> {
        (0..self.index.segments.len()).map(|index| Segment {
            message: self,
            index,
        })
    }
    pub fn segment_named(&self, id: &[u8; 3], occurrence: usize) -> Option<Segment<'_>> {
        self.segments()
            .filter(|segment| segment.id() == id)
            .nth(occurrence.checked_sub(1)?)
    }
    pub fn value(&self, path: &[usize]) -> Value<'_> {
        let mut node = None;
        if valid_path(path)
            && let Some(segment) = self.index.segments.get(path[0] - 1)
        {
            node = segment.fields.clone().nth(path[1] - 1);
            for &number in &path[2..] {
                node = node.and_then(|id| self.index.nodes[id].children.clone().nth(number - 1));
            }
        }
        Value {
            message: self,
            node,
        }
    }
    pub fn edit(&self) -> Editor<'_> {
        Editor::new(self)
    }
}

fn valid_path(path: &[usize]) -> bool {
    (2..=5).contains(&path.len()) && !path.contains(&0)
}

#[derive(Debug, Clone, Copy)]
pub struct Segment<'a> {
    message: &'a Message,
    index: usize,
}
impl<'a> Segment<'a> {
    pub fn id(self) -> &'a [u8; 3] {
        &self.message.index.segments[self.index].id
    }
    pub fn occurrence(self) -> usize {
        self.index + 1
    }
    pub fn span(self) -> Span {
        self.message.index.segments[self.index].span.clone()
    }
    pub fn raw(self) -> &'a [u8] {
        &self.message.bytes[self.span()]
    }
    pub fn terminator(self) -> &'a [u8] {
        let data = &self.message.index.segments[self.index];
        &self.message.bytes[data.span.end..data.end]
    }
    pub fn field_count(self) -> usize {
        self.message.index.segments[self.index].fields.len()
    }
    pub fn field(self, number: usize) -> Value<'a> {
        self.message.value(&[self.index + 1, number])
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Value<'a> {
    message: &'a Message,
    node: Option<usize>,
}
impl<'a> Value<'a> {
    pub fn span(self) -> Option<Span> {
        self.node
            .map(|id| self.message.index.nodes[id].span.clone())
    }
    pub fn raw(self) -> Option<&'a [u8]> {
        self.span().map(|span| &self.message.bytes[span])
    }
    pub fn presence(self) -> Presence {
        match self.raw() {
            None => Presence::Missing,
            Some([]) => Presence::Empty,
            Some(b"\"\"") => Presence::Null,
            _ => Presence::Value,
        }
    }
    pub fn child_count(self) -> usize {
        self.node
            .map_or(0, |id| self.message.index.nodes[id].children.len())
    }
    pub fn child(self, number: usize) -> Self {
        let node = self.node.and_then(|id| {
            number
                .checked_sub(1)
                .and_then(|n| self.message.index.nodes[id].children.clone().nth(n))
        });
        Self { node, ..self }
    }
    /// Decode a scalar explicitly; unsupported escapes fail without changing the original.
    pub fn decode(self) -> Result<Cow<'a, [u8]>, Error> {
        let span = self.span().unwrap_or(0..0);
        match self.presence() {
            Presence::Missing => return Err(Error::new(ErrorKind::Missing, span)),
            Presence::Null => return Err(Error::new(ErrorKind::Null, span)),
            _ => {}
        }
        let mut node = self.node;
        while let Some(id) = node {
            let data = &self.message.index.nodes[id];
            if data.children.len() > 1 {
                return Err(Error::new(ErrorKind::Composite, span));
            }
            node = data.children.clone().next();
            if node.is_none() {
                let raw = &self.message.bytes[data.span.clone()];
                if id < 2 {
                    return Ok(Cow::Borrowed(raw));
                }
                return escape::decode(raw, data.span.start, self.message.delimiters());
            }
        }
        Err(Error::new(ErrorKind::Missing, span))
    }
}
