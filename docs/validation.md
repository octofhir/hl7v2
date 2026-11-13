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
composites, legacy TS and closed UTF-8 code sets. Names do not imply standard rules: registering `NM` does not add a
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
Conditional requirements are separate from dynamic datatype resolution; the limited API below can add same-segment presence constraints.
The same-segment OBX-2/5 association was checked against the v2.5.1 [observation chapter](https://hl7.eu/HL7v2x/v251/std251/ch07.html),
sections 7.4.2.2 and 7.4.2.5. Tests use original synthetic subsets, not a copied standards catalog.

## Closed code sets

`DatatypeKind::Codes(&[&str])` defines a closed set of exact decoded UTF-8 values. Compilation
sorts and retains the entries once; validation uses binary search. These types can be fields,
components/subcomponents or same-segment dynamic selectors, and support explicit scalar length
units. Matching is byte-exact after one scalar decode: no trim, case folding, Unicode normalization,
numeric coercion or fallback occurs. A code containing a separator must be escaped on the wire.

```rust
use octofhir_hl7v2::{
    schema::{CompileLimits, fields::{Datatype, DatatypeKind, Element, FieldSchema,
        SegmentDefinition, ValidationLimits}},
    wire::Message,
};
let types = [Datatype { name: "STATUS", kind: DatatypeKind::Codes(&["F", "P", "C"]) }];
let profiles = [SegmentDefinition { name: "Local", id: *b"Z99",
    fields: &[Element::required(1, "STATUS")] }];
let schema = FieldSchema::compile(&types, &profiles, CompileLimits::default())?;
let source = Message::parse(b"MSH|^~\\&\rZ99|F\r")?;
assert!(schema.validate_segment(&source, "Local", 2, ValidationLimits::default()).is_valid());
# Ok::<(), Box<dyn std::error::Error>>(())
```

Sets must be nonempty, with unique nonempty entries, at most 4,096 UTF-8 bytes per entry and no
characters accepted by Rust's `char::is_control`. Spaces are retained and can be explicit codes.
Entries consume the compile rule budget; entry counts and cumulative code bytes consume compile
work before validation/sorting. Together with the entry-size cap these bound compilation/sorting
and retained metadata. Code order does not affect compiled identity. Duplicate entries fail rather
than being silently deduplicated. No published standard tables are bundled.

`IssueKind::Code` reports a populated decoded value outside its set, with the original span/path
and no value text. Invalid UTF-8 reports `Encoding`; malformed/unsupported escapes and possible
terminal truncation markers preserve the existing wire errors. Presence and cardinality policies
run first: required missing/empty values fail, nullable explicit null short-circuits membership,
and escaped literal quote bytes are ordinary values that need an explicit matching entry. Every
populated repetition is checked independently.

Each code-search probe consumes a step and charges twice the shorter compared byte length before
reading the two prefixes. These comparison bytes join the existing cumulative value/selector
byte budget. Exhausting either budget makes the report incomplete; omitted code issues also
prevent validation success. Definitions and lookup plans are cached across messages.

These are caller-authored finite constraints, not a terminology service: code-system/version
resolution, coded-pair correlation, subsumption, translations, standard table acquisition and
conditional terminology remain separate work. Record the set's scope/provenance in package metadata.
[Native packages](schema-packages.md) use V2 for code sets while retaining exact V1 identities for
legacy definitions; default field validation still needs no dependencies.

## Conditional presence

`FieldSchema::compile_with_conditions` adds caller-authored `Condition` definitions to named
segment profiles. `Package::compile_with_conditions` includes the same rules in persisted identity.
Each rule observes one declared field in the current source segment and requires or forbids a
different declared optional field. Existing cardinality, datatype, nullability and length checks
always apply. There are no writes, evaluation-order effects or iterative resolution.

```rust
use octofhir_hl7v2::{
    schema::{CompileLimits, fields::{Condition, Datatype, DatatypeKind, Element,
        FieldSchema, Predicate, Primitive, Requirement, SegmentDefinition, ValidationLimits}},
    wire::Message,
};
let types = [Datatype { name: "ID", kind: DatatypeKind::Primitive(Primitive::Ascii) }];
let profiles = [SegmentDefinition { name: "Local", id: *b"Z99",
    fields: &[Element::required(1, "ID"), Element::optional(2, "ID")] }];
let conditions = [Condition { profile: "Local", target: 2,
    when: Predicate::Equals { field: 1, value: "F" }, requirement: Requirement::Required }];
let schema = FieldSchema::compile_with_conditions(&types, &profiles, &conditions, CompileLimits::default())?;
let source = Message::parse(b"MSH|^~\\&\rZ99|F\r")?;
let report = schema.validate_segment(&source, "Local", 2, ValidationLimits::default());
assert!(!report.is_valid());
assert_eq!(&report.issues[0].path[..2], &[2, 2]);
# Ok::<(), Box<dyn std::error::Error>>(())
```

`Predicate::Is` compares the field's raw top-level `Missing`, `Empty`, `Null` or `Value` state,
including on composite/repeating fields. `Predicate::Equals` compares one decoded scalar with an
exact UTF-8 literal. Missing, empty and explicit null never equal a literal; escaped quote bytes
remain data. No trim, numeric coercion, case folding or Unicode normalization occurs. Equality
selectors must be statically typed, non-repeating primitive/code fields. Literals must satisfy that
datatype, be nonempty/control-free and at most 4,096 UTF-8 bytes. Composite, TS and dynamic equality
selectors are unsupported and rejected at activation.

An active `Required` rule rejects missing/empty targets and every empty physical repetition.
Explicit null satisfies it only when the target's existing policy permits null. `Forbidden` rejects
populated values and explicit null, allowing absent fields and empty positional padding. Targets
must have static minimum zero; a required action cannot target static maximum zero. Unknown
profiles/positions, self references, duplicate premises and opposite actions with identical premises
fail compilation. Other rules are additive: overlapping different premises can make a message
invalid; activation does not solve arbitrary logical contradictions.

Rules consume compile rule/work budgets; literal bytes also consume work. Runtime probes consume
steps, raw equality-selector bytes are charged before decoding, and comparisons charge twice the
shorter byte length. Equality decoding is capped at 20,480 raw bytes (five times the literal cap,
allowing escaped bytes). A malformed/repeated/composite or oversized equality selector produces
`ConditionSelector { target }`; decoder failures preserve wire issues. Such selectors never silently
pass validation. Active target failures produce `Conditional { selector, requirement }` at the
target's original path/span/presence. Report caps and exhaustion retain the same fail-closed policy.
Plans and owned literal metadata are compiled once and reused across messages.

This subset adds no expression language, cross-segment/group queries, code-system lookup,
conditional datatype overrides or standard rule catalog. The synthetic example defines a local
policy and makes no claim about normative HL7 conditions. [Package V3](schema-packages.md) persists
these rules while unchanged V1/V2 definitions keep their historical pins and bytes.

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
charged before decoding; dynamic datatype selector decoding is additionally capped at 320 raw bytes. Metadata and
repetition probes consume steps, including empty slots. These are work/allocation bounds, not a
wall-clock timeout or total process-memory measurement.

After the issue cap, validation continues and increments `omitted`. Work/byte exhaustion emits
`Limit`, sets `complete=false` and stops. Neither a zero issue cap nor an incomplete check can
produce `is_valid()=true`. Compile failures are separate from runtime reports. `validate_segment`
checks one profile only; callers needing envelope/structure checks must also use `Catalog::group`.
External terminology, broader conditional rules, approved release packages/overlays, FML loss reporting and R4
validation remain separate stages. Original source bytes/indexes are never mutated or projected to JSON.
