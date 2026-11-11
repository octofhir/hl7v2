# PostgreSQL custody inbox

The optional `postgres` feature adds an initial custody adapter over a host-provided SQLx 0.9
`PgPool`. It accepts the existing v2.5.1 enhanced `AL/NE` envelope subset. Successful receipt means
the original, selected plan identity, schedulable `ready` row, delivery outcome and ACK bytes have
committed together. It does not mean clinical validation, FHIR conversion or application succeeded.

```no_run
use octofhir_hl7v2::{inbox::{AckMetadata, Inbox, Receipt}, wire::Message};
use std::sync::Arc;

async fn accept(
    inbox: &Inbox,
    payload: Arc<[u8]>,
    plan: &[u8; 32],
    response: AckMetadata<'_>,
) -> Result<Receipt, Box<dyn std::error::Error>> {
    let message = Message::from_shared(payload, Default::default())?;
    // The host has authenticated this channel and checked its sender/interface contract.
    Ok(inbox.receive("tenant/interface/sender-policy-v1", "host-channel", &message, plan, response).await?)
}
```

Construct `Inbox::new(pool, max_payload, statement_timeout)` once and share its cheap clones.
The payload limit must be 1 byte–8 MiB; statement timeout must be 1–2,147,483,647 milliseconds.
ACK output is capped at 64 KiB. The host supplies each attempted response's fresh MSH-10 and
complete timestamp through `AckMetadata`; duplicates replay the prior ACK bytes instead.

## Schema and transaction boundary

Apply [0001_inbox.sql](../migrations/0001_inbox.sql) **once** using the host's migration tool.
The same text is exported as `inbox::SCHEMA`. It creates the `hl7v2` schema and its `inbox` and
`deliveries` tables. Initialization is explicit, not an automatic action on receipt. Future schema
changes need ordered migrations; this adapter does not manage migration history or deployment.
Normal receipt needs schema/sequence usage and SELECT/INSERT on both tables; it needs neither
superuser nor UPDATE access. Schema initialization and test trigger installation use a separate
owner role.

Each `receive` uses an explicit READ COMMITTED transaction. PostgreSQL's unique constraint chooses
one original for `(scope, processing_id, decoded MSH-10)`. `INSERT ... ON CONFLICT DO NOTHING`
waits for a competing insertion; a separate statement can then read its committed winner. There
is no process-local deduplication cache, check-then-insert race or silent update of the original.

