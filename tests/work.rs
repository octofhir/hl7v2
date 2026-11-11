#![cfg(feature = "postgres")]

use octofhir_hl7v2::{
    inbox::{AckMetadata, Claim, ClaimOptions, Error, Inbox, Lease, Outcome, State},
    wire::Message,
};
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::{
    collections::HashSet,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
const PLAN: [u8; 32] = [42; 32];
static NEXT_SCOPE: AtomicU64 = AtomicU64::new(0);

async fn fixture(count: usize) -> Result<(PgPool, Inbox, String)> {
    let scope = format!(
        "work-{}-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
        NEXT_SCOPE.fetch_add(1, Ordering::Relaxed)
    );
    let options: PgConnectOptions = std::env::var("HL7V2_TEST_DATABASE_URL")?.parse()?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect_with(options.application_name(&scope))
        .await?;
    let inbox = Inbox::new(pool.clone(), 65536, Duration::from_secs(5))?;
    for n in 0..count {
        insert(&inbox, &scope, &format!("work-{n}")).await?;
    }
    Ok((pool, inbox, scope))
}
async fn insert(inbox: &Inbox, scope: &str, id: &str) -> Result<i64> {
    let message = Message::parse(format!(
        "MSH|^~\\&|S|S|T|T|20261111||ADT^A01^ADT_A01|{id}|T|2.5.1|||AL|NE\rPID|1||synthetic\r"
    ))?;
    let ack_id = format!("ack-{id}");
    Ok(inbox
        .receive(
            scope,
            "test-channel",
            &message,
            &PLAN,
            AckMetadata {
                control_id: ack_id.as_bytes(),
                timestamp: b"20261111120000",
            },
        )
        .await?
        .message_id)
}
fn work(claim: Claim) -> Result<Lease> {
    match claim {
        Claim::Work(lease) => Ok(lease),
        other => Err(format!("expected work, got {other:?}").into()),
    }
}
async fn due(pool: &PgPool, id: i64) -> Result {
    sqlx::query(
        "WITH due AS (UPDATE hl7v2.inbox SET available_at=clock_timestamp()-interval '1 second'
        WHERE id=$1 RETURNING id,generation,available_at)
        UPDATE hl7v2.attempts a SET lease_until=due.available_at FROM due
        WHERE a.message_id=due.id AND a.generation=due.generation AND a.outcome IS NULL",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL with migrations 0001 through 0003"]
async fn concurrent_claims_are_scoped_plan_exact_and_skip_locked_rows() -> Result {
    let (pool, inbox, scope) = fixture(8).await?;
    assert!(matches!(
        inbox
            .claim(&scope, "wrong-plan", &[99; 32], ClaimOptions::default())
            .await?,
        Claim::Empty
    ));
    assert!(matches!(
        inbox
            .claim("another-scope", "worker", &PLAN, ClaimOptions::default())
            .await?,
        Claim::Empty
    ));
    let mut tasks = tokio::task::JoinSet::new();
    for n in 0..12 {
        let inbox = inbox.clone();
        let scope = scope.clone();
        tasks.spawn(async move {
            inbox
                .claim(
                    &scope,
                    &format!("worker-{n}"),
                    &PLAN,
                    ClaimOptions::default(),
                )
                .await
        });
    }
    let mut ids = HashSet::new();
    while let Some(claim) = tasks.join_next().await {
        if let Claim::Work(lease) = claim?? {
            assert!(ids.insert(lease.message_id()));
            assert_eq!(lease.generation(), 1);
            assert_eq!(lease.plan(), &PLAN);
            let message = Message::from_shared(lease.payload().clone(), Default::default())?;
            assert_eq!(message.original().as_ptr(), lease.payload().as_ptr());
        }
    }
    assert_eq!(ids.len(), 8);
    let first = insert(&inbox, &scope, "locked").await?;
    let second = insert(&inbox, &scope, "unlocked").await?;
    let mut held = pool.begin().await?;
    sqlx::query("SELECT id FROM hl7v2.inbox WHERE id=$1 FOR UPDATE")
        .bind(first)
        .execute(&mut *held)
        .await?;
    let lease = work(
        tokio::time::timeout(
            Duration::from_secs(1),
            inbox.claim(&scope, "skip-lock", &PLAN, ClaimOptions::default()),
        )
        .await??,
    )?;
    assert_eq!(lease.message_id(), second);
    lease.renew(Duration::from_secs(60)).await?;
    assert_eq!(
        lease.finish(Outcome::Completed, b"converted").await?,
        State::Completed
    );
    assert!(matches!(
        lease.renew(Duration::from_secs(60)).await,
        Err(Error::LeaseLost)
    ));
    held.rollback().await?;
    pool.close().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL with migrations 0001 through 0003"]
async fn expiry_is_checked_after_lock_wait_and_stale_generation_cannot_finish() -> Result {
    let (pool, inbox, scope) = fixture(1).await?;
    let options = ClaimOptions {
        lease_for: Duration::from_secs(2),
        max_attempts: 2,
    };
    let lease = work(inbox.claim(&scope, "first", &PLAN, options).await?)?;
    let mut held = pool.begin().await?;
    sqlx::query("SELECT id FROM hl7v2.inbox WHERE id=$1 FOR UPDATE")
        .bind(lease.message_id())
        .execute(&mut *held)
        .await?;
    let old = lease.clone();
    let renewal = tokio::spawn(async move { old.renew(Duration::from_secs(60)).await });
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE application_name=$1 AND wait_event_type='Lock'")
                .bind(&scope).fetch_one(&pool).await?;
            if waiting > 0 { break Ok::<(), sqlx::Error>(()); }
            tokio::task::yield_now().await;
        }
    }).await??;
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT available_at>clock_timestamp() FROM hl7v2.inbox WHERE id=$1"
        )
        .bind(lease.message_id())
        .fetch_one(&mut *held)
        .await?
    );
    sqlx::query("SELECT pg_sleep(2.1)")
        .execute(&mut *held)
        .await?;
    held.commit().await?;
    assert!(matches!(renewal.await?, Err(Error::LeaseLost)));
    assert!(matches!(
        lease.finish(Outcome::Completed, b"late").await,
        Err(Error::LeaseLost)
    ));
    let next = work(
        inbox
            .claim(
                &scope,
                "replacement",
                &PLAN,
                ClaimOptions {
                    max_attempts: 1000,
                    ..Default::default()
                },
            )
            .await?,
    )?;
    assert_eq!(next.message_id(), lease.message_id());
    assert_eq!(next.generation(), lease.generation() + 1);
    assert_eq!(next.payload(), lease.payload());
    assert!(matches!(
        lease.finish(Outcome::Quarantine, b"stale").await,
        Err(Error::LeaseLost)
    ));
    assert_eq!(
        next.finish(Outcome::Completed, b"recovered").await?,
        State::Completed
    );
    let history: Vec<String> = sqlx::query_scalar(
        "SELECT outcome FROM hl7v2.attempts WHERE message_id=$1 ORDER BY generation",
    )
    .bind(lease.message_id())
    .fetch_all(&pool)
    .await?;
    assert_eq!(history, ["expired", "completed"]);
    pool.close().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL with migrations 0001 through 0003"]
