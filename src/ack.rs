#![doc = include_str!("../docs/ack.md")]

use crate::wire::{self, Message, Presence};
use std::{borrow::Cow, fmt};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    AA,
    AE,
    AR,
    CA,
    CE,
    CR,
}
impl Code {
    pub fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::AA => b"AA",
            Self::AE => b"AE",
            Self::AR => b"AR",
            Self::CA => b"CA",
            Self::CE => b"CE",
            Self::CR => b"CR",
        }
    }
    pub fn is_success(self) -> bool {
        matches!(self, Self::AA | Self::CA)
    }
    pub fn is_commit(self) -> bool {
        matches!(self, Self::CA | Self::CE | Self::CR)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Condition {
    Always,
    Never,
    OnSuccess,
    OnError,
}
impl Condition {
    pub fn requests(self, code: Code) -> bool {
        match self {
            Self::Always => true,
            Self::Never => false,
            Self::OnSuccess => code.is_success(),
            Self::OnError => !code.is_success(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    Original,
    Enhanced {
        accept: Condition,
        application: Condition,
    },
}
impl Policy {
    pub fn from_message(message: &Message) -> Result<Self, Error> {
        let condition = |field| match message.value(&[1, field]).presence() {
            Presence::Missing | Presence::Empty => Ok(None),
            _ => match scalar(message, &[1, field])?.as_ref() {
                b"AL" => Ok(Some(Condition::Always)),
                b"NE" => Ok(Some(Condition::Never)),
                b"SU" => Ok(Some(Condition::OnSuccess)),
                b"ER" => Ok(Some(Condition::OnError)),
                _ => Err(Error::Policy),
            },
        };
        match (condition(15)?, condition(16)?) {
            (None, None) => Ok(Self::Original),
            (Some(accept), Some(application)) => Ok(Self::Enhanced {
                accept,
                application,
            }),
            _ => Err(Error::Policy), // This v2.5.1 contract requires both enhanced conditions.
        }
    }
    pub fn requests(self, code: Code) -> bool {
        match self {
            Self::Original => !code.is_commit(),
            Self::Enhanced {
                accept,
                application,
            } => {
                if code.is_commit() {
                    accept.requests(code)
                } else {
                    application.requests(code)
                }
            }
        }
    }
}

/// Supported synchronous request/response contracts. No implicit mode selection is performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Original,
    EnhancedAcceptOnly,
}

#[derive(Debug)]
pub enum Error {
    Wire(wire::Error),
    Header,
    Version,
    Charset,
    Policy,
    Correlation,
    Phase,
    Shape,
    Issue,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "HL7 ACK {self:?}")
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Wire(error) => Some(error),
            _ => None,
        }
    }
}
impl From<wire::Error> for Error {
    fn from(error: wire::Error) -> Self {
        Self::Wire(error)
    }
}

#[derive(Clone, Copy)]
pub enum Severity {
    Information,
    Warning,
    Error,
}
pub struct Issue<'a> {
    pub code: u16,
    pub severity: Severity,
    pub diagnostic: &'a [u8],
}
pub struct Reply<'a> {
    pub code: Code,
    pub control_id: &'a [u8],
    pub timestamp: &'a [u8],
    pub issue: Option<Issue<'a>>,
}
pub struct Acknowledgement {
    pub code: Code,
    pub message: Message,
}