Before inserting, the adapter checks `fsync=on`, `full_page_writes=on` and a `synchronous_commit`
mode that includes local WAL flush: `on`, `local`, `remote_write` or `remote_apply`. It rejects
`off` and retains the deployment's selected mode instead of downgrading it. The selected mode is
recorded per delivery. Actual storage, replication, backups and disaster-recovery guarantees remain
deployment policy. These checks do not promise survival of database-host/storage loss. See
[PostgreSQL WAL settings](https://www.postgresql.org/docs/14/runtime-config-wal.html).

A receipt is returned only after `commit().await` succeeds. Errors return no ACK; timeout/cancellation
or connection loss during commit can leave the outcome unknown. Retry the same identity and bytes
to recover the applicable response. SQL statement timeouts do not replace pool acquisition limits,
host admission, an overall handler deadline or unknown-commit reconciliation.

## Identity, plans and outcomes

`scope` is trusted host configuration, not an MSH claim. It must bind tenant, interface, authenticated
sender, permitted MSH-3/MSH-4 identities, identity-policy revision and any control-ID reuse epoch.
The host validates those bindings before calling `receive`. The adapter additionally separates
MSH-11 processing IDs (`P/T/D`) in the key. MSH-10 is decoded, nonempty and limited to 256 bytes.
Changing only its escape spelling still addresses the same key. No MSH timestamp or patient ID is
added to evade duplicate detection. Never mix independent sender namespaces under one scope.

The 32-byte `plan` identifies the host's immutable processing manifest: exact schemas, overrides,
maps, terminology, validation and application policy. Artifact storage/resolution is not implemented
here. The optional [native schema package](schema-packages.md) supplies a content-bound schema
digest to include in that manifest; it is not itself the whole processing-plan ID.
The host must preserve those assets; a digest alone does not make a plan executable. The
first receipt pins its plan. Later configuration changes do not rewrite it; each delivery records
the newly requested plan separately. Missing historical assets must fail later replay explicitly.
An interface must admit only plans it can actually process; this module cannot enforce artifact
availability or the interface's clinical acceptance policy.

| Outcome | Durable result and response |
|---|---|
| New | One inbox row, an original delivery and a saved `CA` |
| Duplicate | Same exact payload; another delivery, original plan and exact prior `CA` bytes |
| Conflict | Different payload under the same key; both originals retained, no second work item, saved `CE` with a bounded ERR-7 diagnostic |

SHA-256 covers exact ER7 payload bytes, excluding MLLP framing and any normalization. The comparison
also checks actual bytes; matching hashes cannot turn a different payload into a duplicate. The
initial conflict response uses ERR-3 code 207 and does not reuse the original acceptance. `CE` follows
the v2.5.1 acceptance-error phase; `CR` is reserved for unacceptable message type/version/processing
headers. See the [control chapter](https://hl7.eu/HL7v2x/v251/std251/ch02.html).

Each delivery retains channel evidence, attempted plan, exact payload/hash, outcome, ACK bytes and
commit mode. The winning payload is also in its inbox row, so the first delivery stores it twice.
This is deliberate initial duplication; storage quota, access, retention and protected backups
belong to the host. Original and selected plan are unchanged through this API. Deleting key evidence
without a retention/replay policy can permit duplicate clinical effects and is not implemented.

## MLLP integration and remaining scope

The `serve` handler can parse its shared payload, call `receive`, and return
`Some(receipt.acknowledgement)`. A write failure after commit does not undo receipt; the sender's
repeat recovers its saved response. An inbox error should close the connection or follow an explicit
failure-ACK policy; never fabricate `CA`. A partial frame never reaches the inbox handler.

The host owns configured pool size, acquisition timeout, authenticated channels, TLS and bounded
admission. SQLx's PostgreSQL driver and SHA-256 are optional; default ER7/ACK/framing remain free of
runtime dependencies. This adapter does not join the OctoFHIR server's FHIR transaction.

An optional second migration adds [work claims, leases, attempt history, retries and quarantine](work.md).
These APIs fence inbox metadata; [same-plan quarantine replay](replay.md) adds audited scheduling.
New-plan reprocessing, ACK emission attempts, original-mode application
completion, full schema validation and FHIR application remain planned. Invalid/unkeyed envelopes are rejected
before receipt; bounded forensic/quarantine storage for them is a later processing concern.

## Verification

Tests require an explicit **dedicated test database** with this migration applied. They create
synthetic rows and install a test-only deferred trigger to refuse COMMIT. Normal `cargo test` lists
these tests as ignored; it does not silently substitute memory storage for PostgreSQL.

```sh
psql "$HL7V2_TEST_DATABASE_URL" --single-transaction -v ON_ERROR_STOP=1 -f migrations/0001_inbox.sql
cargo test --locked --all-features --test inbox -- --ignored --skip recovery_across_postgres_crash
HL7V2_RECOVERY_PHASE=seed cargo test --locked --all-features --test inbox recovery_across_postgres_crash -- --ignored
# Crash and restart ONLY that dedicated PostgreSQL instance, preserving its data directory.
HL7V2_RECOVERY_PHASE=verify cargo test --locked --all-features --test inbox recovery_across_postgres_crash -- --ignored
```

Coverage includes concurrent duplicates, changed payloads, fixed plan/ACK recovery, environment
scoping, commit-time refusal, asynchronous-WAL rejection, TCP disconnect/retry and incomplete EOF.
The explicit two-process recovery test checks the pre-crash response and plan after database restart.
Verification also requires the server's start time to postdate the checkpoint.
CI uses a dedicated PostgreSQL service and kills/restarts its container between the recovery phases.
