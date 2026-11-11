# Core fuzzing

This unpublished development harness uses cargo-fuzz/libFuzzer and AddressSanitizer. Its separate
workspace keeps fuzzing dependencies and nightly Rust out of the library and its lockfile. Seeds
are our synthetic wire fixtures; no upstream HL7 implementation or corpus is used. Generated
corpus entries and artifacts are ignored by Git.

## Run from the repository root

```sh
rustup toolchain install nightly-2026-09-27 --profile minimal
cargo install cargo-fuzz --version 0.13.2 --locked
cargo fetch --locked --manifest-path fuzz/Cargo.toml
mkdir -p fuzz/corpus/wire fuzz/corpus/mllp fuzz/corpus/schema
cp tests/fixtures/*.er7 fuzz/corpus/wire/
cp tests/fixtures/*.er7 fuzz/corpus/mllp/
cp tests/fixtures/*.er7 fuzz/corpus/schema/
cargo +nightly-2026-09-27 fuzz run wire -- -dict=fuzz/wire.dict -seed=701 -max_total_time=60 -max_len=4096 -rss_limit_mb=512 -timeout=5
cargo +nightly-2026-09-27 fuzz run mllp -- -dict=fuzz/mllp.dict -seed=701 -max_total_time=60 -max_len=4096 -rss_limit_mb=512 -timeout=5
ASAN_OPTIONS=quarantine_size_mb=64:thread_local_quarantine_size_kb=256 cargo +nightly-2026-09-27 fuzz run schema -- -seed=733 -max_total_time=60 -max_len=16384 -rss_limit_mb=512 -timeout=5
```

Requires a C++ compiler supported by libfuzzer-sys. Sanitization, debug assertions and overflow
checks are enabled by cargo-fuzz's default build; `--release` disables the assertions/checks.
The pinned toolchain, tool version and harness lockfile leave the library's stable/MSRV compiler
unchanged. See the [Rust Fuzz Book](https://rust-fuzz.github.io/book/cargo-fuzz/guide.html).

The harness caps input at 16 KiB and bounds parser nodes, segments and diagnostics. These smoke
commands restrict generated inputs to 4 KiB, per-input runtime to 5 s and process RSS to 512 MiB.
RSS includes instrumentation and is not a library allocation measurement. A time-bound run can
finish its last input after the nominal deadline. Longer campaigns can raise `max_total_time`
and use `max_len=16384` without changing the harness's input boundary.

## Invariants

`wire` checks accepted source retention, source spans and parent/child bounds, fixed indexing depth,
missing/out-of-range access, retained-diagnostic limits and byte-identical original serialization.
It also validates arbitrary standalone decimal/DT/DTM inputs and explicit typed node reads,
checking source preservation, borrowed standalone decimals and precision/fraction bounds.
It submits combinations of scalar/empty/null/unset edits at several positional depths and verifies
that successful serialization reparses with the original limits. Sparse creation also checks the
requested values after single and batched insertions; TS reads check effective precision at all
supported path depths. Eligible v2.5.1 requests also
exercise ACK generation: the result must parse, avoid malformed escapes and correlate in the
explicitly selected original or enhanced accept-only mode. Unsupported envelopes may be rejected.

`mllp` compares arbitrary input decoded all at once and in input-derived small chunks: completed
frames and final errors must agree. It checks progress, buffered-size limits, poisoned state and
reset. For encodable payloads it verifies exact writer/buffer equivalence and reconstruction of
three coalesced frames, including an empty frame. Invalid outgoing payloads must write no bytes.
The shared ER7 seeds are useful payloads; the target constructs valid frames itself.

`schema` compiles generated flat rules/cardinalities and a synthetic nested group grammar. It
constructs valid instances and checks complete, ordered leaf coverage, borrowed source identity,
occurrence bounds and error spans under smaller budgets. Arbitrary accepted ER7 inputs also
exercise resolution and direct shape matching. Definitions are capped at 64 rules, depth 8 and
4,096 compilation work units; source input is capped at 16 KiB/512 segments/8,192 wire nodes and
matching at 1,024 occurrences/4,096 steps. No third-party schema data is included.

The schema command retains ASAN instrumentation with a 64 MiB freed-block quarantine and 256 KiB
thread-local quarantine to fit the 512 MiB process budget. An initial macOS run with the default
quarantine reached that process limit: its report showed about 238 MiB quarantined and 25 MiB live
heap, mostly libFuzzer. The emitted empty artifact replayed successfully. This harness setting is
not a change to library limits or a measurement of its peak memory.

CI runs bounded smoke campaigns; it does not establish complete coverage or prove absence of bugs.
This harness does not exercise network scheduling, durable storage or FHIR semantics.
Synthetic schema coverage does not establish version-specific catalog conformance.
Connection/receiver tests cover their separate concurrency and shutdown contracts.

Local evidence on 2026-11-11, macOS arm64, pinned tools above, AddressSanitizer (default unless noted):

| Target | Seed | Maximum generated input | Runs | Result |
|---|---|---|---|---|
| MLLP | 701 | 4 KiB | 8,339,442 in 61 s | No invariant/sanitizer failure |
| Wire/edit/ACK, after fix | 703 | 16 KiB | 592,269 in 61 s | No invariant/sanitizer failure |
| Wire/edit/ACK and scalar views | 709 | 4 KiB | 265,868 in 46 s | No invariant/sanitizer failure |
| Wire/TS/creation/edit/ACK | 719 | 16 KiB | 195,223 in 61 s | No invariant/sanitizer failure |
| Schema, 64 MiB quarantine | 733 | 16 KiB | 437,059 in 61 s | No invariant/sanitizer failure |

The wire campaign used the evolving synthetic corpus from an earlier run. These are finite
campaigns, not performance measurements or complete coverage. An initial wire run found that an
unmatched escape in a copied MSH address field could reach a generated ACK. The minimized case
now replays successfully; regression coverage includes malformed/unsupported escapes, encoded
controls and invalid charset bytes even with `max_diagnostics=0`.

## Reproduce and retain a failure

```sh
cargo +nightly-2026-09-27 fuzz run wire fuzz/artifacts/wire/CRASH_FILE
cargo +nightly-2026-09-27 fuzz tmin wire fuzz/artifacts/wire/CRASH_FILE
```

Replace `CRASH_FILE` and the target with the actual reported artifact. Inspect a minimized input
and add the smallest focused regression to the integration suite. Any new binary fixture also
needs an origin/license/hash entry in `tests/fixtures/manifest.json`. Do not commit the growing
generated corpus. CI uploads failing artifacts from synthetic inputs for inspection.
