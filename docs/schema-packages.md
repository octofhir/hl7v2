# Content-bound schema packages

The optional `schema-package` feature adds `schema::package::Package` and uses SHA-256 from the
existing optional `sha2` dependency. Wire parsing, grouping and field checks remain dependency-free
without this feature. Packages combine native Rust definitions with a manifest, compile both schema
layers and check every grammar/profile binding before exposing an immutable package.

This is an in-process package identity API, not a file/archive loader or an approved standard
catalog. It does not copy third-party schema data. The host retains the definitions and their
source evidence and can reconstruct and verify the exact package after restart.

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

`FORMAT_VERSION=1` identifies this encoding and the schema-validation semantics revision; it is
distinct from HL7 release and manifest package revision. Changes to encoding or schema semantics
must revise this constant before shipping. Retain the relevant library revision in the processing
plan as well. This identity does not pin wire parsing, mapping or clinical application behavior.

SHA-256 covers a domain tag, format version, all manifest text/source digests, all compiled grammar
roles/order/cardinalities/profile bindings, exact event associations, datatype kinds/components,
dynamic selectors/allowlists, field positions/cardinalities/nullability and explicit length units.
Unused registered definitions are also included. Strings/byte arrays have u64 big-endian length
prefixes; counts/positions are u64 big-endian, and options/variants use explicit byte tags. Lengths
and cardinalities must be representable on the target platform. There is no concatenation ambiguity,
native-endian encoding, floating point, locale formatting or randomized map iteration.

Structures, bindings, datatypes, profiles, field/component positions, source names and dynamic
allowlists use their compiled sorted order. Reordering these input tables preserves identity;
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
files or their terms. It has no signatures, package registry, external import, overlays, terminology
or conditional-rule support. Those gates remain explicit in the [implementation plan](implementation-plan.md).
