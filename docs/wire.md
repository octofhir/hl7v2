# ER7 wire core

`octofhir-hl7v2` implements an original, schema-free ER7 parser and editor with no dependencies in the default library. Explicit [numeric/date views](scalars.md), [MLLP framing](mllp.md), [ACKs](ack.md), optional Tokio [async connections](transport.md) and PostgreSQL [custody](inbox.md) are available separately. Schemas, FHIR conversion and clinical processing remain planned. See the [architecture](architecture.md) and [implementation plan](implementation-plan.md).

## Representation and access

`Message::parse` checks the byte limit, retains the exact input in `Arc<[u8]>` and builds a flat arena index. Each node contains a source range and a contiguous range of child nodes; there is no allocation per child list. Message clones share the bytes and index. `Message::from_shared` accepts an existing `Arc<[u8]>` without copying it.

`original()` returns unchanged bytes, including unknown/Z-segments, spaces, trailing separators, escape spellings and non-UTF-8 content. Numbers and partial dates are never normalized. `Value::{decimal, date, date_time}` explicitly validate [lexical views](scalars.md) that retain signs, zeros, partial precision and offset presence. They do not select schema types or interpret composite TS precision overrides.

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
- Formatting, highlighting, charset, local and unknown escapes remain raw. Scalar decoding refuses them and malformed hex/unterminated escapes explicitly. The public API exposes raw values and diagnostic ranges rather than a separate token object model.

The wire rules were checked against the HL7 v2.5.1 [control chapter](https://hl7.eu/HL7v2x/v251/std251/ch02.html). This is not full version conformance or clinical validation. Code and fixtures were authored here; no upstream implementation or tests were copied.

## Editing and bounds

`message.edit().set(&[segment, field, ...], Edit::Value(bytes))` replaces an existing position with unescaped scalar bytes. Changed delimiters and control bytes are escaped once. `Empty` and `Null` are explicit operations. Literal `""` supplied as a value is hex-escaped to distinguish it from null. Untouched source ranges are copied verbatim.

`Unset` removes one final sibling per parent per revision, including its preceding separator. Interior deletion and removal of the only child of a present field/component are refused because they would shift positions or turn missing into empty. Unsetting an absent position is a no-op. Creating missing positions is deferred to a builder. Parent/descendant edits conflict; another edit to the same path replaces the pending edit. MSH-1/2 and delimiter changes are refused.

The editor rejects messages with unterminated escapes, including when diagnostics have been omitted by the display budget. Otherwise an inserted escape could pair with an old unmatched marker across field boundaries. These messages remain available for raw inspection and original-byte emission.

`serialize()` applies bounded byte-range patches. `finish()` also reparses the revision with the original policy and limits; the original message stays unchanged. Defaults are 8 MiB of input/output, 16,384 segments, 262,144 index nodes and 1,024 retained diagnostics. Omitted diagnostics are counted. Pending patch count and encoded replacement/output bytes are bounded; container overhead is additional memory. Indexing depth is fixed at field/repetition/component/subcomponent. Debug output omits the original payload.

## Verification

```sh
cargo test --locked --offline
cargo clippy --locked --offline --all-targets -- -D warnings
cargo run --offline --example inspect -- tests/fixtures/adt.er7
cargo bench --offline --bench wire
```

Six integration tests cover source retention, positions/presence, escapes, input/resource failures, patch conflicts and generated inputs. Generated checks use 10,000 byte inputs and 2,000 scalar round trips with shuffled delimiters. A separate [coverage-guided fuzz harness](../fuzz/README.md) checks wire/edit/ACK and MLLP invariants with AddressSanitizer; CI runs bounded smoke campaigns. Fixture origin, license, lengths and digests are recorded in the [manifest](../tests/fixtures/manifest.json).

The benchmark times parsing and index construction, including their allocations, without validation, transport or durability. A local comparison on macOS arm64 used the previous draft and flat index on the same two synthetic fixtures, Rust 1.99, optimized builds, three sequential runs of 100,000 parses each:

| Fixture | Previous draft median | Flat index median |
|---|---|---|
| ADT, 229 bytes | 6,563 ns/message | 2,123 ns/message |
| ORU, 411 bytes | 9,044 ns/message | 3,088 ns/message |

These are local timings on a small corpus, not throughput guarantees. Allocation accounting, larger sender corpora and sustained fuzz campaigns remain open verification work.
