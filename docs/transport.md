# Async MLLP connections

Enable `transport` to use `mllp::transport::Connection<S>` with Tokio `AsyncRead + AsyncWrite +
Unpin` streams. `Connection::connect(SocketAddr, Config)` opens TCP with `TCP_NODELAY`; an accepted
TCP stream can be wrapped with `Connection::new`. The host runs a Tokio runtime with I/O and timers
enabled. The library does not construct a runtime, listener task manager or worker pool.

```no_run
use octofhir_hl7v2::{
    ack::Mode,
    mllp::transport::{Config, Connection},
    wire::Message,
};
use std::net::SocketAddr;

async fn exchange(address: SocketAddr, message: &Message)
    -> Result<(), Box<dyn std::error::Error>>
{
    let mut connection = Connection::connect(address, Config::default()).await?;
    let acknowledgement = connection.request(message, Mode::EnhancedAcceptOnly).await?;
    if !acknowledgement.code.is_success() {
        return Err("remote receiver rejected acceptance".into());
    }
    connection.shutdown().await?;
    Ok(())
}
```

This example requires a v2.5.1 message with MSH-15=`AL`, MSH-16=`NE`. Original mode explicitly uses
`Mode::Original`. A successful commit ACK asserts the receiver's configured acceptance boundary;
it does not establish that FHIR data was written. See the [ACK contract](ack.md).

## Receiving and backpressure

`receive()` returns `Option<Arc<[u8]>>`: one unframed payload or clean EOF. Feed those bytes to
`Message::from_shared` to avoid a second source copy. `send()` validates and writes one frame,
then flushes; it performs no HL7 processing and waits for no response. A receiver explicitly calls
`receive`, performs its admission/processing policy, builds an ACK when appropriate and calls
`send`. Never infer permission to return `CA` or `AA` merely from receiving a complete frame.

Fragmented and coalesced reads work over persistent connections. There is no background reader or
output queue: the host can stop calling `receive` while admission is blocked. A connection retains
one reusable decoder buffer and an 8 KiB read buffer. Returning a payload copies it into an Arc;
decoder capacity and returned payload therefore coexist. Allocator overhead, parser indexes and
payloads retained by the host are additional memory. The frame limit is not a global memory budget.

## Limits, deadlines and cancellation

Default maximum payload: 8 MiB. Default deadlines: connect 10 s, idle 30 s, absolute frame assembly
10 s, whole-frame write/flush 10 s and ACK wait 30 s after writing. Assembly begins when a partial
frame's start is observed and is not extended by drip-fed bytes. Zero/overflowing durations are
configuration errors. The ACK wait is also subject to idle/frame deadlines. Tokio timers require
the executor to keep polling; they do not preempt CPU-bound parsing or host work.

Framing/I/O errors, EOF, timeouts, invalid received ACKs and cancellation of a started I/O operation
drop the owned stream and leave the connection closed. This intentionally prevents reuse after a
partial write or a late ACK. Request-policy and outgoing-frame validation failures occur before
I/O and leave an open connection usable. An unpolled future has not started an operation.

`request()` owns the connection through one write and one correlated response; no pipelining,
automatic reconnect or retry is performed. A duplicate response for another control ID fails
correlation and closes the connection. After a sent-message timeout the remote outcome is unknown;
the host must reconcile or retry under its delivery identity policy. Negative ACK codes remain
observable outcomes rather than I/O errors.

`shutdown()` performs a bounded stream shutdown, then drops it. The host owns connection and global
buffer limits, bounded admission, task cancellation, service-wide draining, TLS/mTLS and durable
receipt. Dropping a receive future is a way to close its stream during host shutdown. There is no
durability or clinical-application guarantee in this transport layer.

## Verification

Tests cover real loopback TCP with repeated original/enhanced exchanges, short duplex reads/writes,
coalesced frames, frames crossing the read-buffer boundary and incomplete EOF. Paused Tokio time
checks idle, non-resetting assembly, write and ACK deadlines; cancellation and duplicate ACKs
verify that a damaged/ambiguous exchange cannot reuse the stream. These are connection tests,
not throughput, recovery or gateway concurrency measurements.
