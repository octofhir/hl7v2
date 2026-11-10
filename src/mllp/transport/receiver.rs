use super::{Config, Connection, Error, Phase, deadline};
use std::{future::Future, net::SocketAddr, sync::Arc, time::Duration};
use tokio::{net::TcpListener, sync::watch, task::JoinSet, time::timeout_at};

/// Per-listener admission and cooperative handler/drain limits. No message queue is created.
#[derive(Debug, Clone, Copy)]
pub struct ReceiverConfig {
    pub connection: Config,
    pub max_connections: usize,
    pub handler_timeout: Duration,
    pub shutdown_timeout: Duration,
}
impl Default for ReceiverConfig {
    fn default() -> Self {
        Self {
            connection: Config::default(),
            max_connections: 64,
            handler_timeout: Duration::from_secs(30),
            shutdown_timeout: Duration::from_secs(30),
        }
    }
}

/// Serve persistent connections with one awaited handler per message, and no implicit ACK.
///
/// `handler` returns an optional unframed response only after its required processing boundary.
/// `on_close` observes each normally joined connection, including handler and transport errors.
/// Shutdown stops admission/reads and drains current handlers and writes within one deadline.
/// Dropping this future aborts owned tasks; it does not gracefully drain them.
pub async fn serve<H, F, O>(
    listener: TcpListener,
    config: ReceiverConfig,
    shutdown: impl Future<Output = ()>,
    handler: H,
    mut on_close: O,
) -> Result<(), Error>
where
    H: Fn(SocketAddr, Arc<[u8]>) -> F + Send + Sync + 'static,
    F: Future<Output = Result<Option<Vec<u8>>, Error>> + Send + 'static,
    O: FnMut(SocketAddr, Result<(), Error>),
{
    config.connection.validate()?;
    deadline(config.handler_timeout)?;
    deadline(config.shutdown_timeout)?;
    if config.max_connections == 0 {
        return Err(Error::Configuration);
    }
    let handler = Arc::new(handler);
    let (stop, _) = watch::channel(false);
    let mut tasks = JoinSet::new();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            biased;
            _ = &mut shutdown => break,
            Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                let (peer, outcome) = result.map_err(Error::Task)?;
                on_close(peer, outcome);
            }
            accepted = listener.accept(), if tasks.len() < config.max_connections => {
                let (stream, peer) = accepted?;
                stream.set_nodelay(true)?;
                let connection = Connection::new(stream, config.connection)?;
                let handler = handler.clone();
                let stop = stop.subscribe();
                tasks.spawn(async move {
                    let outcome = run(connection, peer, config, stop, handler).await;
                    (peer, outcome)
                });
            }
        }
    }
    drop(listener);
    stop.send_replace(true);
    let drain = async {
        while let Some(result) = tasks.join_next().await {
            let (peer, outcome) = result.map_err(Error::Task)?;
            on_close(peer, outcome);
        }
        Ok(())
    };
    match timeout_at(deadline(config.shutdown_timeout)?, drain).await {
        Ok(outcome) => outcome,
        Err(_) => {
            tasks.shutdown().await;
            Err(Error::Timeout(Phase::Shutdown))
        }
    }
}

async fn run<H, F>(
    mut connection: Connection<tokio::net::TcpStream>,
    peer: SocketAddr,
    config: ReceiverConfig,
    mut stop: watch::Receiver<bool>,
    handler: Arc<H>,
) -> Result<(), Error>
where
    H: Fn(SocketAddr, Arc<[u8]>) -> F + Send + Sync,
    F: Future<Output = Result<Option<Vec<u8>>, Error>> + Send,
{
    loop {
        let payload = tokio::select! {
            biased;
            _ = stop.changed() => return connection.shutdown().await,
            payload = connection.receive() => match payload? {
                Some(payload) => payload,
                None => return Ok(()),
            }
        };
        let reply = timeout_at(deadline(config.handler_timeout)?, handler(peer, payload))
            .await
            .map_err(|_| Error::Timeout(Phase::Handler))??;
        if let Some(reply) = reply {
            connection.send(&reply).await?;
        }
    }
}
