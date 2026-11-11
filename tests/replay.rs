#![cfg(feature = "postgres")]

use octofhir_hl7v2::{
    inbox::{AckMetadata, Claim, ClaimOptions, Error, Inbox, Lease, Outcome, ReplayRequest, State},
    wire::Message,
};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
const PLAN: [u8; 32] = [66; 32];
static NEXT_SCOPE: AtomicU64 = AtomicU64::new(0);

async fn fixture() -> Result<(PgPool, Inbox, String, Lease)> {
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&std::env::var("HL7V2_TEST_DATABASE_URL")?)
        .await?;
    let inbox = Inbox::new(pool.clone(), 65536, Duration::from_secs(5))?;
    let scope = format!(
        "replay-{}-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
        NEXT_SCOPE.fetch_add(1, Ordering::Relaxed)
    );
    let lease = quarantine(&inbox, &scope, "original").await?;
    Ok((pool, inbox, scope, lease))
}
async fn quarantine(inbox: &Inbox, scope: &str, id: &str) -> Result<Lease> {
    let mut bytes = format!(
        "MSH|^~\\&|S|S|T|T|20261111||ADT^A01^ADT_A01|{id}|T|2.5.1|||AL|NE\rPID|1||synthetic\r"
    )
    .into_bytes();
    bytes.extend_from_slice(b"Z99|\xff|\x00\r");
    let ack_id = format!("ack-{id}");
    inbox
        .receive(
            scope,
            "test-channel",
            &Message::parse(bytes)?,
            &PLAN,
            AckMetadata {
                control_id: ack_id.as_bytes(),
                timestamp: b"20261111120000",
            },
        )
        .await?;
    let lease = work(
        inbox
            .claim(
                scope,
                "initial",
                &PLAN,
                ClaimOptions {
                    max_attempts: 1,
                    ..Default::default()
                },
            )
            .await?,
    )?;
    assert_eq!(
        lease
            .finish(Outcome::Quarantine, b"original-report")
            .await?,
        State::Quarantined
    );
    Ok(lease)
}
fn work(claim: Claim) -> Result<Lease> {
    match claim {
        Claim::Work(lease) => Ok(lease),
        other => Err(format!("expected work, got {other:?}").into()),
    }
}
fn request(id: &str, message_id: i64, generation: i64) -> ReplayRequest<'_> {
    ReplayRequest {
        request_id: id,
        message_id,
        expected_generation: generation,
        plan: &PLAN,
        additional_attempts: 2,
        operator: "reviewer",
        reason: "restored-pinned-artifacts",
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL with migrations 0001 through 0003"]
async fn replay_preserves_original_history_and_budget_without_reviving_old_lease() -> Result {
    let (pool, inbox, scope, old) = fixture().await?;
    let before = sqlx::query("SELECT payload,plan,acknowledgement FROM hl7v2.inbox WHERE id=$1")
        .bind(old.message_id())
        .fetch_one(&pool)
        .await?;
    let command = request("review-1", old.message_id(), old.generation());
    let id = inbox.replay(&scope, command).await?;
    assert!(matches!(
        old.renew(Duration::from_secs(60)).await,
        Err(Error::LeaseLost)
    ));
    let next = work(
        inbox
            .claim(&scope, "replayed", &PLAN, ClaimOptions::default())
            .await?,
    )?;
    assert_eq!(next.generation(), old.generation() + 1);
    assert_eq!(next.payload(), old.payload());
    assert_eq!(next.plan(), old.plan());
    assert_eq!(inbox.replay(&scope, command).await?, id);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT generation FROM hl7v2.inbox WHERE id=$1")
            .bind(old.message_id())
            .fetch_one(&pool)
            .await?,
        next.generation()
    );
    assert!(matches!(
        old.finish(Outcome::Completed, b"stale").await,
        Err(Error::LeaseLost)
    ));
    assert_eq!(
        next.finish(Outcome::RetryAfter(Duration::from_millis(1)), b"temporary")
            .await?,
        State::RetryScheduled
    );
    sqlx::query(
        "UPDATE hl7v2.inbox SET available_at=clock_timestamp()-interval '1 second' WHERE id=$1",
    )
    .bind(old.message_id())
    .execute(&pool)
    .await?;
    let last = work(
        inbox
            .claim(
                &scope,
                "last",
                &PLAN,
                ClaimOptions {
                    max_attempts: 1000,
                    ..Default::default()
                },
            )
            .await?,
    )?;
    assert_eq!(last.generation(), old.generation() + 2);
    assert_eq!(
        last.finish(Outcome::RetryAfter(Duration::from_secs(1)), b"exhausted")
            .await?,
        State::Quarantined
    );
    assert!(matches!(
        inbox
            .claim(&scope, "no-budget", &PLAN, ClaimOptions::default())
            .await?,
        Claim::Empty
    ));
    let attempts = sqlx::query("SELECT generation,outcome,report,replay_id FROM hl7v2.attempts WHERE message_id=$1 ORDER BY generation")
        .bind(old.message_id()).fetch_all(&pool).await?;
    assert_eq!(attempts.len(), 3);
    assert_eq!(
        attempts[0].try_get::<&[u8], _>("report")?,
        b"original-report"
    );
    assert_eq!(attempts[0].try_get::<Option<i64>, _>("replay_id")?, None);
    for attempt in &attempts[1..] {
        assert_eq!(attempt.try_get::<i64, _>("replay_id")?, id);
    }
    let after = sqlx::query("SELECT payload,plan,acknowledgement FROM hl7v2.inbox WHERE id=$1")
        .bind(old.message_id())
        .fetch_one(&pool)
        .await?;
    for field in ["payload", "plan", "acknowledgement"] {
        assert_eq!(
            after.try_get::<&[u8], _>(field)?,
            before.try_get::<&[u8], _>(field)?
        );
    }
    assert_eq!(inbox.replay(&scope, command).await?, id);
    assert!(matches!(
        inbox
            .replay(
                &scope,
                request("stale-review", old.message_id(), old.generation())
            )
            .await,
        Err(Error::ReplayRejected)
    ));
    let reviewed = inbox
        .replay(
            &scope,
            request("review-2", old.message_id(), last.generation()),
        )
        .await?;
    assert_ne!(reviewed, id);
    assert_eq!(inbox.replay(&scope, command).await?, id);
    let continued = work(
        inbox
            .claim(
                &scope,
                "second-reviewed-run",
                &PLAN,
                ClaimOptions::default(),
            )
            .await?,
    )?;
    assert_eq!(continued.generation(), 4);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT replay_id FROM hl7v2.attempts WHERE message_id=$1 AND generation=4"
        )
        .bind(old.message_id())
        .fetch_one(&pool)
        .await?,
        reviewed
    );
    continued.finish(Outcome::Completed, b"done").await?;
    pool.close().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL with migrations 0001 through 0003"]
