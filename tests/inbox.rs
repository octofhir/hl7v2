#![cfg(feature = "postgres")]

use octofhir_hl7v2::{
    ack::{self, Code, Mode},
    inbox::{AckMetadata, Disposition, Error, Inbox},
    wire::Message,
};
use sha2::{Digest, Sha256};
use sqlx::{
    PgPool, Row,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

async fn pool() -> Result<PgPool> {
    Ok(PgPoolOptions::new()
        .max_connections(8)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&std::env::var("HL7V2_TEST_DATABASE_URL")?)
        .await?)
}
fn inbox(pool: &PgPool) -> std::result::Result<Inbox, Error> {
    Inbox::new(pool.clone(), 65536, Duration::from_secs(5))
}
fn scope(name: &str) -> Result<String> {
    Ok(format!(
        "{name}-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    ))
}
fn message(
    id: &str,
    marker: &str,
    processing: &str,
) -> std::result::Result<Message, octofhir_hl7v2::wire::Error> {
    Message::parse(format!(
        "MSH|^~\\&|SOURCE|S|TARGET|T|20261111120000||ADT^A01^ADT_A01|{id}|{processing}|2.5.1|||AL|NE\rPID|1||{marker}\r"
    ))
}
fn metadata(id: &[u8]) -> AckMetadata<'_> {
    AckMetadata {
        control_id: id,
        timestamp: b"20261111120000+0300",
    }
}
fn code(request: &Message, bytes: &[u8]) -> Result<Code> {
    Ok(ack::correlate(request, Message::parse(bytes)?, Mode::EnhancedAcceptOnly)?.code)
}

#[tokio::test]
#[ignore = "requires a dedicated migrated PostgreSQL database via HL7V2_TEST_DATABASE_URL"]
async fn concurrent_receipts_pin_one_original_plan_and_replay_after_reconnection() -> Result {
    let pool = pool().await?;
    let inbox = inbox(&pool)?;
    let scope = scope("race")?;
    let base = message("m\\F\\1", "synthetic", "T")?;
    let request = Message::parse([base.original(), b"Z99|\xff|\x00\r"].concat())?;
    let mut tasks = tokio::task::JoinSet::new();
    for n in 0..16u8 {
        let inbox = inbox.clone();
        let scope = scope.clone();
        let request = request.clone();
        tasks.spawn(async move {
            let id = format!("candidate-{n}");
            inbox
                .receive(&scope, &id, &request, &[n; 32], metadata(id.as_bytes()))
                .await
        });
    }
    let mut receipts = Vec::new();
    while let Some(receipt) = tasks.join_next().await {
        receipts.push(receipt??);
    }
    let winner = receipts
        .iter()
        .find(|r| r.disposition == Disposition::New)
        .ok_or("no winner")?;
    assert_eq!(
        receipts
            .iter()
            .filter(|r| r.disposition == Disposition::New)
            .count(),
        1
    );
    for receipt in &receipts {
        assert_eq!(receipt.message_id, winner.message_id);
        assert_eq!(receipt.plan, winner.plan);
        assert_eq!(receipt.acknowledgement, winner.acknowledgement);
        assert_eq!(code(&request, &receipt.acknowledgement)?, Code::CA);
    }
    let stored =
        sqlx::query("SELECT payload, sha256, plan, acknowledgement FROM hl7v2.inbox WHERE id=$1")
            .bind(winner.message_id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(stored.try_get::<&[u8], _>("payload")?, request.original());
    assert_eq!(
        stored.try_get::<&[u8], _>("sha256")?,
        Sha256::digest(request.original()).as_slice()
    );
    assert_eq!(stored.try_get::<&[u8], _>("plan")?, winner.plan);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM hl7v2.deliveries WHERE message_id=$1")
            .bind(winner.message_id)
            .fetch_one(&pool)
            .await?,
        16
    );
    pool.close().await;
    let reopened = self::pool().await?;
    let replay = self::inbox(&reopened)?
        .receive(
            &scope,
            "reconnected",
            &request,
            &[255; 32],
            metadata(b"ignored-new-id"),
        )
        .await?;
    assert_eq!(replay.disposition, Disposition::Duplicate);
    assert_eq!(replay.plan, winner.plan);
    assert_eq!(replay.acknowledgement, winner.acknowledgement);
    reopened.close().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a dedicated migrated PostgreSQL database via HL7V2_TEST_DATABASE_URL"]
async fn conflicts_and_commit_failures_preserve_original_and_never_return_acceptance() -> Result {
    let pool = pool().await?;
    let store = inbox(&pool)?;
    let scope = scope("conflict")?;
    let original = message("id-1", "first", "T")?;
    let first = store
        .receive(&scope, "first", &original, &[1; 32], metadata(b"first-ack"))
        .await?;
    let changed = message("id-1", "changed", "T")?;
    let conflict = store
        .receive(&scope, "second", &changed, &[2; 32], metadata(b"error-ack"))
        .await?;
    assert_eq!(conflict.disposition, Disposition::Conflict);
    assert_eq!(conflict.message_id, first.message_id);
    assert_eq!(conflict.plan, [1; 32]);
    assert_eq!(code(&changed, &conflict.acknowledgement)?, Code::CE);
    assert_ne!(conflict.acknowledgement, first.acknowledgement);
    let stored: Vec<u8> = sqlx::query_scalar("SELECT payload FROM hl7v2.inbox WHERE id=$1")
        .bind(first.message_id)
        .fetch_one(&pool)
        .await?;
    assert_eq!(stored, original.original());
    let delivery: Vec<u8> = sqlx::query_scalar("SELECT payload FROM hl7v2.deliveries WHERE id=$1")
        .bind(conflict.delivery_id)
        .fetch_one(&pool)
        .await?;
    assert_eq!(delivery, changed.original());
    assert_eq!(
        store
            .receive(
                &format!("{scope}-other"),
                "other",
                &changed,
                &[2; 32],
                metadata(b"other")
            )
            .await?
            .disposition,
        Disposition::New
    );
    assert_eq!(
        store
            .receive(
                &scope,
                "production",
                &message("id-1", "production", "P")?,
                &[2; 32],
                metadata(b"prod")
            )
            .await?
            .disposition,
        Disposition::New
    );

    // Fail at COMMIT, after both inserts have succeeded, to exercise the actual custody boundary.
    sqlx::raw_sql("CREATE OR REPLACE FUNCTION hl7v2.test_fail_commit() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN IF NEW.channel = 'test-deferred-failure' THEN RAISE EXCEPTION 'injected commit refusal'; END IF;
        RETURN NEW; END $$;
        DROP TRIGGER IF EXISTS test_fail_commit ON hl7v2.deliveries;
        CREATE CONSTRAINT TRIGGER test_fail_commit AFTER INSERT ON hl7v2.deliveries
        DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION hl7v2.test_fail_commit();")
        .execute(&pool).await?;
    let failed = store
        .receive(
            &scope,
            "test-deferred-failure",
            &message("refused", "refused", "T")?,
            &[3; 32],
            metadata(b"not-returned"),
        )
        .await;
    assert!(matches!(failed, Err(Error::Database(_))));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM hl7v2.inbox WHERE scope=$1 AND control_id=$2"
        )
        .bind(&scope)
        .bind(b"refused".as_slice())
        .fetch_one(&pool)
        .await?,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM hl7v2.deliveries WHERE channel='test-deferred-failure'"
        )
        .fetch_one(&pool)
        .await?,
        0
    );
    sqlx::raw_sql("DROP TRIGGER test_fail_commit ON hl7v2.deliveries; DROP FUNCTION hl7v2.test_fail_commit();").execute(&pool).await?;

    let options: PgConnectOptions = std::env::var("HL7V2_TEST_DATABASE_URL")?.parse()?;
    let unsafe_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options.options([("synchronous_commit", "off")]))
        .await?;
    assert!(matches!(
        inbox(&unsafe_pool)?
            .receive(
                &scope,
                "async-wal",
                &message("unsafe", "unsafe", "T")?,
                &[4; 32],
                metadata(b"no-ack")
            )
            .await,
        Err(Error::Durability)
    ));
    unsafe_pool.close().await;
    pool.close().await;
    Ok(())
}