fn scalar<'a>(message: &'a Message, path: &[usize]) -> Result<Cow<'a, [u8]>, Error> {
    let value = message.value(path);
    if value.presence() != Presence::Value {
        return Err(Error::Header);
    }
    Ok(value.decode()?)
}
fn raw(message: &Message, field: usize) -> &[u8] {
    message.value(&[1, field]).raw().unwrap_or(b"")
}
fn text(bytes: &[u8], utf8: bool) -> bool {
    if utf8 {
        std::str::from_utf8(bytes).is_ok()
    } else {
        bytes.is_ascii()
    }
}
fn header(message: &Message) -> Result<(Policy, bool), Error> {
    if message.delimiters().truncation.is_some()
        || [9, 11, 12]
            .iter()
            .any(|&field| message.value(&[1, field]).child_count() != 1)
        || message.value(&[1, 9, 1]).child_count() > 3
        || message.value(&[1, 9, 1, 3]).presence() == Presence::Null
    {
        return Err(Error::Header);
    }
    if scalar(message, &[1, 12, 1, 1])?.as_ref() != b"2.5.1" {
        return Err(Error::Version);
    }
    if !matches!(
        scalar(message, &[1, 11, 1, 1])?.as_ref(),
        b"P" | b"T" | b"D"
    ) {
        return Err(Error::Header);
    }
    let utf8 = match message.value(&[1, 18]).presence() {
        Presence::Missing | Presence::Empty => false,
        _ => match scalar(message, &[1, 18])?.as_ref() {
            b"ASCII" => false,
            b"UNICODE UTF-8" => true,
            _ => return Err(Error::Charset),
        },
    };
    let control = scalar(message, &[1, 10])?;
    if !text(&control, utf8) || control.iter().any(u8::is_ascii_control) {
        return Err(Error::Header);
    }
    for path in [&[1, 9, 1, 1][..], &[1, 9, 1, 2]] {
        let code = scalar(message, path)?;
        if code.len() != 3
            || !code
                .iter()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        {
            return Err(Error::Header);
        }
    }
    Ok((Policy::from_message(message)?, utf8))
}

pub fn validate_request(request: &Message, mode: Mode) -> Result<(), Error> {
    let (policy, _) = header(request)?;
    if scalar(request, &[1, 9, 1, 1])?.as_ref() == b"ACK" {
        return Err(Error::Shape);
    }
    match (mode, policy) {
        (Mode::Original, Policy::Original) => Ok(()),
        (
            Mode::EnhancedAcceptOnly,
            Policy::Enhanced {
                accept: Condition::Always,
                application: Condition::Never,
            },
        ) => Ok(()),
        _ => Err(Error::Policy),
    }
}

/// Check phase, request identity and envelope before exposing an acknowledgement to the caller.
pub fn correlate(
    request: &Message,
    response: Message,
    mode: Mode,
) -> Result<Acknowledgement, Error> {
    validate_request(request, mode)?;
    header(&response)?;
    if scalar(&response, &[1, 9, 1, 1])?.as_ref() != b"ACK"
        || response.segment_named(b"MSA", 2).is_some()
    {
        return Err(Error::Shape);
    }
    if scalar(&response, &[1, 9, 1, 3])?.as_ref() != b"ACK" {
        return Err(Error::Shape);
    }
    let msa = response
        .segment_named(b"MSA", 1)
        .ok_or(Error::Shape)?
        .occurrence();
    let code = match scalar(&response, &[msa, 1])?.as_ref() {
        b"AA" => Code::AA,
        b"AE" => Code::AE,
        b"AR" => Code::AR,
        b"CA" => Code::CA,
        b"CE" => Code::CE,
        b"CR" => Code::CR,
        _ => return Err(Error::Shape),
    };
    if code.is_commit() != (mode == Mode::EnhancedAcceptOnly) {
        return Err(Error::Phase);
    }
    for (left, right) in [
        (&[1, 10][..], &[msa, 2][..]),
        (&[1, 9, 1, 2], &[1, 9, 1, 2]),
        (&[1, 11, 1, 1], &[1, 11, 1, 1]),
    ] {
        if scalar(request, left)? != scalar(&response, right)? {
            return Err(Error::Correlation);
        }
    }
    Ok(Acknowledgement {
        code,
        message: response,
    })
}

