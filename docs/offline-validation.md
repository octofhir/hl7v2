# Offline package validation

The optional `hl7v2-validate` binary loads one native schema package against a trusted expected
schema digest and validates one or more local ER7 message files. Build with `schema-package`;
it uses the existing library APIs and adds no dependencies. The package is activated once before
opening messages, and its owned compiled plans are reused across files.

From the repository root, this original synthetic example exits successfully:

```sh
cargo run --locked --offline --features schema-package --bin hl7v2-validate -- \
  tests/fixtures/native-package-v1.bin \
  0161081c3eb15536d263ba1b1f357725b0efc43b7e2d02a58b541dda1e669cd1 \
  tests/fixtures/native-package.er7
```

The syntax is `hl7v2-validate PACKAGE EXPECTED_SHA256 ER7_FILE [ER7_FILE ...]`. Supply exactly
64 ASCII hexadecimal digits; uppercase and lowercase are accepted. Production callers obtain the
expected schema digest from their retained processing plan, not from an untrusted file's embedded
pin. The [package loader](schema-packages.md) verifies the embedded pin and recompiled identity.
A schema digest remains only one part of a full processing plan.

Each input file contains exactly one unframed ER7 message. Files are read sequentially with a byte
cap and retain exact source bytes; there is no line-ending normalization. The command uses default
`ParseOptions`, `MatchLimits` and `ValidationLimits`: strict CR segment endings, bounded indexing,
grouping and field checks. Package loading uses default `LoadLimits`. There are no command-line
limit overrides, compatibility mode, directory traversal, stdin or MLLP stream handling. Library
callers can select their own explicit policies through the existing APIs.

## Output and completion

The output begins with the verified schema digest. `input=N` identifies the corresponding argument
in one-based order. Input paths and field values are not printed. Reports contain only byte/count
metadata, issue kinds, original half-open byte spans, one-based wire paths, presence states and
schema rule indexes. Zero path coordinates mean unspecified suffixes, as in the library APIs.
Output is human-readable diagnostic text, not a versioned machine interchange format.

For a parsed message, wire diagnostics and their omitted count are printed first. The command then
resolves the exact release/event/structure, groups every segment and validates field profiles.
`schema_valid=true` means those selected structure/field checks completed successfully; it does not
mean full HL7 conformance or clinical acceptance. Missing final CR, for example, is a retained wire
diagnostic permitted by the parser and does not by itself change schema validity. Fatal wire errors
prevent schema checks. Wire diagnostics remain separate from the field report's `complete` and
`omitted` accounting.

The field summary includes validity, completion, retained/omitted issues and charged steps/bytes.
Default field reports retain at most 1,024 issues; work/byte exhaustion or omitted field issues
cannot yield successful schema validation. Default wire diagnostics also retain at most 1,024
entries and expose their separate omitted count. See [wire bounds](wire.md), [grouping bounds](schema.md)
and [field-report bounds](validation.md) for the exact admission and validation contract.

| Exit status | Meaning |
|---|---|
| 0 | Every input parsed and passed the selected structure/field checks. Nonfatal wire diagnostics may still be present. |
| 1 | At least one input failed wire parsing, structure selection/grouping or field validation, including incomplete/omitted field reports. |
| 2 | Usage, pin/package activation, input/output I/O or other operational failure. This takes precedence over status 1. |

Activation/usage failures go to stderr and stop before message files are opened. Per-file read and
validation failures are reported on stdout and processing continues with later files. An output I/O
failure stops the run and returns status 2; partial output is not a complete result.

This command does not emit ACKs, persist custody, process batch envelopes or execute FML/FHIR.
The bundled message/package demonstrate native persistence and validation only. They are synthetic
fixtures with recorded provenance and hashes, not approved official HL7 schemas. Standards sources,
rights and completeness remain the existing release gate.