async fn concurrent_commands_are_idempotent_and_cannot_change_scope_plan_or_completed_work()
-> Result {
    let (pool, inbox, scope, old) = fixture().await?;
    let same_scope = quarantine(&inbox, &scope, "same-scope-other").await?;
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let inbox = inbox.clone();
        let scope = scope.clone();
        let message_id = old.message_id();
        tasks.spawn(async move {
            inbox
                .replay(&scope, request("shared-command", message_id, 1))
                .await
        });
    }
    let mut ids = Vec::new();
    while let Some(id) = tasks.join_next().await {
        ids.push(id??);
    }
    assert_eq!(ids.len(), 8);
    assert!(ids.iter().all(|id| *id == ids[0]));
    assert!(matches!(
        inbox
            .replay(
                &scope,
                request("shared-command", same_scope.message_id(), 1)
            )
            .await,
        Err(Error::ReplayConflict)
    ));
    let second = quarantine(&inbox, &format!("{scope}-other"), "other").await?;
    assert!(matches!(
        inbox
            .replay(&scope, request("wrong-scope", second.message_id(), 1))
            .await,
        Err(Error::ReplayRejected)
    ));
    let duplicate = request("shared-command", old.message_id(), 1);
    assert!(matches!(
        inbox
            .replay(
                &scope,
                ReplayRequest {
                    reason: "different-review",
                    ..duplicate
                }
            )
            .await,
        Err(Error::ReplayConflict)
    ));
    assert!(matches!(
        inbox
            .replay(&scope, request("second-review", old.message_id(), 1))
            .await,
        Err(Error::ReplayRejected)
    ));
    assert!(matches!(
        inbox
            .replay(
                &format!("{scope}-other"),
                ReplayRequest {
                    request_id: "wrong-plan",
                    plan: &[99; 32],
                    ..request("unused", second.message_id(), 1)
                }
            )
            .await,
        Err(Error::ReplayRejected)
    ));
    let lease = work(
        inbox
            .claim(&scope, "complete", &PLAN, ClaimOptions::default())
            .await?,
    )?;
    lease.finish(Outcome::Completed, b"done").await?;
    assert!(matches!(
        inbox
            .replay(
                &scope,
                request("completed-review", lease.message_id(), lease.generation())
            )
            .await,
        Err(Error::ReplayRejected)
    ));
    assert_eq!(inbox.replay(&scope, duplicate).await?, ids[0]);
    pool.close().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL with migrations 0001 through 0003"]
