//! Bounded native package persistence. This is not a standards-data importer.
use super::{Definitions, Error, Fingerprint, Manifest, Package, Source};
use crate::schema::{
    Binding, Catalog, CompileLimits, Content, Occurs, Rule, Structure,
    fields::{
        Condition, Datatype, DatatypeKind, Element, FieldSchema, Length, Predicate, Primitive,
        Requirement, SegmentDefinition, TypeRef,
    },
};
use std::{
    fmt,
    io::{Read, Write},
};

/// Latest file grammar version; canonical V1 export remains available for legacy definitions.
pub const FILE_VERSION: u64 = 3;
const MAGIC: &[u8; 8] = b"OH7PKG\r\n";

#[derive(Clone, Copy)]
pub struct LoadLimits {
    pub max_bytes: usize,
    /// Cumulative entries across every table, group, component list and allowlist.
    pub max_items: usize,
    pub max_text_bytes: usize,
    pub compile: CompileLimits,
}
impl Default for LoadLimits {
    fn default() -> Self {
        Self {
            max_bytes: 8 * 1024 * 1024,
            max_items: 65_536,
            max_text_bytes: 4096,
            compile: CompileLimits::default(),
        }
    }
}
#[derive(Debug)]
pub enum FileError {
    Format { offset: usize },
    UnsupportedVersion,
    Limit,
    Package(Error),
    Io(std::io::Error),
}
impl fmt::Display for FileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Package(e) => e.fmt(f),
            Self::Io(e) => e.fmt(f),
            _ => write!(f, "HL7 package file {self:?}"),
        }
    }
}
impl std::error::Error for FileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Package(e) => Some(e),
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}
impl From<Error> for FileError {
    fn from(e: Error) -> Self {
        Self::Package(e)
    }
}
impl From<std::io::Error> for FileError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

