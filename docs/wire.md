# ER7 wire core

`octofhir-hl7v2` implements an original, schema-free ER7 parser and editor with no dependencies in the default library. Explicit [numeric/date views](scalars.md), [caller-supplied schema grouping](schema.md), [MLLP framing](mllp.md), [ACKs](ack.md), optional Tokio [async connections](transport.md) and PostgreSQL [custody](inbox.md) are available separately. Versioned schema catalogs, FHIR conversion and clinical processing remain planned. See the [architecture](architecture.md) and [implementation plan](implementation-plan.md).

## Representation and access

`Message::parse` checks the byte limit, retains the exact input in `Arc<[u8]>` and builds a flat arena index. Each node contains a source range and a contiguous range of child nodes; there is no allocation per child list. Message clones share the bytes and index. `Message::from_shared` accepts an existing `Arc<[u8]>` without copying it.

`original()` returns unchanged bytes, including unknown/Z-segments, spaces, trailing separators, escape spellings and non-UTF-8 content. Numbers and partial dates are never normalized. `Value::{decimal, date, date_time}` explicitly validate [lexical views](scalars.md) that retain signs, zeros, partial precision and offset presence. `Message::timestamp(path)` also applies composite TS precision overrides. These operations do not select schema types automatically.

All paths are one-based. `value(&[segment, field, repetition, component, subcomponent])` accepts two to five coordinates. Segment numbers count every segment in wire order, including MSH. `segment_named(b"OBX", n)` finds the nth occurrence of an ID; its `occurrence()` gives that segment's global number. A field's children are repetitions, then components, then subcomponents. MSH-1/2 are opaque declarations with no children.

`presence()` returns `Missing`, `Empty`, `Null` (raw bytes exactly `""`) or `Value`. Missing values have no source range; empty values have a zero-length range. `span()` is a half-open byte range into the original. Diagnostic paths use five coordinates with zero for unspecified positions. Null placement and clinical update meaning require a schema/interface contract.

