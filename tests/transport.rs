#![cfg(feature = "transport")]

use octofhir_hl7v2::{
    ack::{self, Code, Mode, Policy, Reply},
    mllp::{
        self,
        transport::{Config, Connection, Error, Phase},
    },
    wire::{Edit, Message},
};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{self, AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    time::{sleep, timeout},
};
type Result = std::result::Result<(), Box<dyn std::error::Error + Send + Sync>>;

fn request(id: &str, enhanced: bool) -> std::result::Result<Message, octofhir_hl7v2::wire::Error> {
    let conditions = if enhanced { "|||AL|NE" } else { "" };
    Message::parse(format!(
        "MSH|^~\\&|SOURCE|S|TARGET|T|20261110||ADT^A01^ADT_A01|{id}|T|2.5.1{conditions}\rPID|1\r"
    ))
}
fn response(request: &Message, code: Code) -> std::result::Result<Vec<u8>, ack::Error> {
    ack::build(
        request,
        Reply {
            code,
            control_id: b"ack-id",
            timestamp: b"20261110120000",
            issue: None,
        },
        4096,
    )
}

#[tokio::test]
async fn persistent_tcp_connections_exchange_original_and_enhanced_acknowledgements() -> Result {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut connection = Connection::new(stream, Config::default())?;
        for _ in 0..2 {
            let bytes = connection.receive().await?.ok_or("unexpected EOF")?;
            let request = Message::from_shared(bytes, Default::default())?;
            let code = if Policy::from_message(&request)? == Policy::Original {
                Code::AA
            } else {
                Code::CA
            };
            connection.send(&response(&request, code)?).await?;
        }
        assert!(connection.receive().await?.is_none());
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    });
    let mut client = Connection::connect(address, Config::default()).await?;
    assert_eq!(
        client
            .request(&request("first", false)?, Mode::Original)
            .await?
            .code,
        Code::AA
    );
    assert_eq!(
        client
            .request(&request("second", true)?, Mode::EnhancedAcceptOnly)
            .await?
            .code,
        Code::CA
    );
    client.shutdown().await?;
    assert!(!client.is_open());
    server.await??;
    Ok(())
}

#[tokio::test]
async fn stream_reads_preserve_fragmentation_coalescence_and_eof() -> Result {
    let (stream, mut peer) = io::duplex(32);
    let payloads = [
        Arc::<[u8]>::from(include_bytes!("fixtures/adt.er7").as_slice()),
        Arc::from(&b""[..]),
        Arc::from(vec![b'x'; 8193]),
    ];
    let mut encoded = Vec::new();
    for payload in &payloads {
        encoded.extend(mllp::encode(payload, 9000)?);
    }
    let writer = tokio::spawn(async move {
        for chunk in encoded.chunks(3) {
            peer.write_all(chunk).await?;
        }
        io::Result::Ok(())
    });
    let mut connection = Connection::new(
        stream,
        Config {
            max_payload: 9000,
            ..Default::default()
        },
    )?;
    for expected in payloads {
        assert_eq!(
            connection.receive().await?.ok_or("missing frame")?,
            expected
        );
    }
    assert!(connection.receive().await?.is_none());
    writer.await??;
    let (stream, mut peer) = io::duplex(32);
    peer.write_all(b"\x0bpartial\x1c").await?;
    drop(peer);
    let mut connection = Connection::new(stream, Config::default())?;
    assert!(matches!(
        connection.receive().await,
        Err(Error::Frame(mllp::Error::Incomplete))
    ));
    assert!(!connection.is_open());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn independent_deadlines_and_external_cancellation_close_the_stream() -> Result {
    let (stream, mut peer) = io::duplex(32);
    let mut connection = Connection::new(stream, Config::default())?;
    assert!(matches!(
        connection.receive().await,
        Err(Error::Timeout(Phase::Idle))
    ));
    assert_eq!(peer.read(&mut [0; 1]).await?, 0);
    let (stream, mut peer) = io::duplex(32);
    let slow = tokio::spawn(async move {
        peer.write_all(b"\x0b").await?;
        for _ in 0..2 {
            sleep(Duration::from_secs(2)).await;
            peer.write_all(b"x").await?;
        }
        sleep(Duration::from_secs(10)).await;
        io::Result::Ok(())
    });
    let mut connection = Connection::new(
        stream,
        Config {
            frame_timeout: Duration::from_secs(5),
            ..Default::default()
        },
    )?;
    assert!(matches!(
        connection.receive().await,
        Err(Error::Timeout(Phase::Frame))
    ));
    assert!(!connection.is_open());
    slow.abort();
    assert!(slow.await.is_err());
    let (stream, _peer) = io::duplex(1);
    let mut connection = Connection::new(stream, Config::default())?;
    assert!(matches!(
        connection.send(b"abc").await,
        Err(Error::Timeout(Phase::Write))
    ));
    assert!(!connection.is_open());
    let (stream, _peer) = io::duplex(4096);
    let mut connection = Connection::new(
        stream,
        Config {
            acknowledgement_timeout: Duration::from_secs(5),
            ..Default::default()
        },
    )?;
    assert!(matches!(
        connection
            .request(&request("timeout", false)?, Mode::Original)
            .await,
        Err(Error::Timeout(Phase::Acknowledgement))
    ));
    assert!(!connection.is_open());
    let (stream, mut peer) = io::duplex(4096);
    let mut connection = Connection::new(stream, Config::default())?;
    assert!(
        timeout(
            Duration::from_secs(1),
            connection.request(&request("cancel", false)?, Mode::Original)
        )
        .await
        .is_err()
    );
    assert!(!connection.is_open());
    assert!(peer.write_all(b"late ACK").await.is_err());
    assert!(matches!(connection.receive().await, Err(Error::Closed)));
    Ok(())
}

#[tokio::test]
async fn duplicate_ack_cannot_complete_another_request_and_invalid_policy_writes_nothing() -> Result
{
    let (stream, peer) = io::duplex(4096);
    let server = tokio::spawn(async move {
        let mut peer = Connection::new(peer, Config::default())?;
        let first = Message::from_shared(
            peer.receive().await?.ok_or("missing request")?,
            Default::default(),
        )?;
        let ack = response(&first, Code::AA)?;
        peer.send(&ack).await?;
        peer.send(&ack).await?;
        peer.receive().await?.ok_or("missing second request")?;
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    });
    let mut connection = Connection::new(stream, Config::default())?;
    connection
        .request(&request("first", false)?, Mode::Original)
        .await?;
    assert!(matches!(
        connection
            .request(&request("second", false)?, Mode::Original)
            .await,
        Err(Error::Ack(ack::Error::Correlation))
    ));
    assert!(!connection.is_open());
    server.await??;
    let (stream, mut peer) = io::duplex(4096);
    let mut connection = Connection::new(stream, Config::default())?;
    let source = request("unsupported", true)?;
    let mut editor = source.edit();
    editor.set(&[1, 15], Edit::Value(b"ER".to_vec()))?;
    assert!(matches!(
        connection
            .request(&editor.finish()?, Mode::EnhancedAcceptOnly)
            .await,
        Err(Error::Ack(ack::Error::Policy))
    ));
    assert!(connection.is_open());
    connection.shutdown().await?;
    assert_eq!(peer.read(&mut [0; 1]).await?, 0);
    Ok(())
}
