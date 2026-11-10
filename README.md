<p align="center"><img src=".github/banner.png" alt="octo/hl7v2 — HL7 v2 messages, from wire to FHIR" width="100%"></p>

<p align="center"><b>HL7 v2 tooling in Rust: MLLP transport, message schemas and FHIR Mapping Language maps.</b></p>

<p align="center">
  <img src="https://img.shields.io/badge/status-early%20development-f5b335" alt="Status: early development">
  <a href="#license"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg" alt="License: MIT OR Apache-2.0"></a>
</p>

> ER7 parsing/editing, bounded MLLP framing, v2.5.1 ACKs and optional async connections are implemented. Schemas, FHIR maps and durable processing are planned.

## Available now

The `octofhir-hl7v2` Rust package provides an original ER7 parser and editor. Its default library
has no third-party dependencies. It retains exact source bytes, byte spans, field positions, repetitions and unknown/Z-segments,
and distinguishes missing, empty, explicit null and populated positions. Scalar escape decoding is
explicit; unsupported escapes remain inspectable. Numbers and dates retain their original lexemes.
Parser and editor limits bound input, index and output growth.

The initial supported subset uses distinct ASCII punctuation delimiters, four or five MSH-2 encoding
characters and CR segment terminators; CRLF/LF require an explicit compatibility option. Parsing
accepts non-UTF-8 bytes but does not transcode character sets or validate an HL7 schema/version.
See the [wire API and limitations](docs/wire.md).

The [MLLP codec](docs/mllp.md) handles fragmented/coalesced frames, an explicit payload limit,
incomplete EOF and framing errors. It returns one borrowed payload at a time and retains no output
queue. Encoding supports a framed buffer or a standard writer.

[ACK generation and correlation](docs/ack.md) support a documented v2.5.1 envelope subset, original
mode and enhanced acknowledgement conditions. The optional `transport` feature adds Tokio-backed
[async connections](docs/transport.md): persistent sessions, bounded frames, deadlines and one
outstanding request with MSA-2 correlation. The host owns the listener, connection/admission limits,
processing and ACK timing. There is no durable inbox or automatic success ACK.

```rust
use octofhir_hl7v2::wire::{Message, Presence};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input = b"MSH|^~\\&|LAB\rPID|1||p1||Example^Alex|\r";
    let message = Message::parse(input)?;
    let pid = message.segment_named(b"PID", 1).ok_or("PID segment is missing")?;
    assert_eq!(pid.field(6).presence(), Presence::Empty);
    assert_eq!(pid.field(7).presence(), Presence::Missing);
    assert_eq!(pid.field(5).child(1).child(2).decode()?.as_ref(), b"Alex");
    Ok(())
}
```

From this checkout, with Rust 1.98 or newer:

```sh
cargo test --locked --offline
cargo test --locked --offline --all-features
cargo run --offline --example inspect -- tests/fixtures/adt.er7
cargo bench --offline --bench wire
```

The package has not been published to crates.io. Benchmark output is a local parser/index timing,
without schema validation, transport, durability or FHIR processing.
Tokio is optional for the library and is also used by development tests. No third-party HL7
implementation is used.

## Direction

Build an HL7 v2 library that works independently of a FHIR server, with optional FHIR conversion
and a runtime that can be embedded in [OctoFHIR](https://github.com/octofhir/server-rs) or deployed
as a separate gateway.

| Layer | Planned scope |
|---|---|
| Message core | ER7 parsing and serialization; preserve field positions, repetitions, custom delimiters, unknown segments and original bytes |
| Schemas | Versioned message, segment and datatype definitions; nested groups and per-interface overrides |
| MLLP | Async receiver and sender; persistent connections, bounded frames, timeouts, backpressure and correlated HL7 acknowledgements |
| FHIR conversion | FML maps on [fml-rs](https://github.com/octofhir/fml-rs), ConceptMaps, validation and reports of unmapped or lost data |
| Processing | Durable inbox, delivery deduplication, entity identity, event ordering, retries, quarantine and replay |

Initial mapping candidates are ADT A01/A03/A08 and ORU R01 to FHIR R4. Their exact scope will be
defined against a representative message corpus.

## Design principles

- Parse the wire format independently of the schema; apply schema-aware views and validation on top.
- Keep transport and the message core independent of FHIR.
- Use FML and ConceptMap for FHIR mappings.
- Keep conversion separate from applying an event to existing clinical data.
- Distinguish message identity from patient, encounter, order and result identity.
- Define acknowledgement semantics per interface, including the boundary between durable receipt
  and successful application processing.
- Measure performance with the validation and durability guarantees stated explicitly.

The [HL7 V2 to FHIR Implementation Guide](https://hl7.org/fhir/uv/v2mappings/) is a mapping reference.
Its ConceptMaps are not a ready-to-run set of FML transforms.

The [architecture](docs/architecture.md) and [implementation plan](docs/implementation-plan.md)
record accepted decisions, integration boundaries and remaining verification gates.

## Related projects

- [OctoFHIR server](https://github.com/octofhir/server-rs)
- [fml-rs](https://github.com/octofhir/fml-rs) — FHIR Mapping Language parser, checker and engine
- [ccda-fhir](https://github.com/octofhir/ccda-fhir) — C-CDA and FHIR conversion on FML maps

## License

Licensed under either [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
