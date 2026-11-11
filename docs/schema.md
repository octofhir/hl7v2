# Caller-supplied schema views

Schema compilation, exact release/structure resolution and segment grouping are separate from
generic wire parsing. This module contains no bundled HL7 catalog or import format. Definitions
are caller-supplied Rust values; their version label does not certify provenance or conformance.

```rust
use octofhir_hl7v2::{schema::{Binding, Catalog, CompileLimits, MatchLimits, Occurs, Rule, Structure}, wire::Message};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let observation = [Rule::segment("obx", *b"OBX", Occurs::ONCE), Rule::segment("notes", *b"NTE", Occurs::MANY)];
    let rules = [Rule::segment("msh", *b"MSH", Occurs::ONCE), Rule::group("observation", &observation, Occurs::MANY)];
    let structures = [Structure { name: "SYNTHETIC", rules: &rules }];
    let bindings = [Binding { message: "ZZZ", event: "Z01", structure: "SYNTHETIC" }];
    let catalog = Catalog::compile("TEST", &structures, &bindings, CompileLimits::default())?;
    let message = Message::parse(b"MSH|^~\\&|S|S|T|T|20261111||ZZZ^Z01^SYNTHETIC|id|T|TEST\rOBX|first\rNTE|note\rOBX|second\r")?;
    let view = catalog.group(&message, MatchLimits::default())?;
    assert_eq!(view.root().segment_range(), 1..5);
    let groups: Vec<_> = view.root().children().skip(1).collect();
    assert_eq!(groups[0].segment_range(), 2..4);
    assert_eq!(groups[1].segment_range(), 4..5);
    assert_eq!(view.message().original().as_ptr(), message.original().as_ptr());
    Ok(())
}
```

## Exact selection

`Catalog` contains one release ID, named structures and explicit `(message code, event, structure)`
bindings. Definition and binding order do not affect lookup. Duplicate identifiers/bindings,
unknown binding targets and invalid cardinalities fail compilation. Message/event codes currently
require three uppercase ASCII letters/digits; structure IDs require uppercase ASCII identifiers.

`resolve` checks scalar MSH-12.1 against the exact catalog release. It then selects a populated
MSH-9.3 only if that structure exists and is bound to MSH-9.1/9.2. There is no nearest release,
default structure or concatenation of message and event. Missing/empty 9.3 permits an event-table
lookup only when its result is unique; explicit null, malformed tokens and conflicting structures
fail. Escapes are decoded once, with diagnostic byte ranges retained.

This initial API rejects populated MSH-12.2/12.3 international/local variants and MSH-21 profiles:
they need explicit supported selection policy before they can affect grammar. Their raw bytes
remain in the message. Repeated MSG/VID fields and extra components also fail. These checks do
not validate processing ID, character sets, ACK policy or the remaining envelope fields.

The MSH component interpretation was checked against the v2.5.1 [control chapter](https://hl7.eu/HL7v2x/v251/std251/ch02.html).
The A08 → ADT_A01 lookup regression was independently checked in the [patient-administration chapter](https://hl7.eu/HL7v2x/v251/std251/ch03.html),
section 3.3.8. The tests' shortened ADT/ORU-like grammars are original synthetic fixtures, not full
definitions of those messages or a redistributable standards catalog.

## Deterministic grouping

Rules describe ordered segments or nested groups, each with minimum and optional maximum
occurrences. Names are stable, unique among siblings and independent of segment IDs.
Role identifiers start with a letter, use ASCII letters/digits/underscore and are limited to 64 bytes.
Fixed-count adjacent occurrences of the same segment ID can have distinct roles. `None` as a maximum means
unbounded cardinality under the runtime work/occurrence limits.

The first rule is exactly one MSH. Compilation computes FIRST/FOLLOW segment sets and refuses
variable occurrences that could consume a following rule's start. Group bodies must consume at
least one required segment. Choices, nullable group bodies and ambiguous grammars are unsupported;
the conservative checks may refuse grammars requiring contextual lookahead/backtracking. Cyclic
or excessive nesting fails within the depth budget. There is no runtime backtracking or heuristic
reassignment.

Matching consumes every source segment in wire order. Unexpected segments, including Z-segments,
are never skipped or attached to an inferred group. Declare an explicit extension rule or handle
the structural error while inspecting the retained original. Missing required segments and trailing
content report source segment positions/ranges. Errors contain metadata rather than payload text.

`View` borrows the message and immutable compiled schema. Its flat occurrence arena references
global one-based half-open segment ranges; `Occurrence::segment()` returns a borrowed source
segment for a leaf. `children()` skips subtrees without allocating per-group child lists. Names
identify schema roles; source occurrence numbers identify actual wire segments. Field presence,
repetitions, unknown bytes, precision and source spans stay available through the wire API.
`StructureSchema::group` intentionally matches only segment shape; `Catalog::group` also resolves
the envelope. Neither is field/datatype, conditional-rule or terminology validation.

## Bounds, caching and remaining gates

Default compilation limits are 8,192 total rules across a catalog, depth 32 and 1,000,000 work
units for definition/set processing. Depth has an additional hard ceiling of 64. Structure/binding
counts are also capped by `max_rules`. Defaults for one match are 262,144 occurrence nodes and
1,000,000 rule steps, including unsuccessful optional/repetition probes. Metadata container
capacity and the existing message index are additional memory. Compilation work units are not
a wall-clock deadline. Compile once and reuse catalogs; matching creates only occurrence metadata
and bounded decoded header tokens, with no JSON projection or payload tree copy.

Five integration scenarios cover selection failures, shared event structures, nested/repeated
groups, fixed same-ID roles, unknown/Z content, cardinality/budget failures, nullable/cyclic
definitions and FIRST/FOLLOW ambiguity. A bounded-language check covers all 364 sequences of
length 0–5 over three segment IDs. A separate schema fuzz target checks generated valid instances,
arbitrary flat definitions/cardinalities, source coverage and bounded matching failures.

`HL7V2_BENCH_ITERS=100000 cargo bench --locked --offline --bench schema` separately times catalog
compilation, cached-source resolution/grouping, and parse+resolve+group. Synthetic 1- and 1,000-patient
messages have fixed nested order/observation/note groups; the large case uses 1/1,000 of the
configured iterations. Preflight checks source identity, group/segment coverage and occurrence count.
There is no field/terminology/FHIR validation, allocation count or durability in these measurements.
They are not comparisons with other implementations or production throughput guarantees.

No package loader, deterministic artifact digest, overlay compiler or approved versioned schema
data ships here. The host must bind any cached catalog to its immutable processing assets; a
release label alone is insufficient for durable replay. Field definitions/validation, dynamic
OBX-5 typing, profile registration and logical FML models remain separate stages. The acquisition,
rights/provenance and completeness gates in the [implementation plan](implementation-plan.md)
remain open. This module establishes a reusable structural engine, not an HL7 conformance claim.
