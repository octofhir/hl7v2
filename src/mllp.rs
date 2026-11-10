#![doc = include_str!("../docs/mllp.md")]

use std::{
    fmt,
    io::{self, Write},
};

pub const START_BLOCK: u8 = 0x0b;
pub const END_BLOCK: u8 = 0x1c;
pub const CARRIAGE_RETURN: u8 = 0x0d;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    ExpectedStart,
    NestedStart,
    InvalidTrailer,
    TooLarge,
    ReservedByte,
    Incomplete,
    Failed,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MLLP {self:?}")
    }
}
impl std::error::Error for Error {}

#[derive(Clone, Copy)]
enum State {
    Start,
    Payload,
    Trailer,
    Complete,
    Failed,
}

/// The unconsumed suffix belongs to the caller. A payload borrows the decoder until processed.
pub struct Decoded<'a> {
    pub consumed: usize,
    pub payload: Option<&'a [u8]>,
}

/// One reusable payload buffer and no output queue. The maximum excludes the three framing bytes.
pub struct Decoder {
    state: State,
    payload: Vec<u8>,
    max_payload: usize,
}

impl Decoder {
    pub fn new(max_payload: usize) -> Self {
        Self {
            state: State::Start,
            payload: Vec::new(),
            max_payload,
        }
    }
    pub fn buffered_len(&self) -> usize {
        self.payload.len()
    }
    /// True between frames, including after returning a complete frame.
    pub fn is_idle(&self) -> bool {
        matches!(self.state, State::Start | State::Complete)
    }
    /// Explicitly abandon a connection's state before reusing the decoder for another connection.
    pub fn reset(&mut self) {
        self.payload.clear();
        self.state = State::Start;
    }

    /// Consume at most one frame. Incomplete input consumes the entire slice and returns no payload.
    /// An error poisons the decoder; close the connection rather than guessing a new frame boundary.
    pub fn decode<'a>(&'a mut self, input: &[u8]) -> Result<Decoded<'a>, Error> {
        if matches!(self.state, State::Failed) {
            return Err(Error::Failed);
        }
        if matches!(self.state, State::Complete) {
            self.reset();
        }
        let mut consumed = 0;
        while consumed < input.len() {
            match self.state {
                State::Start => {
                    if input[consumed] != START_BLOCK {
                        return Err(self.fail(Error::ExpectedStart));
                    }
                    self.state = State::Payload;
                    consumed += 1;
                }
                State::Payload => {
                    let remaining = &input[consumed..];
                    let budget = self.max_payload - self.payload.len();
                    let window = remaining.len().min(budget.saturating_add(1));
                    let length = remaining[..window]
                        .iter()
                        .position(|&b| b == START_BLOCK || b == END_BLOCK)
                        .unwrap_or(window);
                    if length > budget {
                        return Err(self.fail(Error::TooLarge));
                    }
                    self.payload.extend_from_slice(&remaining[..length]);
                    consumed += length;
                    if consumed == input.len() {
                        break;
                    }
                    if input[consumed] == START_BLOCK {
                        return Err(self.fail(Error::NestedStart));
                    }
                    self.state = State::Trailer;
                    consumed += 1;
                }
                State::Trailer => {
                    if input[consumed] != CARRIAGE_RETURN {
                        return Err(self.fail(Error::InvalidTrailer));
                    }
                    self.state = State::Complete;
                    return Ok(Decoded {
                        consumed: consumed + 1,
                        payload: Some(&self.payload),
                    });
                }
                State::Complete | State::Failed => return Err(self.fail(Error::Failed)),
            }
        }
        Ok(Decoded {
            consumed,
            payload: None,
        })
    }
    /// Call when the stream reaches EOF. Partial headers, payloads and trailers are errors.
    pub fn finish(&mut self) -> Result<(), Error> {
        if self.is_idle() {
            Ok(())
        } else if matches!(self.state, State::Failed) {
            Err(Error::Failed)
        } else {
            Err(self.fail(Error::Incomplete))
        }
    }
    fn fail(&mut self, error: Error) -> Error {
        self.payload.clear();
        self.state = State::Failed;
        error
    }
}

fn validate(payload: &[u8], max_payload: usize) -> Result<(), Error> {
    if payload.len() > max_payload || payload.len().checked_add(3).is_none() {
        return Err(Error::TooLarge);
    }
    if payload.iter().any(|&b| b == START_BLOCK || b == END_BLOCK) {
        return Err(Error::ReservedByte);
    }
    Ok(())
}

pub fn encode(payload: &[u8], max_payload: usize) -> Result<Vec<u8>, Error> {
    validate(payload, max_payload)?;
    let mut output = Vec::with_capacity(payload.len() + 3);
    output.push(START_BLOCK);
    output.extend_from_slice(payload);
    output.extend_from_slice(&[END_BLOCK, CARRIAGE_RETURN]);
    Ok(output)
}

/// Validate before writing any bytes. I/O errors may leave a partial frame on the connection.
/// This does not flush a buffered writer or wait for an acknowledgement.
pub fn write_frame(writer: &mut impl Write, payload: &[u8], max_payload: usize) -> io::Result<()> {
    validate(payload, max_payload)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    writer.write_all(&[START_BLOCK])?;
    writer.write_all(payload)?;
    writer.write_all(&[END_BLOCK, CARRIAGE_RETURN])
}
