# MLLP framing core

`mllp` implements incremental byte framing with no dependencies. The frame layout is VT (`0x0B`), payload, FS (`0x1C`), CR (`0x0D`), following the [HL7 MLLP block format](https://hl7.eu/refactored/transport01mllp.html). Payload content, character sets, HL7 parsing and acknowledgements are separate concerns. This module does not open sockets or issue ACKs.

## Incremental decoding

Construct `Decoder::new(max_payload)` with an explicit payload-byte limit. `decode(input)` returns the number of consumed bytes and at most one borrowed payload. Partial input consumes the entire slice and returns no payload. A complete frame stops at its CR; pass the unconsumed suffix back after processing the returned payload. The decoder retains one reusable buffer, with no internal message queue. Borrowing the payload prevents another mutable decoding call until that payload has been processed.

```rust
use octofhir_hl7v2::{mllp::Decoder, wire::Message};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let stream = b"\x0bMSH|^~\\&|LAB\r\x1c\r\x0bMSH|^~\\&|EHR\r\x1c\r";
    let mut decoder = Decoder::new(1024);
    let mut received = 0;
    for chunk in stream.chunks(7) {
        let mut remaining = chunk;
        while !remaining.is_empty() {
            let decoded = decoder.decode(remaining)?;
            remaining = &remaining[decoded.consumed..];
            if let Some(payload) = decoded.payload {
                let message = Message::parse(payload)?;
                assert_eq!(message.segments().len(), 1);
                received += 1;
            }
        }
    }
    decoder.finish()?; // EOF, not the end of an individual read.
    assert_eq!(received, 2);
    Ok(())
}
```

Fragmented start/trailer markers and multiple frames in a read are supported. Zero-length frames are valid framing; the HL7 parser independently rejects an empty message. CRs inside the payload remain data. The codec preserves non-UTF-8 bytes and performs no charset conversion or clinical validation.

`finish()` checks EOF: only a boundary between frames is clean. A partial payload or trailer fails with `Incomplete`. Junk outside frames, nested VT, an invalid trailer and payload overflow poison the decoder. Subsequent decoding fails with `Failed`; the host should close that connection. `reset()` explicitly abandons the state when reusing the decoder for a new connection, retaining reusable buffer capacity. There is no heuristic resynchronization or lenient trailer mode.

The limit excludes the three framing bytes. Bulk scanning stops at the first marker or the first byte beyond the remaining budget. Bytes that would exceed the limit are not appended. `buffered_len()` never exceeds the limit, but vector capacity and allocator overhead are additional memory. After a complete frame, the buffer remains borrowed/available until the next decoding call. `is_idle()` distinguishes a between-frame boundary from an incomplete frame. Hosts must separately enforce connection/global memory budgets and absolute frame-assembly, idle and write deadlines; no clock or timeout policy is hidden in the codec.

## Encoding

`encode(payload, max_payload)` returns one framed buffer. `write_frame(writer, payload, max_payload)` writes framing and payload to a standard `Write` sink without constructing another full frame buffer. Both validate size and reject raw VT/FS payload bytes before writing. The ER7 layer can represent reserved data bytes through hex escapes; the framer never rewrites a payload.

`write_frame` handles short writes through `write_all` and propagates I/O failure. An I/O failure may leave a partial frame, so the host must discard that connection. It does not flush a buffered writer, correlate responses, or treat successful writing as durable acceptance.

## Verification and next boundary

Three integration scenarios cover every two-part split of four concatenated frames, fixed chunk sizes including one-byte reads, opaque bytes, an empty frame, all EOF positions, exact/overflow limits, malformed framing, poison/reset behavior, short writes and interrupted output. The documentation example is compiled as a doctest.

Async sockets, connection limits/timeouts/shutdown and correlated ACK choreography are the next transport layer. Durable receipt and application completion remain later processing boundaries; the framing codec alone makes no delivery guarantee.
