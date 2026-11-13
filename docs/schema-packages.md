# Content-bound schema packages

The optional `schema-package` feature adds `schema::package::Package` and uses SHA-256 from the
existing optional `sha2` dependency. Wire parsing, grouping and field checks remain dependency-free
without this feature. Packages combine native Rust definitions with a manifest, compile both schema
layers and check every grammar/profile binding before exposing an immutable package.

Native packages can be exported and loaded through bounded versioned binary formats. This is our own
Rust-definition persistence format, not an importer for official HL7 artifacts or an approved
standard catalog. It does not copy third-party schema data. The host retains historical files,
their expected identities and source evidence for reconstruction after restart.

## Activation and replay

Construct `Definitions` from the same structures, event bindings, datatypes and segment profiles
used with `Catalog::compile` and `FieldSchema::compile`. A `Manifest` identifies the package name,
exact HL7 release, package revision, producer revision, explicit scope, limitations and source
evidence. Every source supplies a unique name, location, revision, SHA-256, license and notice.
For original synthetic definitions, state their synthetic coverage and original authorship; do not
label a partial profile as a full standard catalog.

```rust,no_run
use octofhir_hl7v2::schema::{CompileLimits,
    package::{Definitions, Manifest, Package}};

fn restore<'a>(manifest: Manifest<'a>, definitions: Definitions<'a>, expected: [u8; 32])
    -> Result<Package<'a>, Box<dyn std::error::Error>>
{
    Ok(Package::compile(manifest, definitions, CompileLimits::default())?
        .require_digest(expected)?)
}
```

Compile/cache packages at activation, then use `catalog()` and `fields()` for messages. `digest()`
returns the exact 32-byte package identity. Historical activation uses `require_digest(saved)`;
a changed definition, release, revision or provenance fails with `DigestMismatch`. Never substitute
the newest package when historical content is unavailable. An existing cached package remains
immutable while the host activates another revision for new receipts.

The [inbox plan](inbox.md) is the digest of the **whole processing manifest**, not this schema digest.
Include the package digest in that manifest alongside maps, terminology, engine versions, wire
options, runtime validation limits and application policy. This API does not construct or rewrite
inbox plans, store assets, resolve history or execute a replay. The host must retain all historical
assets; hashing alone does not make them available.

## Identity contract

`FORMAT_VERSION=3` is the latest supported identity/validation-semantics revision, distinct from
HL7 release and manifest package revision. `Package::format_version()` and
`OwnedPackage::format_version()` expose the version selected for each compiled package. Legacy
primitive/composite/TS definitions retain V1 and their exact historical digest/export bytes;
registering any `Codes` datatype selects V2, including unused registered types. Any conditional
presence rule selects V3. Rules are supplied separately through `compile_with_conditions`, preserving
the existing `Definitions` and profile structs. No implicit
historical upgrade occurs. New encoding/schema-semantics changes must revise the latest version
before shipping and retain supported historical behavior. Retain the relevant library revision in the processing
plan as well. This identity does not pin wire parsing, mapping or clinical application behavior.

SHA-256 covers a domain tag, format version, all manifest text/source digests, all compiled grammar
roles/order/cardinalities/profile bindings, exact event associations, datatype kinds/components,
dynamic selectors/allowlists, field positions/cardinalities/nullability and explicit length units.
V2 adds datatype tag 3 followed by a count and sorted length-prefixed UTF-8 code entries; all
legacy type tags and layouts remain unchanged. V1 never permits that tag. V3 adds per-profile
conditional lists, including zero counts for profiles with no rules, using the file encoding below.
Unused registered definitions are also included. Strings/byte arrays have u64 big-endian length
prefixes; counts/positions are u64 big-endian, and options/variants use explicit byte tags. Lengths
and cardinalities must be representable on the target platform. There is no concatenation ambiguity,
native-endian encoding, floating point, locale formatting or randomized map iteration.

