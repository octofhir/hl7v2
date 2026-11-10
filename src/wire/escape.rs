use super::{Delimiters, DiagnosticKind, Error, ErrorKind, Span};
use std::borrow::Cow;

pub(super) struct Token<'a> {
    pub span: Span,
    pub payload: Option<&'a [u8]>,
}

pub(super) fn closing(bytes: &[u8], start: usize, end: usize, escape: u8) -> Option<usize> {
    bytes[start + 1..end]
        .iter()
        .position(|&byte| byte == escape)
        .map(|n| start + n + 2)
}

pub(super) fn tokens(bytes: &[u8], offset: usize, escape: u8) -> impl Iterator<Item = Token<'_>> {
    let mut next = 0;
    std::iter::from_fn(move || {
        let start = next + bytes[next..].iter().position(|&byte| byte == escape)?;
        let end = closing(bytes, start, bytes.len(), escape);
        next = end.unwrap_or(bytes.len());
        Some(Token {
            span: offset + start..offset + next,
            payload: end.map(|end| &bytes[start + 1..end - 1]),
        })
    })
}

fn escape_byte(payload: &[u8], delimiters: Delimiters) -> Option<u8> {
    let [code] = payload else {
        return None;
    };
    match code {
        b'F' => Some(delimiters.field),
        b'S' => Some(delimiters.encoding[0]),
        b'R' => Some(delimiters.encoding[1]),
        b'E' => Some(delimiters.encoding[2]),
        b'T' => Some(delimiters.encoding[3]),
        b'P' => delimiters.truncation,
        _ => None,
    }
}

fn valid_hex(payload: &[u8]) -> bool {
    payload.len() > 1
        && (payload.len() - 1).is_multiple_of(2)
        && payload[1..].iter().all(u8::is_ascii_hexdigit)
}

pub(super) fn diagnostic(payload: Option<&[u8]>, delimiters: Delimiters) -> Option<DiagnosticKind> {
    match payload {
        None => Some(DiagnosticKind::UnterminatedEscape),
        Some(payload) if escape_byte(payload, delimiters).is_some() => None,
        Some(payload) if payload.starts_with(b"X") => {
            (!valid_hex(payload)).then_some(DiagnosticKind::InvalidHex)
        }
        Some(_) => Some(DiagnosticKind::UnsupportedEscape),
    }
}

pub(super) fn decode(
    bytes: &[u8],
    offset: usize,
    delimiters: Delimiters,
) -> Result<Cow<'_, [u8]>, Error> {
    if !bytes.contains(&delimiters.encoding[2]) {
        return Ok(Cow::Borrowed(bytes));
    }
    let mut output = Vec::with_capacity(bytes.len());
    let mut cursor = 0;
    for token in tokens(bytes, offset, delimiters.encoding[2]) {
        let payload = token
            .payload
            .ok_or_else(|| Error::new(ErrorKind::Escape, token.span.clone()))?;
        output.extend_from_slice(&bytes[cursor..token.span.start - offset]);
        if let Some(byte) = escape_byte(payload, delimiters) {
            output.push(byte);
        } else if payload.starts_with(b"X") {
            if !valid_hex(payload) {
                return Err(Error::new(ErrorKind::InvalidHex, token.span));
            }
            for pair in payload[1..].as_chunks::<2>().0 {
                output.push(hex(pair[0]) * 16 + hex(pair[1]));
            }
        } else {
            return Err(Error::new(ErrorKind::Escape, token.span));
        }
        cursor = token.span.end - offset;
    }
    output.extend_from_slice(&bytes[cursor..]);
    Ok(Cow::Owned(output))
}

fn hex(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        _ => byte - b'A' + 10,
    }
}

pub(super) fn append(output: &mut Vec<u8>, bytes: &[u8], max: usize) -> Result<(), Error> {
    if bytes.len() > max.saturating_sub(output.len()) {
        return Err(Error::new(ErrorKind::Limit, 0..0));
    }
    output.extend_from_slice(bytes);
    Ok(())
}

pub(super) fn encode(bytes: &[u8], delimiters: Delimiters, max: usize) -> Result<Vec<u8>, Error> {
    let mut output = Vec::new();
    let escape = delimiters.encoding[2];
    if bytes == b"\"\"" {
        append(
            &mut output,
            &[escape, b'X', b'2', b'2', b'2', b'2', escape],
            max,
        )?;
        return Ok(output);
    }
    for &byte in bytes {
        let values = [
            delimiters.field,
            delimiters.encoding[0],
            delimiters.encoding[1],
            escape,
            delimiters.encoding[3],
        ];
        if let Some(index) = values.iter().position(|&value| value == byte) {
            append(&mut output, &[escape, b"FSRET"[index], escape], max)?;
        } else if Some(byte) == delimiters.truncation {
            append(&mut output, &[escape, b'P', escape], max)?;
        } else if byte.is_ascii_control() {
            const HEX: &[u8] = b"0123456789ABCDEF";
            append(
                &mut output,
                &[
                    escape,
                    b'X',
                    HEX[usize::from(byte >> 4)],
                    HEX[usize::from(byte & 15)],
                    escape,
                ],
                max,
            )?;
        } else {
            append(&mut output, &[byte], max)?;
        }
    }
    Ok(output)
}
