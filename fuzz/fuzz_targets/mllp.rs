#![no_main]
use libfuzzer_sys::fuzz_target;
use octofhir_hl7v2::mllp::{self, Decoder, Error};

fn decode(bytes: &[u8], limit: usize, chunk: usize) -> (Vec<Vec<u8>>, Result<(), Error>) {
    let mut decoder = Decoder::new(limit);
    let mut frames = Vec::new();
    let mut result = Ok(());
    'input: for part in bytes.chunks(chunk) {
        let mut remaining = part;
        while !remaining.is_empty() {
            match decoder.decode(remaining) {
                Ok(decoded) => {
                    assert!(decoded.consumed > 0 && decoded.consumed <= remaining.len());
                    remaining = &remaining[decoded.consumed..];
                    if let Some(payload) = decoded.payload {
                        assert!(payload.len() <= limit);
                        frames.push(payload.to_vec());
                    }
                }
                Err(error) => {
                    result = Err(error);
                    break 'input;
                }
            }
            assert!(decoder.buffered_len() <= limit);
        }
    }
    if result.is_ok() {
        result = decoder.finish();
    }
    if result.is_err() {
        assert!(matches!(decoder.decode(b""), Err(Error::Failed)));
    }
    decoder.reset();
    assert!(decoder.is_idle());
    assert_eq!(decoder.buffered_len(), 0);
    assert!(matches!(decoder.decode(b"\x0b\x1c\r"), Ok(frame) if frame.payload == Some(b"")));
    (frames, result)
}

fuzz_target!(|bytes: &[u8]| {
    if bytes.len() > 16_384 {
        return;
    }
    let limit = [0, 1, 64, 16_384][usize::from(bytes.first().copied().unwrap_or(0)) % 4];
    let chunk = usize::from(bytes.get(1).copied().unwrap_or(0)) % 64 + 1;
    assert_eq!(
        decode(bytes, limit, chunk),
        decode(bytes, limit, bytes.len().max(1))
    );
    let mut written = Vec::new();
    match mllp::encode(bytes, limit) {
        Ok(frame) => {
            assert!(mllp::write_frame(&mut written, bytes, limit).is_ok());
            assert_eq!(written, frame);
            let expected = vec![bytes.to_vec(), Vec::new(), bytes.to_vec()];
            let coalesced = [frame.as_slice(), b"\x0b\x1c\r", frame.as_slice()].concat();
            assert_eq!(decode(&coalesced, limit, chunk), (expected, Ok(())));
        }
        Err(_) => {
            assert!(mllp::write_frame(&mut written, bytes, limit).is_err());
            assert!(written.is_empty());
        }
    }
});