Structures, bindings, datatypes, profiles, field/component positions, source names and dynamic
allowlists, code sets and conditional rules use their compiled sorted order. Reordering these input tables preserves identity;
ordered grammar siblings and group associations remain significant. FIRST/FOLLOW sets, capacities,
compiler work budgets and other derived metadata are excluded. Compile limits govern admission,
not schema semantics; runtime limits belong in the processing plan. Hashing streams compiled
metadata directly and does not serialize messages or build a JSON projection.

Metadata is bounded before compilation/sorting/hashing: 1–32 sources, at most 4,096 UTF-8 bytes per
text value and at most 128 KiB total manifest/source text. Empty/whitespace-only text, NULs and
duplicate source names fail. Scope and limitations are mandatory prose declarations; they are not
a machine-verified completeness matrix. Compilation budgets apply separately to the structural
and field compilers. Existing grammar and datatype restrictions still apply.

A digest establishes content identity against an expected value, not authenticity, legal rights or
standard conformance. Source digests are declared evidence; this API does not fetch/verify source
files or their terms. It has no signatures, package registry, standards-artifact import, overlays, terminology
or broader conditional-rule support. Those gates remain explicit in the [implementation plan](implementation-plan.md).

## File persistence and ownership

`Package::encode(max_bytes)` exports a canonical native package. `write_to(writer, max_bytes)`
checks the complete encoded size before writing; an I/O failure can still leave partial output.
The host owns file creation, durable/atomic publication and historical retention. The library does
not open paths from the manifest, fetch URLs, resolve includes or decompress archives.

`OwnedPackage::decode(bytes, expected, limits)` and `read_from(reader, expected, limits)` require
an externally retained expected schema digest. The embedded digest must match it, and the package
is recompiled through the existing grammar/field compilers and hashed before activation. A file
cannot authorize its own changed content by replacing its embedded digest. Unknown versions/tags,
truncated input, trailing bytes, invalid UTF-8, missing/duplicate references, invalid constraints
and unsupported schema constructs fail. Nonsemantic table reordering is accepted and converges to
the same canonical export; changing hashed semantics fails the old pin.

```rust,no_run
use std::{fs::File, path::Path};
use octofhir_hl7v2::schema::package::{LoadLimits, OwnedPackage};

fn restore_file(path: &Path, saved_schema_digest: [u8; 32])
    -> Result<OwnedPackage, Box<dyn std::error::Error>>
{
    Ok(OwnedPackage::read_from(File::open(path)?, saved_schema_digest, LoadLimits::default())?)
}
```

The loaded package owns its manifest and compiled metadata. `manifest()` exposes an immutable
`OwnedManifest`/`OwnedSource`; `catalog()`, `fields()` and `digest()` have the same meaning as on a
native package. Parsing borrows input text temporarily, constructs bounded definition projections,
compiles once and moves the resulting plans into owned storage. Only metadata strings are copied
when promoting the plans; no self references, leaked allocations or per-message recompilation
are needed. Input buffers/readers may be dropped immediately. Cache this object at activation and
reuse its existing grouping/validation APIs for messages.

`LoadLimits` defaults to 8 MiB input, 65,536 cumulative entries, 4,096 bytes per text value and the
existing `CompileLimits`. Every list charges its count before allocation, including source evidence,
grammar nodes, fields/components and dynamic allowlists. Grammar parsing uses the compile depth
limit with the existing hard ceiling of 64. Counts must fit the target's `usize`; oversized lengths
are checked before slicing. `read_from` reads at most `max_bytes + 1` bytes, including from a growing
or endless reader. Byte/entry/depth limits bound parsing and temporary projections; compiler work
budgets then bound semantic compilation separately. Container capacity and owned metadata copies
are additional memory; these limits are not a wall-clock timeout or process RSS cap.

The optional [`hl7v2-validate` binary](offline-validation.md) provides an offline file-to-report
scenario using these APIs. It activates one exact package and reuses it across message files.

## Binary files V1, V2 and V3