/// Build an unframed ACK. Call only after reaching the durability/application boundary it asserts.
pub fn build(request: &Message, reply: Reply<'_>, max_bytes: usize) -> Result<Vec<u8>, Error> {
    let (policy, utf8) = header(request)?;
    if scalar(request, &[1, 9, 1, 1])?.as_ref() == b"ACK" {
        return Err(Error::Shape);
    }
    if !policy.requests(reply.code) {
        return Err(Error::Policy);
    }
    if reply.control_id.is_empty()
        || !text(reply.control_id, utf8)
        || reply.control_id.iter().any(u8::is_ascii_control)
        || !valid_timestamp(reply.timestamp)
    {
        return Err(Error::Header);
    }
    for field in [3, 4, 5, 6, 11, 12, 17, 18] {
        if !text(raw(request, field), utf8) || raw(request, field).iter().any(u8::is_ascii_control)
        {
            return Err(Error::Charset);
        }
    }
    let delimiters = request.delimiters();
    let timestamp = wire::encode_scalar(reply.timestamp, delimiters, max_bytes)?;
    let control = wire::encode_scalar(reply.control_id, delimiters, max_bytes)?;
    let mut message_type = b"ACK".to_vec();
    message_type.push(delimiters.encoding[0]);
    message_type.extend(wire::encode_scalar(
        &scalar(request, &[1, 9, 1, 2])?,
        delimiters,
        max_bytes,
    )?);
    message_type.push(delimiters.encoding[0]);
    message_type.extend_from_slice(b"ACK");
    let no_ack: &[u8] = if policy == Policy::Original {
        b""
    } else {
        b"NE"
    };
    let mut output = Vec::new();
    wire::append(&mut output, b"MSH", max_bytes)?;
    wire::append(&mut output, &[delimiters.field], max_bytes)?;
    wire::append(&mut output, raw(request, 2), max_bytes)?;
    for field in [
        raw(request, 5),
        raw(request, 6),
        raw(request, 3),
        raw(request, 4),
        &timestamp,
        b"",
        &message_type,
        &control,
        raw(request, 11),
        raw(request, 12),
        b"",
        b"",
        no_ack,
        no_ack,
        raw(request, 17),
        raw(request, 18),
    ] {
        wire::append(&mut output, &[delimiters.field], max_bytes)?;
        wire::append(&mut output, field, max_bytes)?;
    }
    wire::append(&mut output, b"\rMSA", max_bytes)?;
    for field in [reply.code.as_bytes(), raw(request, 10)] {
        wire::append(&mut output, &[delimiters.field], max_bytes)?;
        wire::append(&mut output, field, max_bytes)?;
    }
    if let Some(issue) = reply.issue {
        if !matches!(issue.code, 0 | 100..=103 | 200..=207) || !text(issue.diagnostic, utf8) {
            return Err(Error::Issue);
        }
        let coding = format!(
            "{}{}{}HL70357",
            issue.code,
            char::from(delimiters.encoding[0]),
            char::from(delimiters.encoding[0])
        );
        let severity: &[u8] = match issue.severity {
            Severity::Information => b"I",
            Severity::Warning => b"W",
            Severity::Error => b"E",
        };
        let diagnostic = wire::encode_scalar(issue.diagnostic, delimiters, max_bytes)?;
        wire::append(&mut output, b"\rERR", max_bytes)?;
        for field in [
            b"".as_slice(),
            b"",
            coding.as_bytes(),
            severity,
            b"",
            b"",
            &diagnostic,
        ] {
            wire::append(&mut output, &[delimiters.field], max_bytes)?;
            wire::append(&mut output, field, max_bytes)?;
        }
    }
    wire::append(&mut output, b"\r", max_bytes)?;
    Ok(output)
}

// Generated ACKs use complete v2.5.1 timestamps; incoming partial dates remain wire data.
fn valid_timestamp(bytes: &[u8]) -> bool {
    if bytes.len() < 14 || !bytes[..14].iter().all(u8::is_ascii_digit) {
        return false;
    }
    let number = |range: std::ops::Range<usize>| {
        bytes[range]
            .iter()
            .fold(0u16, |n, b| n * 10 + u16::from(b - b'0'))
    };
    let year = number(0..4);
    let month = number(4..6);
    let day = number(6..8);
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = match month {
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => return false,
    };
    if year == 0
        || day == 0
        || day > days
        || number(8..10) > 23
        || number(10..12) > 59
        || number(12..14) > 59
    {
        return false;
    }
    let mut tail = &bytes[14..];
    if tail.first() == Some(&b'.') {
        let digits = tail[1..].iter().take_while(|b| b.is_ascii_digit()).count();
        if !(1..=4).contains(&digits) {
            return false;
        }
        tail = &tail[digits + 1..];
    }
    tail.is_empty()
        || (tail.len() == 5
            && matches!(tail[0], b'+' | b'-')
            && tail[1..].iter().all(u8::is_ascii_digit)
            && (tail[1] - b'0') * 10 + tail[2] - b'0' <= 23
            && (tail[3] - b'0') * 10 + tail[4] - b'0' <= 59)
}
