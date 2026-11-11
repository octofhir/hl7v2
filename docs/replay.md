# Same-plan quarantine replay

`Inbox::replay` schedules another bounded run of a **quarantined** message using its original
immutable plan. It is an explicit host command with a durable audit record, not an automatic retry
or a new HL7 delivery. Apply [0003_replay.sql](../migrations/0003_replay.sql) after migrations 0001/0002;
the text is also exported as `inbox::REPLAY_SCHEMA`.

```no_run
use octofhir_hl7v2::inbox::{Inbox, Lease, ReplayRequest};

async fn schedule(inbox: &Inbox, scope: &str, quarantined: &Lease,
    request_id: &str, operator: &str, reason: &str)
    -> Result<i64, Box<dyn std::error::Error>>
{
    Ok(inbox.replay(scope, ReplayRequest {
        request_id,
        message_id: quarantined.message_id(),
        expected_generation: quarantined.generation(),
        plan: quarantined.plan(),
        additional_attempts: 2,
        operator,
        reason,
    }).await?)
}
```

The host supplies a stable, unique command ID and binds operator/scope to its authorization policy.
Metadata strings alone do not authenticate a reviewer. The host verifies that the exact historical artifacts are available
before scheduling. This API does not prompt users, load assets or authorize clinical effects.

## Eligibility and idempotency

The command names a message in its trusted scope, the observed generation, the original plan,
1–1000 additional attempts, an operator and a reason. Source bytes/SHA-256 are checked before a
new schedule. Active, ready, retry-scheduled and completed work cannot be newly replayed. Wrong
scope, generation or plan returns `ReplayRejected`; changing map/ConceptMap versions requires a
separate reviewed reprocessing revision, which is not implemented here.

The message row is locked before checking eligibility. The audit insert and transition to `ready`
commit together under the existing durability checks. Failure at COMMIT exposes no successful
result. Each command is unique by `(scope, request_id)`. An exact repeat returns its prior audit ID
without rescheduling, including after the work was claimed/completed or the database restarted.
Reusing that ID with different parameters returns `ReplayConflict`. Concurrent commands with
different IDs cannot both reset the same observed quarantine state.

Command ID is capped at 128 bytes, operator at 256 and reason at 2048. Values must be nonempty and
contain no control characters. Audit retains the old generation, plan, attempt ceiling, quarantine
reason, new budget, operator/reason, commit mode and time. Keep its retention/access policy with
the original message and attempt history; deleting audit rows invalidates command-retry evidence.

## Generations and budgets

Replay preserves the original payload, hash, plan, message key, custody ACK and all finalized
attempts. It does not increment or reset generation. The next claim increments it, so old leases
cannot become active again. The reviewed budget is an absolute ceiling of
`expected_generation + additional_attempts`; arithmetic overflow is rejected. Worker claim options
cannot enlarge that ceiling. Another review after a later quarantine needs a new command ID and
the newly observed generation.

Attempts from each reviewed run retain its replay ID. Subsequent retries in that run keep the same
link. Migration 0003 widens the absolute attempt ceiling to `bigint`, allowing long-lived messages
to accumulate history beyond 1000 attempts while each reviewed run stays bounded. Drain old worker
binaries before this migration and deploy the new work APIs after it; older SQLx decoders expect
an integer column. Custody ingestion remains independent of the worker counter.

This is source/plan pinning, not a guarantee of bit-identical conversion output: the host must also
fix or record clock values, ID assignments and external terminology outcomes. Generation and replay
ID are not new clinical effect identities. Even quarantined work may have an unknown external-write
outcome; the host must reconcile that outcome before permitting another run. FHIR application,
target-side idempotency, dry runs and new-plan reprocessing remain outside this API.

## Verification

On a dedicated database with all migrations applied:

```sh
cargo test --locked --all-features --test replay -- --ignored --skip replay_recovery_across_postgres_crash
HL7V2_RECOVERY_PHASE=seed cargo test --locked --all-features --test replay replay_recovery_across_postgres_crash -- --ignored
# Crash and restart only that dedicated PostgreSQL instance, preserving its data directory.
HL7V2_RECOVERY_PHASE=verify cargo test --locked --all-features --test replay replay_recovery_across_postgres_crash -- --ignored
```

Scenarios cover source/history preservation, bounded subsequent runs, stale leases/commands,
concurrent exact retries, scope/plan/command-ID conflicts, completed-work refusal, commit rollback,
generation 1002 and command recovery after database restart. They use synthetic data/plans and
perform no FHIR writes.