The [README example](../README.md#available-now) uses normal error propagation and is compiled directly as the library doctest.

## Supported subset

- One message starts at MSH; a second MSH, empty segments, malformed segment IDs and batch envelopes are rejected. IDs contain three uppercase ASCII letters/digits.
- Delimiters are distinct ASCII punctuation excluding double quotes. MSH-2 has four encoding characters and optionally a fifth truncation character. Omitted, alphanumeric and multibyte delimiters are unsupported by this initial subset.
- CR is the default terminator. Set `ParseOptions::compatible_line_endings` to accept CRLF/LF with diagnostics. All terminators and spaces are retained. A missing final terminator is retained with a diagnostic.
- Parsing accepts bytes without requiring UTF-8. It does not transcode MSH-18 character sets or process charset-switch escapes.
- Structure is indexed before scalar decoding. Matched escapes are skipped during delimiter scanning. Unterminated escapes remain visible with diagnostics and do not swallow later structural positions.
- `decode()` handles F/S/R/T/E, declared truncation P and valid hex escapes in one pass. Decoded separators/CR stay scalar data. A value with several children, an explicit null or a missing position cannot be decoded as one scalar.
- An unescaped terminal truncation marker makes scalar decoding fail with `Truncation`, even if diagnostics were omitted. Nonterminal and escaped literal markers (`P` or hex) remain scalar data. Raw inspection and original emission still work; deciding a datatype's truncatability requires a versioned profile.
- Formatting, highlighting, charset, local and unknown escapes remain raw. Scalar decoding refuses them and malformed hex/unterminated escapes explicitly. The public API exposes raw values and diagnostic ranges rather than a separate token object model.

The wire rules were checked against the HL7 v2.5.1 [control chapter](https://hl7.eu/HL7v2x/v251/std251/ch02.html). This is not full version conformance or clinical validation. Code and fixtures were authored here; no upstream implementation or tests were copied.

Terminal-marker handling was also checked against the v2.7 [truncation discussion](https://hl7.eu/HL7v2x/v27/std27/ch02.html).
Generic decoding conservatively refuses possible truncation; version/datatype-specific truncation
policies are not implemented. Nonterminal marker characters remain ordinary data.

## Editing and bounds

`Message::create(delimiters, options)` starts a fresh message with only separator declarations in
a CR-terminated MSH. Populate remaining header fields and segments explicitly before schema checks.

`message.edit().set(&[segment, field, ...], Edit::Value(bytes))` replaces an existing position with unescaped scalar bytes. Changed delimiters and control bytes are escaped once. `Empty` and `Null` are explicit operations. Literal `""` supplied as a value is hex-escaped to distinguish it from null. Untouched source ranges are copied verbatim.

`set_or_create` extends missing positions within existing segments, filling intervening siblings with explicit empty values. Several sparse creations are combined in path order. `Unset` removes one originally final sibling per parent per revision, including its preceding separator; it cannot be combined with creating later siblings. Interior deletion and removal of the only child of a present field/component are refused. Unsetting an absent position cancels its exact pending creation or is a no-op. Parent/descendant edits conflict. MSH-1/2 and delimiter changes are refused. See the [editing contract](editing.md) for null handling, limits and a compiled example. Non-MSH segment builders, insertion at original boundaries and whole-segment removal are available
in the same sparse editor; see the [segment editing contract](editing.md#segment-construction-and-splicing).

The editor rejects messages with unterminated escapes, including when diagnostics have been omitted by the display budget. Otherwise an inserted escape could pair with an old unmatched marker across field boundaries. These messages remain available for raw inspection and original-byte emission.

`serialize()` applies bounded byte-range patches. `finish()` also reparses the revision with the original policy and limits; the original message stays unchanged. Defaults are 8 MiB of input/output, 16,384 segments, 262,144 index nodes and 1,024 retained diagnostics. Omitted diagnostics are counted. Pending patch count and encoded replacement/output bytes are bounded; container overhead is additional memory. Indexing depth is fixed at field/repetition/component/subcomponent. Debug output omits the original payload.

## Verification

```sh
cargo test --locked --offline
cargo clippy --locked --offline --all-targets -- -D warnings
cargo run --offline --example inspect -- tests/fixtures/adt.er7
cargo bench --offline --bench wire
```

Six initial integration tests cover source retention, positions/presence, escapes, input/resource failures, patch conflicts and generated inputs. Three creation scenarios cover sparse ordering and budgets; five scalar scenarios cover numeric/temporal/TS semantics. Generated checks use 10,000 byte inputs and 2,000 scalar replacement/creation round trips with shuffled delimiters. A separate [coverage-guided fuzz harness](../fuzz/README.md) checks wire/edit/TS/creation/ACK and MLLP invariants with AddressSanitizer; CI runs bounded smoke campaigns. Fixture origin, license, lengths and digests are recorded in the [manifest](../tests/fixtures/manifest.json).

The benchmark separates message parsing/indexing, lexical decimal/DTM parsing, serialization of prepared sparse patches, and patch preparation + serialization + reindexing. Correctness checks run before timing. The edit case changes one field, creates nested positions and explicit null, and checks that untouched segments match. Prepared serialization excludes source parsing, patch preparation and reindexing; the combined edit case excludes only initial source parsing. Timings include allocations on their respective paths but do not count allocations or measure peak memory. They exclude schema validation, transport and durability.

For a local warmed run, execute `HL7V2_BENCH_ITERS=100000 cargo bench --locked --offline --bench wire` three times sequentially. Keep build flags and corpus fixed; report each run and its included boundary rather than mixing the result with MLLP/clinical throughput. The fixtures' hashes are in the manifest above.

A previous local comparison on macOS arm64 used the previous draft and flat index on the same two synthetic fixtures, Rust 1.99, optimized builds, three sequential runs of 100,000 parses each:

| Fixture | Previous draft median | Flat index median |
|---|---|---|
| ADT, 229 bytes | 6,563 ns/message | 2,123 ns/message |
| ORU, 411 bytes | 9,044 ns/message | 3,088 ns/message |

These are local timings on a small corpus, not throughput guarantees. Allocation accounting, larger sender corpora and sustained fuzz campaigns remain open verification work.