pub struct OwnedSource {
    pub name: String,
    pub location: String,
    pub revision: String,
    pub sha256: Fingerprint,
    pub license: String,
    pub notice: String,
}
pub struct OwnedManifest {
    pub name: String,
    pub release: String,
    pub revision: String,
    pub producer: String,
    pub scope: String,
    pub limitations: String,
    pub sources: Vec<OwnedSource>,
}
impl From<Manifest<'_>> for OwnedManifest {
    fn from(m: Manifest<'_>) -> Self {
        Self {
            name: m.name.into(),
            release: m.release.into(),
            revision: m.revision.into(),
            producer: m.producer.into(),
            scope: m.scope.into(),
            limitations: m.limitations.into(),
            sources: m
                .sources
                .iter()
                .map(|s| OwnedSource {
                    name: s.name.into(),
                    location: s.location.into(),
                    revision: s.revision.into(),
                    sha256: s.sha256,
                    license: s.license.into(),
                    notice: s.notice.into(),
                })
                .collect(),
        }
    }
}
/// Fully owned immutable compiled package; independent of its input buffer/reader.
pub struct OwnedPackage {
    manifest: OwnedManifest,
    catalog: Catalog<'static>,
    fields: FieldSchema<'static>,
    digest: Fingerprint,
    format_version: u64,
}
impl OwnedPackage {
    /// Load with a trusted, externally retained historical identity, not a pin taken from this file.
    pub fn decode(
        bytes: &[u8],
        expected: Fingerprint,
        limits: LoadLimits,
    ) -> Result<Self, FileError> {
        if bytes.len() > limits.max_bytes {
            return Err(FileError::Limit);
        }
        if limits.compile.max_depth == 0 || limits.compile.max_depth > 64 {
            return Err(FileError::Limit);
        }
        let mut r = Reader {
            bytes,
            offset: 0,
            items: limits.max_items,
            limits,
        };
        if r.raw(8)? != MAGIC {
            return Err(r.format());
        }
        let file_version = r.number()?;
        let format_version = r.number()?;
        if !matches!((file_version, format_version), (1, 1) | (2, 2) | (3, 3)) {
            return Err(FileError::UnsupportedVersion);
        }
        let stored = r.array::<32>()?;
        if stored != expected {
            return Err(Error::DigestMismatch.into());
        }
        let [name, release, revision, producer, scope, limitations] = r.texts()?;
        let sources = (0..r.count()?)
            .map(|_| {
                let [name, location, revision, license, notice] = r.texts()?;
                Ok(Source {
                    name,
                    location,
                    revision,
                    license,
                    notice,
                    sha256: r.array()?,
                })
            })
            .collect::<Result<Vec<_>, FileError>>()?;
        let manifest = Manifest {
            name,
            release,
            revision,
            producer,
            scope,
            limitations,
            sources: &sources,
        };
        let structures = (0..r.count()?)
            .map(|_| {
                let name = r.text()?;
                let group = r.group(limits.compile.max_depth)?;
                Ok((name, group))
            })
            .collect::<Result<Vec<_>, FileError>>()?;
        let bindings = (0..r.count()?)
            .map(|_| {
                let [message, event, structure] = r.texts()?;
                Ok(Binding {
                    message,
                    event,
                    structure,
                })
            })
            .collect::<Result<Vec<_>, FileError>>()?;
        let types = (0..r.count()?)
            .map(|_| {
                let name = r.text()?;
                let kind = match r.tag()? {
                    0 => ParsedKind::Primitive(match r.tag()? {
                        0 => Primitive::Bytes,
                        1 => Primitive::Ascii,
                        2 => Primitive::Utf8,
                        3 => Primitive::Number,
                        4 => Primitive::Date,
                        5 => Primitive::DateTime,
                        _ => return Err(r.format()),
                    }),
                    1 => ParsedKind::Timestamp,
                    2 => ParsedKind::Composite(r.elements()?),
                    3 if format_version >= 2 => ParsedKind::Codes(
                        (0..r.count()?)
                            .map(|_| r.text())
                            .collect::<Result<Vec<_>, _>>()?,
                    ),
                    _ => return Err(r.format()),
                };
                Ok(ParsedDatatype { name, kind })
            })
            .collect::<Result<Vec<_>, FileError>>()?;
        let segments = (0..r.count()?)
            .map(|_| {
                let name = r.text()?;
                let id = r.array::<3>()?;
                let fields = r.elements()?;
                let conditions = if format_version >= 3 {
                    r.conditions(name)?
                } else {
                    Vec::new()
                };
                Ok((name, id, fields, conditions))
            })
            .collect::<Result<Vec<_>, FileError>>()?;
        if r.offset != bytes.len() {
            return Err(r.format());
        }
        // Projection buffers only live during activation. Compiled metadata is then promoted to
        // owned strings; indexes, FIRST sets and field plans are moved, not compiled a second time.
        let mut buffers: Vec<Vec<Vec<Rule<'_>>>> = structures
            .iter()
            .map(|(_, g)| (0..g.groups).map(|_| Vec::new()).collect())
            .collect();
        let defs = structures
            .iter()
            .zip(&mut buffers)
            .map(|((name, group), buffers)| Structure {
                name,
                rules: group.project(buffers),
            })
            .collect::<Vec<_>>();
        let components: Vec<_> = types
            .iter()
            .map(|t| match &t.kind {
                ParsedKind::Composite(elements) => project_elements(elements),
                _ => Vec::new(),
            })
            .collect();
        let datatypes: Vec<_> = types
            .iter()
            .zip(&components)
            .map(|(t, elements)| Datatype {
                name: t.name,
                kind: match &t.kind {
                    ParsedKind::Primitive(p) => DatatypeKind::Primitive(*p),
                    ParsedKind::Timestamp => DatatypeKind::Timestamp,
                    ParsedKind::Composite(_) => DatatypeKind::Composite(elements),
                    ParsedKind::Codes(values) => DatatypeKind::Codes(values),
                },
            })
            .collect();
        let conditions: Vec<_> = segments.iter().flat_map(|s| s.3.iter().copied()).collect();
        let fields: Vec<_> = segments.iter().map(|s| project_elements(&s.2)).collect();
        let segments: Vec<_> = segments
            .iter()
            .zip(&fields)
            .map(|(s, fields)| SegmentDefinition {
                name: s.0,
                id: s.1,
                fields,
            })
            .collect();
        let package = Package::compile_with_conditions(
            manifest,
            Definitions {
                structures: &defs,
                bindings: &bindings,
                datatypes: &datatypes,
                segments: &segments,
            },
            &conditions,
            limits.compile,
        )?;
        if package.format_version() != format_version {
            return Err(FileError::UnsupportedVersion);
        }
        let package = package.require_digest(expected)?;
        Ok(Self {
            manifest: package.manifest.into(),
            catalog: package.catalog.into_owned(),
            fields: package.fields.into_owned(),
            digest: package.digest,
            format_version,
        })
    }
    /// Read at most `max_bytes + 1` bytes, then decode. The host opens the local asset itself.
    pub fn read_from(
        reader: impl Read,
        expected: Fingerprint,
        limits: LoadLimits,
    ) -> Result<Self, FileError> {
        let cap = limits
            .max_bytes
            .checked_add(1)
            .and_then(|n| u64::try_from(n).ok())
            .ok_or(FileError::Limit)?;
        let mut bytes = Vec::new();
        reader.take(cap).read_to_end(&mut bytes)?;
        Self::decode(&bytes, expected, limits)
    }
    pub fn manifest(&self) -> &OwnedManifest {
        &self.manifest
    }
    pub fn catalog(&self) -> &Catalog<'static> {
        &self.catalog
    }
    pub fn fields(&self) -> &FieldSchema<'static> {
        &self.fields
    }
    pub fn format_version(&self) -> u64 {
        self.format_version
    }
    pub fn digest(&self) -> Fingerprint {
        self.digest
    }
    pub fn encode(&self, max_bytes: usize) -> Result<Vec<u8>, FileError> {
        let m = &self.manifest;
        let sources = m
            .sources
            .iter()
            .map(|s| Source {
                name: &s.name,
                location: &s.location,
                revision: &s.revision,
                sha256: s.sha256,
                license: &s.license,
                notice: &s.notice,
            })
            .collect::<Vec<_>>();
        encode(
            Manifest {
                name: &m.name,
                release: &m.release,
                revision: &m.revision,
                producer: &m.producer,
                scope: &m.scope,
                limitations: &m.limitations,
                sources: &sources,
            },
            &self.catalog,
            &self.fields,
            self.digest,
            self.format_version,
            max_bytes,
        )
    }
    pub fn write_to(&self, mut writer: impl Write, max_bytes: usize) -> Result<(), FileError> {
        writer.write_all(&self.encode(max_bytes)?)?;
        Ok(())
    }
}
impl Package<'_> {
    pub fn encode(&self, max_bytes: usize) -> Result<Vec<u8>, FileError> {
        encode(
            self.manifest,
            &self.catalog,
            &self.fields,
            self.digest,
            self.format_version,
            max_bytes,
        )
    }
    /// Validate the output byte limit before writing any bytes. I/O failures may leave partial output.
    pub fn write_to(&self, mut writer: impl Write, max_bytes: usize) -> Result<(), FileError> {
        writer.write_all(&self.encode(max_bytes)?)?;
        Ok(())
    }
}
fn encode(
    m: Manifest<'_>,
    catalog: &Catalog<'_>,
    fields: &FieldSchema<'_>,
    digest: Fingerprint,
    format_version: u64,
    max: usize,
) -> Result<Vec<u8>, FileError> {
    let mut w = Writer {
        bytes: Vec::new(),
        remaining: max,
    };
    w.raw(MAGIC)?;
    w.number(match format_version {
        1 => 1,
        2 => 2,
        3 => 3,
        _ => return Err(FileError::UnsupportedVersion),
    })?;
    w.number(format_version)?;
    w.raw(&digest)?;
    for s in [
        m.name,
        m.release,
        m.revision,
        m.producer,
        m.scope,
        m.limitations,
    ] {
        w.text(s)?;
    }
    let mut sources = m.sources.iter().collect::<Vec<_>>();
    sources.sort_unstable_by_key(|s| s.name);
    w.count(sources.len())?;
    for s in sources {
        for t in [s.name, s.location, s.revision, s.license, s.notice] {
            w.text(t)?;
        }
        w.raw(&s.sha256)?;
    }
    w.count(catalog.structures.len())?;
    for structure in &catalog.structures {
        structure.write_file(&mut w)?;
    }
    w.count(catalog.bindings.len())?;
    for b in &catalog.bindings {
        for s in [&b.message, &b.event, &b.structure] {
            w.text(s)?;
        }
    }
    fields.write_file(&mut w)?;
    Ok(w.bytes)
}