async fn retry_budget_is_pinned_and_expired_last_attempt_is_quarantined() -> Result {
    let (pool, inbox, scope) = fixture(1).await?;
    let first = work(
        inbox
            .claim(
                &scope,
                "first",
                &PLAN,
                ClaimOptions {
                    max_attempts: 2,
                    ..Default::default()
                },
            )
            .await?,
    )?;
    assert_eq!(
        first
            .finish(
                Outcome::RetryAfter(Duration::from_secs(60)),
                b"temporary failure"
            )
            .await?,
        State::RetryScheduled
    );
    assert!(matches!(
        inbox
            .claim(&scope, "too-early", &PLAN, ClaimOptions::default())
            .await?,
        Claim::Empty
    ));
    due(&pool, first.message_id()).await?;
    let second = work(
        inbox
            .claim(
                &scope,
                "second",
                &PLAN,
                ClaimOptions {
                    max_attempts: 1000,
                    ..Default::default()
                },
            )
            .await?,
    )?;
    assert_eq!(
        second
            .finish(
                Outcome::RetryAfter(Duration::from_secs(1)),
                b"still failing"
            )
            .await?,
        State::Quarantined
    );
    assert!(matches!(
        inbox
            .claim(&scope, "terminal", &PLAN, ClaimOptions::default())
            .await?,
        Claim::Empty
    ));
    let history: Vec<String> = sqlx::query_scalar(
        "SELECT outcome FROM hl7v2.attempts WHERE message_id=$1 ORDER BY generation",
    )
    .bind(first.message_id())
    .fetch_all(&pool)
    .await?;
    assert_eq!(history, ["retry_scheduled", "retry_limit"]);
    let id = insert(&inbox, &scope, "abandoned").await?;
    let abandoned = work(
        inbox
            .claim(
                &scope,
                "abandoned",
                &PLAN,
                ClaimOptions {
                    max_attempts: 1,
                    ..Default::default()
                },
            )
            .await?,
    )?;
    assert_eq!(abandoned.message_id(), id);
    due(&pool, id).await?;
    assert!(
        matches!(inbox.claim(&scope, "cannot-reset-budget", &PLAN, ClaimOptions::default()).await?, Claim::Quarantined { message_id } if message_id == id)
    );
    assert!(matches!(
        abandoned.renew(Duration::from_secs(60)).await,
        Err(Error::LeaseLost)
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM hl7v2.attempts WHERE message_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await?,
        1
    );
    pool.close().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL with migrations 0001 through 0003"]
async fn failed_completion_commit_keeps_attempt_active_and_outcome_atomic() -> Result {
    let (pool, inbox, scope) = fixture(1).await?;
    let lease = work(
        inbox
            .claim(
                &scope,
                "test-commit-refusal",
                &PLAN,
                ClaimOptions::default(),
            )
            .await?,
    )?;
    sqlx::raw_sql("CREATE OR REPLACE FUNCTION hl7v2.test_fail_attempt() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN IF NEW.worker='test-commit-refusal' AND NEW.outcome='completed' THEN RAISE EXCEPTION 'injected work commit refusal'; END IF;
        RETURN NEW; END $$;
        DROP TRIGGER IF EXISTS test_fail_attempt ON hl7v2.attempts;
        CREATE CONSTRAINT TRIGGER test_fail_attempt AFTER UPDATE ON hl7v2.attempts
        DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION hl7v2.test_fail_attempt();")
        .execute(&pool).await?;
    assert!(matches!(
        lease.finish(Outcome::Completed, b"not committed").await,
        Err(Error::Database(_))
    ));
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT state FROM hl7v2.inbox WHERE id=$1")
            .bind(lease.message_id())
            .fetch_one(&pool)
            .await?,
        "processing"
    );
    assert!(sqlx::query_scalar::<_, bool>("SELECT outcome IS NULL AND ended_at IS NULL AND report=''::bytea FROM hl7v2.attempts WHERE message_id=$1")
        .bind(lease.message_id()).fetch_one(&pool).await?);
    sqlx::raw_sql("DROP TRIGGER test_fail_attempt ON hl7v2.attempts; DROP FUNCTION hl7v2.test_fail_attempt();").execute(&pool).await?;
    assert_eq!(
        lease
            .finish(Outcome::Quarantine, b"requires operator")
            .await?,
        State::Quarantined
    );
    pool.close().await;
    Ok(())
}

#[tokio::test]
#[ignore = "run seed, crash/restart dedicated PostgreSQL, then verify via HL7V2_RECOVERY_PHASE"]
async fn work_recovery_across_postgres_crash() -> Result {
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&std::env::var("HL7V2_TEST_DATABASE_URL")?)
        .await?;
    let inbox = Inbox::new(pool.clone(), 65536, Duration::from_secs(5))?;
    let scope = "work-crash-recovery-v1";
    let phase = std::env::var("HL7V2_RECOVERY_PHASE")?;
    match phase.as_str() {
        "seed" => {
            insert(&inbox, scope, "work-crash").await?;
            let lease = work(
                inbox
                    .claim(
                        scope,
                        "before-crash",
                        &PLAN,
                        ClaimOptions {
                            lease_for: Duration::from_secs(2),
                            ..Default::default()
                        },
                    )
                    .await?,
            )?;
            assert_eq!(lease.generation(), 1);
        }
        "verify" => {
            assert!(
                sqlx::query_scalar::<_, bool>(
                    "SELECT received_at<pg_postmaster_start_time() AND generation=1
                FROM hl7v2.inbox WHERE scope=$1"
                )
                .bind(scope)
                .fetch_one(&pool)
                .await?
            );
            sqlx::query("SELECT pg_sleep(greatest(0,extract(epoch FROM (available_at-clock_timestamp())))::double precision)
                FROM hl7v2.inbox WHERE scope=$1").bind(scope).execute(&pool).await?;
            let lease = work(
                inbox
                    .claim(scope, "after-crash", &PLAN, ClaimOptions::default())
                    .await?,
            )?;
            assert_eq!(lease.generation(), 2);
            assert_eq!(lease.plan(), &PLAN);
            assert_eq!(
                lease
                    .finish(Outcome::Completed, b"recovered after database crash")
                    .await?,
                State::Completed
            );
        }
        _ => return Err("invalid recovery phase".into()),
    }
    pool.close().await;
    Ok(())
}
