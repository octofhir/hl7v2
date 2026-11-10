use octofhir_hl7v2::{
    mllp::{self, Decoder, Error},
    wire::Message,
};
use std::io::{self, Write};

type Result = std::result::Result<(), Box<dyn std::error::Error>>;

fn collect(decoder: &mut Decoder, input: &[u8], output: &mut Vec<Vec<u8>>) -> Result {
    let mut remaining = input;
    while !remaining.is_empty() {
        let decoded = decoder.decode(remaining)?;
        assert!(decoded.consumed > 0 && decoded.consumed <= remaining.len());
        remaining = &remaining[decoded.consumed..];
        if let Some(payload) = decoded.payload {
            output.push(payload.to_vec());
        }
    }
    Ok(())
}

#[test]
fn every_split_one_byte_reads_and_coalesced_frames_preserve_payloads() -> Result {
    let payloads = [
        include_bytes!("fixtures/adt.er7").as_slice(),
        include_bytes!("fixtures/oru.er7").as_slice(),
        b"",
        b"\xff\r\xfe",
    ];
    let mut wire = Vec::new();
    for payload in payloads {
        wire.extend(mllp::encode(payload, 1024)?);
    }
    for split in 0..=wire.len() {
        let mut decoder = Decoder::new(1024);
        let mut received = Vec::new();
        collect(&mut decoder, &wire[..split], &mut received)?;
        collect(&mut decoder, &wire[split..], &mut received)?;
        decoder.finish()?;
        assert_eq!(received, payloads);
    }
    for size in [1, 2, 7, 1024] {
        let mut decoder = Decoder::new(1024);
        let mut received = Vec::new();
        for chunk in wire.chunks(size) {
            collect(&mut decoder, chunk, &mut received)?;
        }
        assert_eq!(received, payloads);
        decoder.finish()?;
        assert_eq!(Message::parse(&received[0])?.original(), payloads[0]);
    }
    Ok(())
}

#[test]
fn limits_malformed_framing_and_eof_fail_without_resynchronizing() -> Result {
    for (input, expected) in [
        (b"junk".as_slice(), Error::ExpectedStart),
        (b"\x0ba\x0bb", Error::NestedStart),
        (b"\x0ba\x1c!", Error::InvalidTrailer),
        (b"\x0babcd", Error::TooLarge),
    ] {
        let mut decoder = Decoder::new(3);
        assert!(matches!(decoder.decode(input), Err(error) if error == expected));
        assert_eq!(decoder.buffered_len(), 0);
        assert!(matches!(
            decoder.decode(b"\x0bok\x1c\r"),
            Err(Error::Failed)
        ));
        assert_eq!(decoder.finish(), Err(Error::Failed));
        decoder.reset();
        assert_eq!(
            decoder.decode(b"\x0bok\x1c\r")?.payload,
            Some(b"ok".as_slice())
        );
    }
    let frame = mllp::encode(b"abc", 3)?;
    for prefix in 0..=frame.len() {
        let mut decoder = Decoder::new(3);
        let decoded = decoder.decode(&frame[..prefix])?;
        assert_eq!(decoded.consumed, prefix);
        assert_eq!(decoded.payload.is_some(), prefix == frame.len());
        assert_eq!(
            decoder.finish(),
            if prefix == 0 || prefix == frame.len() {
                Ok(())
            } else {
                Err(Error::Incomplete)
            }
        );
    }
    let mut decoder = Decoder::new(0);
    assert_eq!(decoder.decode(b"\x0b\x1c\r")?.payload, Some(b"".as_slice()));
    assert!(matches!(decoder.decode(b"\x0bx"), Err(Error::TooLarge)));
    Ok(())
}

struct ShortWriter {
    bytes: Vec<u8>,
    remaining: usize,
}
impl Write for ShortWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "test disconnect"));
        }
        let count = bytes.len().min(2).min(self.remaining);
        self.bytes.extend_from_slice(&bytes[..count]);
        self.remaining -= count;
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn sender_validates_before_writing_and_propagates_partial_io_failure() -> Result {
    for (payload, maximum, expected) in [
        (b"a\x0bb".as_slice(), 3, Error::ReservedByte),
        (b"a\x1cb", 3, Error::ReservedByte),
        (b"abcd", 3, Error::TooLarge),
    ] {
        assert_eq!(mllp::encode(payload, maximum), Err(expected));
        let mut output = Vec::new();
        assert!(mllp::write_frame(&mut output, payload, maximum).is_err());
        assert!(output.is_empty());
    }
    let mut writer = ShortWriter {
        bytes: Vec::new(),
        remaining: 100,
    };
    mllp::write_frame(&mut writer, b"a\rb", 3)?;
    assert_eq!(writer.bytes, mllp::encode(b"a\rb", 3)?);
    let mut writer = ShortWriter {
        bytes: Vec::new(),
        remaining: 3,
    };
    assert!(
        matches!(mllp::write_frame(&mut writer, b"abc", 3), Err(error) if error.kind() == io::ErrorKind::BrokenPipe)
    );
    assert_eq!(writer.bytes, b"\x0bab");
    Ok(())
}