pub(in crate::schema) struct Writer {
    bytes: Vec<u8>,
    remaining: usize,
}
impl Writer {
    pub(in crate::schema) fn raw(&mut self, bytes: &[u8]) -> Result<(), FileError> {
        self.remaining = self
            .remaining
            .checked_sub(bytes.len())
            .ok_or(FileError::Limit)?;
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
    pub(in crate::schema) fn tag(&mut self, n: u8) -> Result<(), FileError> {
        self.raw(&[n])
    }
    fn number(&mut self, n: u64) -> Result<(), FileError> {
        self.raw(&n.to_be_bytes())
    }
    pub(in crate::schema) fn count(&mut self, n: usize) -> Result<(), FileError> {
        self.number(u64::try_from(n).map_err(|_| FileError::Limit)?)
    }
    pub(in crate::schema) fn text(&mut self, s: &str) -> Result<(), FileError> {
        self.count(s.len())?;
        self.raw(s.as_bytes())
    }
    pub(in crate::schema) fn occurs(&mut self, o: Occurs) -> Result<(), FileError> {
        self.count(o.min)?;
        match o.max {
            None => self.tag(0),
            Some(n) => {
                self.tag(1)?;
                self.count(n)
            }
        }
    }
}
struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
    items: usize,
    limits: LoadLimits,
}
impl<'a> Reader<'a> {
    fn format(&self) -> FileError {
        FileError::Format {
            offset: self.offset,
        }
    }
    fn raw(&mut self, n: usize) -> Result<&'a [u8], FileError> {
        let end = self.offset.checked_add(n).ok_or_else(|| self.format())?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| self.format())?;
        self.offset = end;
        Ok(bytes)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], FileError> {
        self.raw(N)?.try_into().map_err(|_| self.format())
    }
    fn tag(&mut self) -> Result<u8, FileError> {
        Ok(self.array::<1>()?[0])
    }
    fn number(&mut self) -> Result<u64, FileError> {
        Ok(u64::from_be_bytes(self.array()?))
    }
    fn size(&mut self) -> Result<usize, FileError> {
        usize::try_from(self.number()?).map_err(|_| FileError::Limit)
    }
    fn count(&mut self) -> Result<usize, FileError> {
        let n = self.size()?;
        self.items = self.items.checked_sub(n).ok_or(FileError::Limit)?;
        if n > self.bytes.len() - self.offset {
            return Err(self.format());
        }
        Ok(n)
    }
    fn text(&mut self) -> Result<&'a str, FileError> {
        let n = self.size()?;
        if n > self.limits.max_text_bytes {
            return Err(FileError::Limit);
        }
        std::str::from_utf8(self.raw(n)?).map_err(|_| self.format())
    }
    fn texts<const N: usize>(&mut self) -> Result<[&'a str; N], FileError> {
        let mut values = [""; N];
        for value in &mut values {
            *value = self.text()?;
        }
        Ok(values)
    }
    fn occurs(&mut self) -> Result<Occurs, FileError> {
        let min = self.size()?;
        let max = match self.tag()? {
            0 => None,
            1 => Some(self.size()?),
            _ => return Err(self.format()),
        };
        Ok(Occurs { min, max })
    }
    fn group(&mut self, depth: usize) -> Result<Group<'a>, FileError> {
        if depth == 0 {
            return Err(FileError::Limit);
        }
        let mut group = Group {
            nodes: Vec::new(),
            groups: 1,
        };
        for _ in 0..self.count()? {
            let name = self.text()?;
            let occurs = self.occurs()?;
            let content = match self.tag()? {
                0 => NodeContent::Segment(self.array()?, self.text()?),
                1 => {
                    let child = self.group(depth - 1)?;
                    group.groups += child.groups;
                    NodeContent::Group(child)
                }
                _ => return Err(self.format()),
            };
            group.nodes.push(Node {
                name,
                occurs,
                content,
            });
        }
        Ok(group)
    }
    fn conditions(&mut self, profile: &'a str) -> Result<Vec<Condition<'a>>, FileError> {
        (0..self.count()?)
            .map(|_| {
                let target = self.size()?;
                let field = self.size()?;
                let when = match self.tag()? {
                    0 => Predicate::Is {
                        field,
                        state: match self.tag()? {
                            0 => crate::wire::Presence::Missing,
                            1 => crate::wire::Presence::Empty,
                            2 => crate::wire::Presence::Null,
                            3 => crate::wire::Presence::Value,
                            _ => return Err(self.format()),
                        },
                    },
                    1 => Predicate::Equals {
                        field,
                        value: self.text()?,
                    },
                    _ => return Err(self.format()),
                };
                let requirement = match self.tag()? {
                    0 => Requirement::Required,
                    1 => Requirement::Forbidden,
                    _ => return Err(self.format()),
                };
                Ok(Condition {
                    profile,
                    target,
                    when,
                    requirement,
                })
            })
            .collect()
    }
    fn elements(&mut self) -> Result<Vec<ElementData<'a>>, FileError> {
        (0..self.count()?)
            .map(|_| {
                let position = self.size()?;
                let occurs = self.occurs()?;
                let nullable = match self.tag()? {
                    0 => false,
                    1 => true,
                    _ => return Err(self.format()),
                };
                let length = match self.tag()? {
                    0 => None,
                    1 => Some(Length::WireBytes(self.size()?)),
                    2 => Some(Length::ScalarBytes(self.size()?)),
                    3 => Some(Length::Utf8Characters(self.size()?)),
                    _ => return Err(self.format()),
                };
                let (fixed, selector, allowed) = match self.tag()? {
                    0 => (Some(self.text()?), 0, Vec::new()),
                    1 => (
                        None,
                        self.size()?,
                        (0..self.count()?)
                            .map(|_| self.text())
                            .collect::<Result<Vec<_>, _>>()?,
                    ),
                    _ => return Err(self.format()),
                };
                Ok(ElementData {
                    position,
                    occurs,
                    nullable,
                    length,
                    fixed,
                    selector,
                    allowed,
                })
            })
            .collect()
    }
}
struct Node<'a> {
    name: &'a str,
    occurs: Occurs,
    content: NodeContent<'a>,
}
enum NodeContent<'a> {
    Segment([u8; 3], &'a str),
    Group(Group<'a>),
}
struct Group<'a> {
    nodes: Vec<Node<'a>>,
    groups: usize,
}
impl Group<'_> {
    fn project<'a>(&'a self, buffers: &'a mut [Vec<Rule<'a>>]) -> &'a [Rule<'a>] {
        // Exactly one buffer per group was allocated from the bounded parsed tree. Disjoint
        // slices keep child arrays stationary until the existing compiler has copied their plans.
        let (root, mut rest) = buffers.split_at_mut(1);
        for node in &self.nodes {
            let content = match &node.content {
                NodeContent::Segment(id, profile) => Content::TypedSegment { id: *id, profile },
                NodeContent::Group(child) => {
                    let (child_buffers, tail) = rest.split_at_mut(child.groups);
                    rest = tail;
                    Content::Group(child.project(child_buffers))
                }
            };
            root[0].push(Rule {
                name: node.name,
                occurs: node.occurs,
                content,
            });
        }
        &root[0]
    }
}
struct ParsedDatatype<'a> {
    name: &'a str,
    kind: ParsedKind<'a>,
}
enum ParsedKind<'a> {
    Primitive(Primitive),
    Timestamp,
    Composite(Vec<ElementData<'a>>),
    Codes(Vec<&'a str>),
}
struct ElementData<'a> {
    position: usize,
    occurs: Occurs,
    nullable: bool,
    length: Option<Length>,
    fixed: Option<&'a str>,
    selector: usize,
    allowed: Vec<&'a str>,
}
fn project_elements<'a>(elements: &'a [ElementData<'a>]) -> Vec<Element<'a>> {
    elements
        .iter()
        .map(|e| Element {
            position: e.position,
            occurs: e.occurs,
            nullable: e.nullable,
            length: e.length,
            datatype: match e.fixed {
                Some(s) => TypeRef::Fixed(s),
                None => TypeRef::ByField {
                    selector: e.selector,
                    allowed: &e.allowed,
                },
            },
        })
        .collect()
}