#[cfg(feature = "transport")]
#[tokio::test]
#[ignore = "requires a dedicated migrated PostgreSQL database via HL7V2_TEST_DATABASE_URL"]
async fn tcp_retry_replays_committed_ack_and_incomplete_frame_creates_no_delivery() -> Result {
    use octofhir_hl7v2::mllp::transport::{self, Config, Connection, ReceiverConfig};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::{
        io::AsyncWriteExt,
        net::{TcpListener, TcpStream},
        sync::{Semaphore, mpsc, oneshot},
    };
    let pool = pool().await?;
    let store = inbox(&pool)?;
    let scope = scope("tcp")?;
    let handler_scope = scope.clone();
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let (stop, stopping) = oneshot::channel();
    let (committed, mut commits) = mpsc::channel(4);
    let (closed, mut closes) = mpsc::channel(4);
    let permits = Arc::new(Semaphore::new(0));
    let gate = permits.clone();
    let sequence = AtomicUsize::new(0);
    let server = tokio::spawn(transport::serve(
        listener,
        ReceiverConfig::default(),
        async {
            let _ = stopping.await;
        },
        move |peer, bytes| {
            let store = store.clone();
            let scope = handler_scope.clone();
            let committed = committed.clone();
            let gate = gate.clone();
            let id = format!("tcp-{}", sequence.fetch_add(1, Ordering::SeqCst));
            async move {
                let message = Message::from_shared(bytes, Default::default())?;
                let receipt = store
                    .receive(
                        &scope,
                        &peer.to_string(),
                        &message,
                        &[9; 32],
                        metadata(id.as_bytes()),
                    )
                    .await
                    .map_err(|e| transport::Error::Handler(Box::new(e)))?;
                committed
                    .send((receipt.message_id, receipt.disposition))
                    .await
                    .map_err(|e| transport::Error::Handler(Box::new(e)))?;
                gate.acquire()
                    .await
                    .map_err(|e| transport::Error::Handler(Box::new(e)))?
                    .forget();
                Ok(Some(receipt.acknowledgement))
            }
        },
        move |_, outcome| {
            assert!(closed.try_send(outcome).is_ok());
        },
    ));
    let request = message("tcp-id", "synthetic", "T")?;
    let mut lost = Connection::connect(address, Config::default()).await?;
    lost.send(request.original()).await?;
    let (id, disposition) = commits.recv().await.ok_or("missing commit")?;
    assert_eq!(disposition, Disposition::New);
    drop(lost); // Disconnect after commit but before the handler releases the response.
    permits.add_permits(1);
    let _ = closes.recv().await.ok_or("missing close")?;
    let stored: Vec<u8> = sqlx::query_scalar("SELECT acknowledgement FROM hl7v2.inbox WHERE id=$1")
        .bind(id)
        .fetch_one(&pool)
        .await?;
    let mut retry = Connection::connect(address, Config::default()).await?;
    permits.add_permits(1);
    let response = retry.request(&request, Mode::EnhancedAcceptOnly).await?;
    assert_eq!(response.code, Code::CA);
    assert_eq!(response.message.original(), stored);
    assert_eq!(commits.recv().await, Some((id, Disposition::Duplicate)));
    retry.shutdown().await?;
    let _ = closes.recv().await.ok_or("missing retry close")?;
    let mut partial = TcpStream::connect(address).await?;
    partial.write_all(b"\x0bMSH|^~\\&|incomplete\x1c").await?;
    drop(partial);
    assert!(closes.recv().await.ok_or("missing partial close")?.is_err());
    assert!(commits.try_recv().is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM hl7v2.deliveries WHERE message_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await?,
        2
    );
    stop.send(()).map_err(|_| "server already stopped")?;
    server.await??;
    pool.close().await;
    Ok(())
}

