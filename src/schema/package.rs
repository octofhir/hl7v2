#![doc = include_str!("../../docs/schema-packages.md")]
use super::{
    Binding, Catalog, CompileLimits, Occurs, Structure,
    fields::{Datatype, FieldSchema, SegmentDefinition},
};
use sha2::{Digest, Sha256};
use std::fmt;

/// Identity encoding and validation-semantics revision, independent of HL7 release/package revision.
pub const FORMAT_VERSION: u64 = 1;
pub type Fingerprint = [u8; 32];

/// Declared source evidence. The host verifies and retains the referenced artifact itself.
#[derive(Clone, Copy)]
pub struct Source<'a> {
    pub name: &'a str,
    pub location: &'a str,
    pub revision: &'a str,
    pub sha256: Fingerprint,
    pub license: &'a str,
    pub notice: &'a str,
}
#[derive(Clone, Copy)]
pub struct Manifest<'a> {
    pub name: &'a str,
    pub release: &'a str,
    pub revision: &'a str,
    /// Generator/authoring-tool identity, including its revision.
    pub producer: &'a str,
    /// Explicit coverage of structures, fields, datatypes and event associations.
    pub scope: &'a str,
    /// Unsupported/unknown constraints, including conditional and terminology checks.
    pub limitations: &'a str,
    pub sources: &'a [Source<'a>],
}
pub struct Definitions<'a> {
    pub structures: &'a [Structure<'a>],
    pub bindings: &'a [Binding<'a>],
    pub datatypes: &'a [Datatype<'a>],
    pub segments: &'a [SegmentDefinition<'a>],
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Manifest,
    Limit,
    Schema(super::Error),
    DigestMismatch,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Schema(error) => error.fmt(f),
            _ => write!(f, "HL7 schema package {self:?}"),
        }
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Schema(error) => Some(error),
            _ => None,
        }
    }
}
impl From<super::Error> for Error {
    fn from(error: super::Error) -> Self {
        Self::Schema(error)
    }
}

/// Immutable compiled definitions and their exact, content-bound identity.
pub struct Package<'a> {
    manifest: Manifest<'a>,
    catalog: Catalog<'a>,
    fields: FieldSchema<'a>,
    digest: Fingerprint,
}
impl<'a> Package<'a> {
    pub fn compile(
        manifest: Manifest<'a>,
        definitions: Definitions<'a>,
        limits: CompileLimits,
    ) -> Result<Self, Error> {
        let sources = check_manifest(&manifest)?;
        let catalog = Catalog::compile(
            manifest.release,
            definitions.structures,
            definitions.bindings,
            limits,
        )?;
        let fields = FieldSchema::compile(definitions.datatypes, definitions.segments, limits)?;
        fields.check_catalog(&catalog)?;
        let mut hash = Identity(Sha256::new());
        hash.bytes(b"octofhir-hl7v2-schema-package");
        hash.0.update(FORMAT_VERSION.to_be_bytes());
        for value in [
            manifest.name,
            manifest.release,
            manifest.revision,
            manifest.producer,
            manifest.scope,
            manifest.limitations,
        ] {
            hash.text(value);
        }
        hash.number(sources.len());
        for source in sources {
            for value in [
                source.name,
                source.location,
                source.revision,
                source.license,
                source.notice,
            ] {
                hash.text(value);
            }
            hash.bytes(&source.sha256);
        }
        hash.number(catalog.structures.len());
        for structure in &catalog.structures {
            structure.identity(&mut hash);
        }
        hash.number(catalog.bindings.len());
        for binding in &catalog.bindings {
            for value in [binding.message, binding.event, binding.structure] {
                hash.text(value);
            }
        }
        fields.identity(&mut hash);
        Ok(Self {
            manifest,
            catalog,
            fields,
            digest: hash.0.finalize().into(),
        })
    }
    pub fn manifest(&self) -> &Manifest<'a> {
        &self.manifest
    }
    pub fn catalog(&self) -> &Catalog<'a> {
        &self.catalog
    }
    pub fn fields(&self) -> &FieldSchema<'a> {
        &self.fields
    }
    pub fn digest(&self) -> Fingerprint {
        self.digest
    }
    /// Verify an expected historical identity; never substitute a newer revision.
    pub fn require_digest(self, expected: Fingerprint) -> Result<Self, Error> {
        if self.digest == expected {
            Ok(self)
        } else {
            Err(Error::DigestMismatch)
        }
    }
}

fn check_manifest<'a>(manifest: &Manifest<'a>) -> Result<Vec<&'a Source<'a>>, Error> {
    if manifest.sources.is_empty() {
        return Err(Error::Manifest);
    }
    if manifest.sources.len() > 32 {
        return Err(Error::Limit);
    }
    let mut budget = 128 * 1024usize;
    let mut text = |value: &str| {
        if value.len() > 4096 {
            return Err(Error::Limit);
        }
        if value.trim().is_empty() || value.contains('\0') {
            return Err(Error::Manifest);
        }
        budget = budget.checked_sub(value.len()).ok_or(Error::Limit)?;
        Ok(())
    };
    for value in [
        manifest.name,
        manifest.release,
        manifest.revision,
        manifest.producer,
        manifest.scope,
        manifest.limitations,
    ] {
        text(value)?;
    }
    for source in manifest.sources {
        for value in [
            source.name,
            source.location,
            source.revision,
            source.license,
            source.notice,
        ] {
            text(value)?;
        }
    }
    let mut sources: Vec<_> = manifest.sources.iter().collect();
    sources.sort_unstable_by_key(|s| s.name);
    if sources.windows(2).any(|p| p[0].name == p[1].name) {
        return Err(Error::Manifest);
    }
    Ok(sources)
}

/// V1 encoding: length-prefixed bytes, big-endian u64 counts, explicit option/variant tags.
pub(super) struct Identity(Sha256);
impl Identity {
    pub(super) fn tag(&mut self, tag: u8) {
        self.0.update([tag]);
    }
    pub(super) fn number(&mut self, number: usize) {
        self.0.update((number as u64).to_be_bytes());
    }
    pub(super) fn bytes(&mut self, bytes: &[u8]) {
        self.number(bytes.len());
        self.0.update(bytes);
    }
    pub(super) fn text(&mut self, value: &str) {
        self.bytes(value.as_bytes());
    }
    pub(super) fn occurs(&mut self, occurs: Occurs) {
        self.number(occurs.min);
        match occurs.max {
            Some(max) => {
                self.tag(1);
                self.number(max);
            }
            None => self.tag(0),
        }
    }
}