`FILE_VERSION=3` is the latest file grammar, versioned independently of schema identity semantics.
Supported `(file version, identity version)` pairs are `(1, 1)`, `(2, 2)` and `(3, 3)`. Canonical export
selects V1 for legacy definitions, V2 for code sets and V3 for conditional presence. V3 without
conditions, V2 without code sets,
V1 with a code-set tag, mixed pairs and unknown versions are rejected. Existing V1 bytes and
schema digests remain unchanged for both V1 and V2.
All integers are unsigned big-endian u64; lengths/counts/positions must also fit `usize`.
`text` is a byte length followed by that many UTF-8 bytes. A `list<T>` is a count followed by T items.
Raw fixed-size bytes have no length prefix. No padding, optional extensions or trailing bytes exist.

The file contains these records in order:

| Record | Encoding |
|---|---|
| Header | Raw eight-byte `OH7PKG\r\n`, file version, identity version, raw 32-byte schema digest |
| Manifest | Six texts: name, release, revision, producer, scope, limitations |
| Evidence | List of five texts (name, location, revision, license, notice), then raw 32-byte source digest per item |
| Structures | List of structure name text and a rule list per item |
| Bindings | List of three texts: message code, event code, structure name |
| Datatypes | List of name text and kind per item |
| Profiles | List of name text, raw three-byte segment ID and element list per item; V3 appends a condition list |

An occurrence is minimum, then byte tag 0 for unbounded maximum or tag 1 followed by maximum.
A rule is name text, occurrence and byte tag 0 followed by raw segment ID/profile-name text,
or tag 1 followed by a nested rule list. The structure root is implicit and occurs exactly once.
Only typed leaves can belong to a complete package, as in native activation.

A datatype kind is byte tag 0 plus a primitive byte (0 bytes, 1 ASCII, 2 UTF-8, 3 number, 4 DT,
5 DTM), tag 1 for TS, or tag 2 followed by a composite element list. In V2/V3, tag 3 followed
by a list of code texts defines a closed UTF-8 code set; its entries use the same text encoding
and must satisfy the code compilation bounds.

An element is position, occurrence, nullable byte (0/1), length constraint and datatype reference.
Length uses tag 0 for none, or tags 1/2/3 plus a maximum for wire bytes/scalar bytes/UTF-8 characters.
A reference uses tag 0 plus fixed datatype-name text, or tag 1 plus selector position and a list of
allowed datatype-name texts. Every other tag is rejected; names do not introduce implicit rules.

A V3 condition is target position, selector position, predicate and requirement. Predicate byte 0
is followed by a presence byte (0 missing, 1 empty, 2 explicit null, 3 value); predicate byte 1 is
followed by a literal text. Requirement byte 0 means required and 1 means forbidden. Conditions
sort by target, selector, predicate tag/value and requirement. Unknown tags and unsupported
references/actions fail activation; every condition charges the cumulative file item budget.

Exports sort evidence by name and use compiled sorted structures/bindings/datatypes/profiles,
positions, allowlists and code entries. Grammar sibling order remains significant. All registered definitions,
including unused ones, are retained. Importing this format establishes no broader conditional,
external terminology or standards coverage.

The frozen synthetic `tests/fixtures/native-package-v1.bin` was independently encoded from this
contract with Python `struct`. It restores the historical V1 pin checked before file support.
Regression checks compare native/exported/loaded behavior, full-input truncations and corruption,
unsupported versions/tags, references and resource bounds. The separate `package` fuzz target
exercises arbitrary files under bounded parsing/compilation and exact-pin reconstruction.

The separately frozen `tests/fixtures/native-package-v2.bin` has independently encoded V2
identity/file bytes from Python `struct`/`hashlib`. It binds a closed UTF-8 set with a scalar length
constraint. Tests verify its exact pin, native/owned validation parity, code-order invariance,
changed-code refusal, version/content agreement and code-list/truncation bounds. The V1 fixture
continues to load and export byte-identically. The `package` fuzz target covers both pinned formats.

The independent Python-encoded `tests/fixtures/native-package-v3.bin` binds two exact-literal
presence rules. Regression checks pin its native/owned digest and canonical bytes, exercise
presence-predicate persistence and reject changed actions, unsupported tags/counts, downgrades
and every truncated prefix. Existing V1/V2 fixtures retain exact bytes and pins. The `package`
fuzz target covers all three pinned formats.
