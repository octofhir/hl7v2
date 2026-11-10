#![doc = include_str!("../../docs/transport.md")]

use crate::{
    ack::{self, Acknowledgement, Mode},
    wire::{Message, ParseOptions},
};
use std::{fmt, io, net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
    time::{Instant, timeout_at},
};

mod receiver;
pub use receiver::{ReceiverConfig, serve};

#[derive(Debug, Clone, Copy)]
pub struct Config {
    pub max_payload: usize,
    pub connect_timeout: Duration,
    pub idle_timeout: Duration,
    pub frame_timeout: Duration,
    pub write_timeout: Duration,
    pub acknowledgement_timeout: Duration,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            max_payload: 8 * 1024 * 1024,
            connect_timeout: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(30),
            frame_timeout: Duration::from_secs(10),
            write_timeout: Duration::from_secs(10),
            acknowledgement_timeout: Duration::from_secs(30),
        }
    }
}
impl Config {
    fn validate(self) -> Result<(), Error> {
        for duration in [
            self.connect_timeout,
            self.idle_timeout,
            self.frame_timeout,
            self.write_timeout,
            self.acknowledgement_timeout,
        ] {
            deadline(duration)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Connect,
    Idle,
    Frame,
    Write,
    Acknowledgement,
    Handler,
    Shutdown,
}
#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Frame(super::Error),
    Ack(ack::Error),
    Wire(crate::wire::Error),
    Timeout(Phase),
    Closed,
    Configuration,
    Handler(Box<dyn std::error::Error + Send + Sync>),
    Task(tokio::task::JoinError),
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MLLP transport {self:?}")
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Frame(e) => Some(e),
            Self::Ack(e) => Some(e),
            Self::Wire(e) => Some(e),
            Self::Handler(e) => Some(e.as_ref()),
            Self::Task(e) => Some(e),
            _ => None,
        }
    }
}
impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<super::Error> for Error {
    fn from(error: super::Error) -> Self {
        Self::Frame(error)
    }
}
impl From<ack::Error> for Error {
    fn from(error: ack::Error) -> Self {
        Self::Ack(error)
    }
}
impl From<crate::wire::Error> for Error {
    fn from(error: crate::wire::Error) -> Self {
        Self::Wire(error)
    }
}

fn deadline(duration: Duration) -> Result<Instant, Error> {
    if duration.is_zero() {
        return Err(Error::Configuration);
    }
    Instant::now()
        .checked_add(duration)
        .ok_or(Error::Configuration)
}

pub struct Connection<S> {
    stream: Option<S>,
    decoder: super::Decoder,
    config: Config,
    buffer: Box<[u8; 8192]>,
    start: usize,
    end: usize,
}
impl<S: AsyncRead + AsyncWrite + Unpin> Connection<S> {
    pub fn new(stream: S, config: Config) -> Result<Self, Error> {
        config.validate()?;
        Ok(Self {
            stream: Some(stream),
            decoder: super::Decoder::new(config.max_payload),
            config,
            buffer: Box::new([0; 8192]),
            start: 0,
            end: 0,
        })
    }
    pub fn is_open(&self) -> bool {
        self.stream.is_some()
    }

    /// A dropped/cancelled operation drops the locally owned stream. It cannot be reused later.
    pub async fn receive(&mut self) -> Result<Option<Arc<[u8]>>, Error> {
        let mut stream = self.stream.take().ok_or(Error::Closed)?;
        let outcome = self.read(&mut stream).await;
        if matches!(&outcome, Ok(Some(_))) {
            self.stream = Some(stream);
        }
        outcome
    }
    pub async fn send(&mut self, payload: &[u8]) -> Result<(), Error> {
        super::validate(payload, self.config.max_payload)?;
        let mut stream = self.stream.take().ok_or(Error::Closed)?;
        let outcome = self.write(&mut stream, payload).await;
        if outcome.is_ok() {
            self.stream = Some(stream);
        }
        outcome
    }
    /// Validate the requested choreography before sending, then correlate exactly one response.
    pub async fn request(
        &mut self,
        request: &Message,
        mode: Mode,
    ) -> Result<Acknowledgement, Error> {
        ack::validate_request(request, mode)?;
        super::validate(request.original(), self.config.max_payload)?;
        let mut stream = self.stream.take().ok_or(Error::Closed)?;
        let outcome = async {
            self.write(&mut stream, request.original()).await?;
            let input = timeout_at(
                deadline(self.config.acknowledgement_timeout)?,
                self.read(&mut stream),
            )
            .await
            .map_err(|_| Error::Timeout(Phase::Acknowledgement))??
            .ok_or(Error::Closed)?;
            let response = Message::from_shared(
                input,
                ParseOptions {
                    max_bytes: self.config.max_payload,
                    ..Default::default()
                },
            )?;
            Ok(ack::correlate(request, response, mode)?)
        }
        .await;
        if outcome.is_ok() {
            self.stream = Some(stream);
        }
        outcome
    }
    pub async fn shutdown(&mut self) -> Result<(), Error> {
        if let Some(mut stream) = self.stream.take() {
            timeout_at(deadline(self.config.write_timeout)?, stream.shutdown())
                .await
                .map_err(|_| Error::Timeout(Phase::Write))??;
        }
        Ok(())
    }

    async fn read(&mut self, stream: &mut S) -> Result<Option<Arc<[u8]>>, Error> {
        let idle = deadline(self.config.idle_timeout)?;
        let mut frame = None;
        loop {
            if frame.is_some_and(|until| Instant::now() >= until) {
                return Err(Error::Timeout(Phase::Frame));
            }
            if self.start < self.end {
                let decoded = self.decoder.decode(&self.buffer[self.start..self.end])?;
                self.start += decoded.consumed;
                if let Some(payload) = decoded.payload {
                    return Ok(Some(Arc::from(payload)));
                }
                if !self.decoder.is_idle() && frame.is_none() {
                    frame = Some(deadline(self.config.frame_timeout)?);
                }
            }
            let until = frame.unwrap_or(idle);
            self.end = timeout_at(until, stream.read(&mut self.buffer[..]))
                .await
                .map_err(|_| {
                    Error::Timeout(if frame.is_some() {
                        Phase::Frame
                    } else {
                        Phase::Idle
                    })
                })??;
            self.start = 0;
            if self.end == 0 {
                self.decoder.finish()?;
                return Ok(None);
            }
        }
    }
    async fn write(&self, stream: &mut S, payload: &[u8]) -> Result<(), Error> {
        timeout_at(deadline(self.config.write_timeout)?, async {
            stream.write_all(&[super::START_BLOCK]).await?;
            stream.write_all(payload).await?;
            stream
                .write_all(&[super::END_BLOCK, super::CARRIAGE_RETURN])
                .await?;
            stream.flush().await
        })
        .await
        .map_err(|_| Error::Timeout(Phase::Write))??;
        Ok(())
    }
}

impl Connection<TcpStream> {
    pub async fn connect(address: SocketAddr, config: Config) -> Result<Self, Error> {
        config.validate()?;
        let stream = timeout_at(
            deadline(config.connect_timeout)?,
            TcpStream::connect(address),
        )
        .await
        .map_err(|_| Error::Timeout(Phase::Connect))??;
        stream.set_nodelay(true)?;
        Self::new(stream, config)
    }
}
