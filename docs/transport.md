# Async MLLP connections

Enable `transport` to use `mllp::transport::Connection<S>` with Tokio `AsyncRead + AsyncWrite +
Unpin` streams. `Connection::connect(SocketAddr, Config)` opens TCP with `TCP_NODELAY`; an accepted
TCP stream can be wrapped with `Connection::new`. The host runs a Tokio runtime with I/O and timers
enabled. The library does not construct a runtime or bind a listening address. The optional `serve`
helper owns bounded connection tasks for a listener supplied by the host.

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

`shutdown()` performs a bounded stream shutdown, then drops it. When using connections directly,
the host owns admission and service-wide draining. Dropping a receive future closes its stream.
TLS/mTLS, durable receipt and a global memory policy remain host responsibilities.

## Bounded TCP receiver

`serve(listener, config, shutdown, handler, on_close)` accepts an already-bound Tokio `TcpListener`.
`ReceiverConfig` defaults to 64 active connection tasks, a 30 s handler timeout and a 30 s shutdown
deadline, alongside the connection settings above. Zero connection limits and invalid durations
are rejected before accepting clients. When full, it stops accepting; extra clients may establish
TCP connections into the OS backlog but cannot start application work. It creates no message queue.

The shared handler receives `(peer: SocketAddr, payload: Arc<[u8]>)` and returns a `Send` future
yielding `Result<Option<Vec<u8>>, Error>`. `Some` is one unframed response; `None` sends nothing.
Every connection awaits its handler and response write before admitting the next message. Requests
on separate connections may run concurrently. Configure `max_connections` and frame limits together:
64 maximum-size frames can retain 1 GiB of payload bytes across decoder buffers and returned payloads,
before excess vector capacity, allocator overhead, handler output, indexes and external processing
state. This is not a process-wide byte-accounting mechanism. Host processing must bound its own
allocations and work.

Only the handler chooses whether/when to build an ACK. Return a success response after the actual
application or durable receipt boundary required by the configured interface. The receiver cannot
infer that boundary. If a handler fails or times out, its connection closes without a fabricated
ACK; `Error::Handler` can carry an application-specific error. `on_close(peer, outcome)` reports
normally joined connection outcomes, including framing, handler, deadline and write failures.
Those errors do not stop healthy connections. The peer address is transport context, not an
authenticated HL7 sender identity.

When `shutdown` resolves, `serve` drops its listener, signals connection tasks to stop reading and
waits for current handlers and response writes. Buffered or partially received messages that have
not entered the handler are discarded without ACK. On drain expiry it cancels and joins remaining
tasks and returns `Timeout(Shutdown)`; forced cancellations have no individual `on_close` callback.
An accept/configuration failure or worker panic returns an error and aborts other tasks. Dropping
the `serve` future also aborts its tasks and is not a graceful drain. Pending committed outcomes
and ACK-write uncertainty must be reconciled by the host's durable processing layer. Cancellation
does not prove that external side effects were rolled back.

All shutdown/handler deadlines are cooperative: handlers, future constructors and `on_close` must
yield promptly and avoid blocking work. Detached tasks or blocking operations started by a handler
are not owned or cancelled by `serve`. There is no database transaction, automatic retry, deduplication
or clinical-application guarantee in the receiver.

## Verification

Tests cover real loopback TCP with repeated original/enhanced exchanges, short duplex reads/writes,
coalesced frames, frames crossing the read-buffer boundary and incomplete EOF. Paused Tokio time
checks idle, non-resetting assembly, write and ACK deadlines; cancellation and duplicate ACKs
verify that a damaged/ambiguous exchange cannot reuse the stream. These are connection tests,
not throughput, recovery or gateway concurrency measurements.

Receiver tests additionally exercise connection admission under stalled handlers, malformed-client
isolation, persistent ACK exchanges, draining only admitted work, and cancellation of handlers at
processing/drain deadlines without emitting success responses.