async fn replay_audit_and_scheduling_roll_back_together_when_commit_is_refused() -> Result {
    let (pool, inbox, scope, old) = fixture().await?;
    sqlx::raw_sql("CREATE OR REPLACE FUNCTION hl7v2.test_fail_replay() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN IF NEW.operator='test-commit-refusal' THEN RAISE EXCEPTION 'injected replay commit refusal'; END IF; RETURN NEW; END $$;
        DROP TRIGGER IF EXISTS test_fail_replay ON hl7v2.replays;
        CREATE CONSTRAINT TRIGGER test_fail_replay AFTER INSERT ON hl7v2.replays
        DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION hl7v2.test_fail_replay();")
        .execute(&pool).await?;
    let command = ReplayRequest {
        operator: "test-commit-refusal",
        ..request("atomic-review", old.message_id(), 1)
    };
    assert!(matches!(
        inbox.replay(&scope, command).await,
        Err(Error::Database(_))
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM hl7v2.replays WHERE scope=$1")
            .bind(&scope)
            .fetch_one(&pool)
            .await?,
        0
    );
    let row =
        sqlx::query("SELECT state,attempt_limit,generation,replay_id FROM hl7v2.inbox WHERE id=$1")
            .bind(old.message_id())
            .fetch_one(&pool)
            .await?;
    assert_eq!(row.try_get::<&str, _>("state")?, "quarantined");
    assert_eq!(row.try_get::<i64, _>("attempt_limit")?, 1);
    assert_eq!(row.try_get::<i64, _>("generation")?, 1);
    assert_eq!(row.try_get::<Option<i64>, _>("replay_id")?, None);
    sqlx::raw_sql(
        "DROP TRIGGER test_fail_replay ON hl7v2.replays; DROP FUNCTION hl7v2.test_fail_replay();",
    )
    .execute(&pool)
    .await?;
    inbox.replay(&scope, command).await?;
    pool.close().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL with migrations 0001 through 0003"]
async fn generation_continues_past_one_thousand_without_rewriting_attempts() -> Result {
    let (pool, inbox, scope, old) = fixture().await?;
    // A long-lived message can have several reviewed runs; seed their retained history compactly.
    sqlx::query("INSERT INTO hl7v2.attempts(message_id,generation,worker,plan,lease_until,ended_at,outcome,commit_mode)
        SELECT id,n,'historical-fixture',plan,clock_timestamp(),clock_timestamp(),'quarantined','on'
        FROM hl7v2.inbox CROSS JOIN generate_series(2,1001) n WHERE id=$1")
        .bind(old.message_id()).execute(&pool).await?;
    sqlx::query("UPDATE hl7v2.inbox SET generation=1001, attempt_limit=1001 WHERE id=$1")
        .bind(old.message_id())
        .execute(&pool)
        .await?;
    let id = inbox
        .replay(
            &scope,
            ReplayRequest {
                additional_attempts: 1,
                ..request("long-history", old.message_id(), 1001)
            },
        )
        .await?;
    let next = work(
        inbox
            .claim(&scope, "continued", &PLAN, ClaimOptions::default())
            .await?,
    )?;
    assert_eq!(next.generation(), 1002);
    next.finish(Outcome::Completed, b"done").await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM hl7v2.attempts WHERE message_id=$1")
            .bind(old.message_id())
            .fetch_one(&pool)
            .await?,
        1002
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT replay_id FROM hl7v2.attempts WHERE message_id=$1 AND generation=1002"
        )
        .bind(old.message_id())
        .fetch_one(&pool)
        .await?,
        id
    );
    pool.close().await;
    Ok(())
}

#[tokio::test]
#[ignore = "run seed, crash/restart dedicated PostgreSQL, then verify via HL7V2_RECOVERY_PHASE"]
async fn replay_recovery_across_postgres_crash() -> Result {
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&std::env::var("HL7V2_TEST_DATABASE_URL")?)
        .await?;
    let inbox = Inbox::new(pool.clone(), 65536, Duration::from_secs(5))?;
    let scope = "replay-crash-recovery-v1";
    match std::env::var("HL7V2_RECOVERY_PHASE")?.as_str() {
        "seed" => {
            let old = quarantine(&inbox, scope, "replay-crash").await?;
            inbox
                .replay(scope, request("durable-command", old.message_id(), 1))
                .await?;
        }
        "verify" => {
            let stored = sqlx::query(
                "SELECT id,message_id,requested_at<pg_postmaster_start_time() AS restarted
                FROM hl7v2.replays WHERE scope=$1 AND request_id='durable-command'",
            )
            .bind(scope)
            .fetch_one(&pool)
            .await?;
            assert!(stored.try_get::<bool, _>("restarted")?);
            let id: i64 = stored.try_get("id")?;
            let message_id = stored.try_get("message_id")?;
            assert_eq!(
                inbox
                    .replay(scope, request("durable-command", message_id, 1))
                    .await?,
                id
            );
            assert_eq!(
                sqlx::query_scalar::<_, i64>("SELECT count(*) FROM hl7v2.replays WHERE scope=$1")
                    .bind(scope)
                    .fetch_one(&pool)
                    .await?,
                1
            );
            let lease = work(
                inbox
                    .claim(scope, "after-crash", &PLAN, ClaimOptions::default())
                    .await?,
            )?;
            assert_eq!(lease.generation(), 2);
            assert_eq!(lease.plan(), &PLAN);
            lease.finish(Outcome::Completed, b"recovered").await?;
        }
        _ => return Err("invalid recovery phase".into()),
    }
    pool.close().await;
    Ok(())
}
