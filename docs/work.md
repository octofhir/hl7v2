# Durable inbox work

The optional PostgreSQL adapter exposes bounded work-state transactions, not a background daemon.
The host polls `Inbox::claim`, resolves the pinned assets, executes its processor and manages
shutdown. Apply [0002_work.sql](../migrations/0002_work.sql) once after the custody migration;
the SQL is also exported as `inbox::WORK_SCHEMA`. Existing payload, plan and ACK bytes are unchanged.

```no_run
use octofhir_hl7v2::inbox::{Claim, ClaimOptions, Inbox, Outcome};

async fn defer_one(inbox: &Inbox, scope: &str, plan: &[u8; 32])
    -> Result<(), Box<dyn std::error::Error>>
{
    if let Claim::Work(lease) = inbox.claim(scope, "processor", plan, ClaimOptions::default()).await? {
        lease.finish(Outcome::RetryAfter(std::time::Duration::from_secs(30)),
            b"downstream-unavailable").await?;
    }
    Ok(())
}
```

## Claims and fencing

Claims match an explicit trusted scope and exact 32-byte plan identity. There is no latest-plan
fallback. The caller must load/verify the corresponding historical assets before admitting work.
`FOR UPDATE SKIP LOCKED` selects at most one due row ordered by due time and ID, without waiting
for other workers' row locks. This ordering does not guarantee clinical event or completion order.
See [PostgreSQL locking](https://www.postgresql.org/docs/14/sql-select.html#SQL-FOR-UPDATE-SHARE).

`Claim::Empty` means this poll returned no work; locked or differently versioned work may remain.
`Claim::Work` carries a private lease bound to its originating pool, message ID and monotonically
increasing generation. Its payload is shared with `Arc`; `Message::from_shared` can index those
bytes without another source copy. The claim checks payload length and SHA-256 before returning
work. Original wire bytes and the selected plan are never replaced by a retry.

The first claim pins `max_attempts` (1–1000, default 5). Later calls cannot reset that budget by
passing another limit. The lease duration is 1 ms–1 h, default 60 s. PostgreSQL wall-clock time
controls due work and expiry; client timestamps and transaction-start `now()` do not authorize
renewal. Database clock changes can affect expiry, so deployments must maintain their clock.
See [current time functions](https://www.postgresql.org/docs/14/functions-datetime.html#FUNCTIONS-DATETIME-CURRENT).

`Lease::renew` extends the active deadline without shortening it. It cannot resurrect an expired
or superseded generation. Renewal/finalization lock the inbox row, then check current database
time after that lock wait. A finish operation checks the token and deadline again at its final
state update. Reclaimed, expired and already finished handles return `Error::LeaseLost`.

All mutations use the same READ COMMITTED, statement-timeout and local-WAL durability checks as
custody. Claim, generation, attempt record and state transition commit together. A failed COMMIT
returns an error. Unknown commit outcomes require reconciliation; do not blindly repeat external
effects. Leases are cooperative ownership of inbox metadata, not authentication or distributed
locks for arbitrary targets.

## Outcomes and recovery

| Operation/outcome | Result |
|---|---|
| Claim ready/due retry | Increment generation; create an attempt; enter `processing` |
| Claim expired work | Finalize the old attempt as `expired`, then create a new fenced generation |
| Expired work with exhausted budget | Quarantine it without creating another attempt; return `Claim::Quarantined` |
| `Outcome::Completed` | Record completion of the host's processing step and enter `completed` |
| `Outcome::RetryAfter` | Schedule using database time; enter `retry_scheduled`, or quarantine when the pinned budget is exhausted |
| `Outcome::Quarantine` | Record the report and enter `quarantined` with reason `requested` |

Retry delay is 1 ms–7 days. Failure classification, exponential backoff, jitter, age limits and polling
backoff are host policy; there is no automatic busy retry loop. An expired last attempt quarantines
when another claim polls it. No background reaper or immediate expiry notification is implied.

`Lease::finish` atomically records the final attempt outcome and inbox state, returning the actual
`State`. The report is opaque bytes capped at 64 KiB; the host defines its format and omission
accounting. Each attempt retains worker, generation, pinned plan, start/deadline/end, outcome,
report and initial commit mode. A live attempt's deadline may be renewed and its outcome finalized
once. Finalized attempts are not rewritten by these APIs; retries append another generation.

Dropping a lease or stopping a worker does not asynchronously release it. Other workers can reclaim
after its durable deadline, including after process/database restart. Generation and history survive
restart. The host should stop new claims during shutdown and either finish or abandon current work
under its drain policy. Keep pool capacity, admission and reports bounded.

## Clinical application boundary

`completed` is an inbox processing outcome. It is **not** a FHIR application receipt and does not
emit an application ACK. These APIs fence only their own inbox/attempt transitions. They cannot
roll back or fence an unrelated database transaction, HTTP write or detached task.

Clinical changes require a sink that atomically validates the fencing token and writes its durable
application receipt with the target changes, or a target-side idempotency/reconciliation contract.
Do not use attempt generation as a new clinical effect identity on retries. That sink, R4 validation,
entity ordering/ownership, reviewed replay/reprocessing and ACK emission history remain unimplemented.
Enhanced AL/NE custody ACKs stay unchanged when work completes, retries or quarantines.

Worker database roles need SELECT plus UPDATE on the inbox work columns, and SELECT/INSERT/UPDATE
on attempts. They need no update privilege on payload, original plan or custody ACK. Authorization
of scopes remains host policy; this adapter does not install tenant RLS or create production roles.

## Verification

On a dedicated database with both migrations applied:

```sh
cargo test --locked --all-features --test work -- --ignored --skip work_recovery_across_postgres_crash
HL7V2_RECOVERY_PHASE=seed cargo test --locked --all-features --test work work_recovery_across_postgres_crash -- --ignored
# Crash and restart only that dedicated PostgreSQL instance, keeping its data directory.
HL7V2_RECOVERY_PHASE=verify cargo test --locked --all-features --test work work_recovery_across_postgres_crash -- --ignored
```

Tests cover parallel claims, plan/scope isolation, locked-row skipping, renewal and terminal handles,
expiry during a real row-lock wait, stale generation rejection, pinned retry budgets, quarantine,
commit refusal with rollback of both outcome/state, and recovery after database restart. Work tests
use synthetic plans and reports; they do not execute FML or write FHIR resources.