#[tokio::test]
#[ignore = "run explicitly with HL7V2_RECOVERY_PHASE=seed, crash/restart the dedicated DB, then phase=verify"]
async fn recovery_across_postgres_crash() -> Result {
    let pool = pool().await?;
    let phase = std::env::var("HL7V2_RECOVERY_PHASE")?;
    if !matches!(phase.as_str(), "seed" | "verify") {
        return Err("invalid recovery phase".into());
    }
    let request = message("crash-id", "synthetic", "T")?;
    let receipt = inbox(&pool)?
        .receive(
            "crash-recovery-v1",
            "recovery-test",
            &request,
            if phase == "seed" {
                &[11; 32]
            } else {
                &[22; 32]
            },
            metadata(if phase == "seed" {
                b"before-crash"
            } else {
                b"after-crash"
            }),
        )
        .await?;
    assert_eq!(
        receipt.disposition,
        if phase == "seed" {
            Disposition::New
        } else {
            Disposition::Duplicate
        }
    );
    assert_eq!(receipt.plan, [11; 32]);
    if phase == "verify" {
        assert!(
            sqlx::query_scalar::<_, bool>(
                "SELECT received_at < pg_postmaster_start_time() FROM hl7v2.inbox WHERE id=$1"
            )
            .bind(receipt.message_id)
            .fetch_one(&pool)
            .await?,
            "database was not restarted after the checkpoint"
        );
    }
    assert_eq!(code(&request, &receipt.acknowledgement)?, Code::CA);
    assert_eq!(
        Message::parse(&receipt.acknowledgement)?
            .value(&[1, 10])
            .decode()?
            .as_ref(),
        b"before-crash"
    );
    pool.close().await;
    Ok(())
}
