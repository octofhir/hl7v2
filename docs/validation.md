# Caller-defined field validation

Field/datatype definitions and explicit grammar profile references are separate from generic parsing.
No standard field catalog is bundled. Numeric and temporal checks reuse the exact wire scalar views.

```rust
use octofhir_hl7v2::{
    schema::{Binding, Catalog, CompileLimits, MatchLimits, Occurs, Rule, Structure,
        fields::{Datatype, DatatypeKind as K, Element as E, FieldSchema, Primitive as P, SegmentDefinition as S, ValidationLimits}},
    wire::Message,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let types = [Datatype { name: "ID", kind: K::Primitive(P::Ascii) },
        Datatype { name: "NM", kind: K::Primitive(P::Number) },
        Datatype { name: "MSG", kind: K::Composite(&[E::required(1, "ID"), E::required(2, "ID"), E::required(3, "ID")]) }];
    let profiles = [
        S { name: "Header", id: *b"MSH", fields: &[E::required(1, "ID"), E::required(2, "ID"), E::required(9, "MSG"), E::required(12, "ID")] },
        S { name: "Observation", id: *b"OBX", fields: &[E::optional(1, "ID"), E::optional(2, "ID"),
            E::optional(3, "ID"), E::optional(4, "ID"), E::by_field(5, 2, &["NM"], Occurs::MANY)] },
    ];
    let rules = [Rule::typed_segment("msh", *b"MSH", "Header", Occurs::ONCE),
        Rule::typed_segment("obx", *b"OBX", "Observation", Occurs::MANY)];
    let structures = [Structure { name: "TEST", rules: &rules }];
    let bindings = [Binding { message: "ZZZ", event: "Z01", structure: "TEST" }];
    let catalog = Catalog::compile("TEST", &structures, &bindings, CompileLimits::default())?;
    let fields = FieldSchema::compile(&types, &profiles, CompileLimits::default())?;
    fields.check_catalog(&catalog)?;
    let source = Message::parse(b"MSH|^~\\&|||||||ZZZ^Z01^TEST|||TEST\rOBX|1|NM|code||0001.2300\r")?;
    let view = catalog.group(&source, MatchLimits::default())?;
    let report = fields.validate(&view, ValidationLimits::default());
    assert!(report.is_valid());
    assert_eq!(fields.datatype(&source, "Observation", &[2, 5]).map_err(|_| "unresolved datatype")?, "NM");
    assert_eq!(source.value(&[2, 5]).decimal()?.as_bytes(), b"0001.2300");
    Ok(())
}
```

This is an original synthetic profile with release label `TEST`, not the mandatory fields of a
standard HL7 message. `is_valid()` means the selected constraints completed without reported or
omitted issues. It does not establish general version conformance, terminology validity or FHIR validity.

## Definitions and activation

`DatatypeKind` supports primitive decoded bytes, ASCII, UTF-8, exact decimal syntax, DT, DTM,
composites and legacy TS. Names do not imply standard rules: registering `NM` does not add a
16-character limit unless the caller declares it. Formatted/local/charset escapes unsupported by
the wire decoder produce issues; this engine does not claim complete FT/TX or charset support.

Every segment grammar role uses an explicit `Rule::typed_segment` profile reference. Two roles
with the same segment ID may select different profiles. `check_catalog` verifies that every
grammar leaf has a matching definition and ID before activation. Plain `Rule::segment` remains
available for shape-only grouping; field validation reports it as unmodeled. Missing/mismatched
profiles also fail at runtime if activation checking was omitted. There is no global fallback by
segment ID. `View::segments()` enumerates source leaves with their roles/profiles.

Compilation sorts position/name indexes and resolves every datatype reference and selector.
Duplicate/zero/out-of-budget positions, cycles, excessive nesting, invalid cardinalities, missing
references and invalid selector definitions fail. Components cannot repeat. Composite depth is
limited to the two available wire levels; TS occupies one level. Dynamic selectors are field-only,
must reference a declared, non-repeating byte/text field in the same segment, and carry a nonempty
explicit allowlist of registered types. Compilation uses the existing rule/work/depth budgets;
allowed-type entries also consume that budget. Definitions remain immutable and reusable.

## Presence, repetitions and lengths

`Element::required` means minimum one, maximum one; `optional` means zero or one. Missing/empty
fields count as zero occurrences and fail when the minimum is positive. Populated fields count
physical repetition slots, including interior/trailing empties, for cardinality. Each required
repetition/component must be nonempty. Explicit null is accepted only with `nullable=true`; it
then short-circuits datatype/child/length checks as a deliberate clear. Cardinality still applies;
a null can satisfy minimum one, but does not invent additional repetitions for a higher minimum.
Zero maximum forbids populated/null content but permits empty positional padding. These are
explicit profile policies rather than a universal interpretation of interface update semantics.

Primitive values are decoded once per check. Escaped literal quotes stay populated values, not
null. Optional empty parents do not invent required children. Numeric spelling/scale, temporal
precision and offset presence are never normalized. MSH-1/2 remain opaque scalar declarations.
Possible terminal truncation markers fail scalar decoding under the conservative [wire policy](wire.md).

Length units are explicit: `WireBytes` counts raw bytes of one repetition/component, including
escape spellings and composite separators; `ScalarBytes` counts decoded scalar bytes;
`Utf8Characters` counts Unicode scalar values after decoding and checking UTF-8. The latter two
are primitive-only, including for dynamic allowlists. They are not an implicit implementation of
every version's standard length rules. No character-set transcoding or grapheme counting occurs.

`TypeRef::ByField` resolves from the current source segment. It never reuses another OBX's type,
normalizes a code or substitutes ST. Unknown/disallowed types, absent/empty/null selectors and
malformed/repeated selectors produce separate source diagnostics. Selectors are resolved once per
field at its first populated repetition; an optional empty field or allowed null needs no datatype.
Conditional OBX-2 requirements based on OBX-11 are not implemented by this generic mechanism.
The same-segment OBX-2/5 association was checked against the v2.5.1 [observation chapter](https://hl7.eu/HL7v2x/v251/std251/ch07.html),
sections 7.4.2.2 and 7.4.2.5. Tests use original synthetic subsets, not a copied standards catalog.

## Reports and bounds

Issues carry wire paths, original byte spans and the observed presence state, never payload text.
Missing positions use a zero-length anchor at the nearest present parent/segment end. Lexical
errors in decoded scalars cover the original selected span; decoder and TS precision errors keep
their narrower source spans. Selector issues point to the selector and identify the target field.
Populated/null unmodeled fields/components and untyped segments are reported rather than dropped;
empty padding is retained without inventing an unsupported value.

`ValidationLimits` defaults to 1,000,000 work steps, 32 MiB cumulative raw value/selector bytes
visited, and 1,024 retained issues. `Report::steps` and `Report::bytes` expose charged work.
Nested scopes may charge the same bytes again. Raw lengths are
charged before decoding; selector decoding is additionally capped at 320 raw bytes. Metadata and
repetition probes consume steps, including empty slots. These are work/allocation bounds, not a
wall-clock timeout or total process-memory measurement.

After the issue cap, validation continues and increments `omitted`. Work/byte exhaustion emits
`Limit`, sets `complete=false` and stops. Neither a zero issue cap nor an incomplete check can
produce `is_valid()=true`. Compile failures are separate from runtime reports. `validate_segment`
checks one profile only; callers needing envelope/structure checks must also use `Catalog::group`.
Terminology, conditional rules, approved release packages/overlays, FML loss reporting and R4
validation remain separate stages. Original source bytes/indexes are never mutated or projected to JSON.
