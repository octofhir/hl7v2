#![cfg(feature = "transport")]

use octofhir_hl7v2::{
    ack::{self, Code, Mode, Reply},
    mllp::{
        self,
        transport::{Config, Connection, Error, Phase, ReceiverConfig, serve},
    },
    wire::Message,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Semaphore, mpsc, oneshot},
    time::{advance, pause, resume, timeout},
};
type Result = std::result::Result<(), Box<dyn std::error::Error + Send + Sync>>;

fn request(id: &str) -> std::result::Result<Message, octofhir_hl7v2::wire::Error> {
    Message::parse(format!(
        "MSH|^~\\&|S|S|T|T|20261110120000||ADT^A01^ADT_A01|{id}|T|2.5.1\rPID|1\r"
    ))
}
fn ack(bytes: Arc<[u8]>) -> std::result::Result<Vec<u8>, Error> {
    let request = Message::from_shared(bytes, Default::default())?;
    Ok(ack::build(
        &request,
        Reply {
            code: Code::AA,
            control_id: b"test-response",
            timestamp: b"20261110120000",
            issue: None,
        },
        4096,
    )?)
}

#[tokio::test]
async fn admission_bounds_active_connections_and_waits_for_each_handler_before_replying() -> Result
{
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let permits = Arc::new(Semaphore::new(0));
    let gate = permits.clone();
    let (started, mut starts) = mpsc::channel(8);
    let (closed, mut closes) = mpsc::channel(4);
    let (stop, stopping) = oneshot::channel();
    let server = tokio::spawn(serve(
        listener,
        ReceiverConfig {
            max_connections: 2,
            ..Default::default()
        },
        async {
            let _ = stopping.await;
        },
        move |peer, bytes| {
            let started = started.clone();
            let gate = gate.clone();
            async move {
                started
                    .send(peer)
                    .await
                    .map_err(|e| Error::Handler(Box::new(e)))?;
                gate.acquire()
                    .await
                    .map_err(|e| Error::Handler(Box::new(e)))?
                    .forget();
                Ok(Some(ack(bytes)?))
            }
        },
        move |peer, result| {
            assert!(closed.try_send((peer, result)).is_ok());
        },
    ));
    // A malformed client is reported and closed without stopping healthy connections.
    let mut malformed = TcpStream::connect(address).await?;
    malformed.write_all(b"junk").await?;
    assert!(matches!(
        closes.recv().await,
        Some((_, Err(Error::Frame(mllp::Error::ExpectedStart))))
    ));
    drop(malformed);
    let mut clients = tokio::task::JoinSet::new();
    for client in 0..4 {
        clients.spawn(async move {
            let mut connection = Connection::connect(address, Config::default()).await?;
            for message in 0..2 {
                let request = request(&format!("{client}-{message}"))?;
                assert_eq!(
                    connection.request(&request, Mode::Original).await?.code,
                    Code::AA
                );
            }
            connection.shutdown().await?;
            Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
        });
    }
    let first = starts.recv().await.ok_or("missing first handler")?;
    let second = starts.recv().await.ok_or("missing second handler")?;
    assert_ne!(first, second);
    assert!(
        timeout(Duration::from_millis(20), starts.recv())
            .await
            .is_err()
    );
    // No callback has completed or returned a response; the other connections stay in the backlog.
    assert!(closes.try_recv().is_err());
    permits.add_permits(8);
    while let Some(client) = clients.join_next().await {
        client??;
    }
    for _ in 0..4 {
        closes.recv().await.ok_or("missing connection outcome")?.1?;
    }
    stop.send(()).map_err(|_| "server exited before shutdown")?;
    server.await??;
    Ok(())
}

#[tokio::test]
async fn graceful_shutdown_finishes_current_response_and_discards_unadmitted_frames() -> Result {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let (stop, stopping) = oneshot::channel();
    let (observed, observed_stop) = oneshot::channel();
    let (started, mut starts) = mpsc::channel(2);
    let permits = Arc::new(Semaphore::new(0));
    let gate = permits.clone();
    let server = tokio::spawn(serve(
        listener,
        ReceiverConfig::default(),
        async {
            let _ = stopping.await;
            let _ = observed.send(());
        },
        move |_, bytes| {
            let started = started.clone();
            let gate = gate.clone();
            async move {
                started
                    .send(())
                    .await
                    .map_err(|e| Error::Handler(Box::new(e)))?;
                gate.acquire()
                    .await
                    .map_err(|e| Error::Handler(Box::new(e)))?
                    .forget();
                Ok(Some(ack(bytes)?))
            }
        },
        |_, result| {
            assert!(result.is_ok());
        },
    ));
    let first = request("first")?;
    let mut stream = TcpStream::connect(address).await?;
    let mut input = mllp::encode(first.original(), 4096)?;
    input.extend(mllp::encode(request("second")?.original(), 4096)?);
    stream.write_all(&input).await?;
    starts.recv().await.ok_or("handler was not called")?;
    stop.send(()).map_err(|_| "server exited")?;
    observed_stop.await?;
    permits.add_permits(1);
    let mut client = Connection::new(stream, Config::default())?;
    let response = Message::from_shared(
        client.receive().await?.ok_or("missing ACK")?,
        Default::default(),
    )?;
    assert_eq!(
        ack::correlate(&first, response, Mode::Original)?.code,
        Code::AA
    );
    assert!(client.receive().await?.is_none());
    server.await??;
    assert!(starts.recv().await.is_none());
    Ok(())
}

struct Dropped(Arc<AtomicBool>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn handler_and_drain_deadlines_cancel_work_and_close_without_a_success_ack() -> Result {
    for force_shutdown in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let (stop, stopping) = oneshot::channel();
        let (observed, observed_stop) = oneshot::channel();
        let (started, mut starts) = mpsc::channel(1);
        let (closed, mut closes) = mpsc::channel(1);
        let dropped = Arc::new(AtomicBool::new(false));
        let guard = dropped.clone();
        let server = tokio::spawn(serve(
            listener,
            ReceiverConfig {
                handler_timeout: Duration::from_secs(5),
                shutdown_timeout: Duration::from_secs(1),
                ..Default::default()
            },
            async {
                let _ = stopping.await;
                let _ = observed.send(());
            },
            move |_, _| {
                let started = started.clone();
                let guard = Dropped(guard.clone());
                async move {
                    let _guard = guard;
                    started
                        .send(())
                        .await
                        .map_err(|e| Error::Handler(Box::new(e)))?;
                    std::future::pending().await
                }
            },
            move |_, result| {
                assert!(closed.try_send(result).is_ok());
            },
        ));
        let mut stream = TcpStream::connect(address).await?;
        stream
            .write_all(&mllp::encode(request("waiting")?.original(), 4096)?)
            .await?;
        starts.recv().await.ok_or("missing handler")?;
        pause();
        if force_shutdown {
            stop.send(()).map_err(|_| "server exited")?;
            observed_stop.await?;
            advance(Duration::from_secs(2)).await;
            assert!(matches!(
                server.await?,
                Err(Error::Timeout(Phase::Shutdown))
            ));
            assert!(closes.recv().await.is_none());
        } else {
            advance(Duration::from_secs(6)).await;
            assert!(matches!(
                closes.recv().await,
                Some(Err(Error::Timeout(Phase::Handler)))
            ));
            stop.send(()).map_err(|_| "server exited")?;
            server.await??;
        }
        assert!(dropped.load(Ordering::SeqCst));
        assert_eq!(stream.read(&mut [0; 1]).await?, 0);
        resume();
    }
    Ok(())
}
