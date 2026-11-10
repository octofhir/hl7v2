# HL7 acknowledgements

The dependency-free `ack` module builds and correlates general ACKs for a narrow v2.5.1 envelope
contract. It does not validate clinical messages against schemas, select a durability boundary or
send a response automatically. The host supplies the ACK's fresh control ID, clock value and code.

```rust
use octofhir_hl7v2::{ack::{self, Code, Mode, Reply}, wire::Message};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let request = Message::parse(
        b"MSH|^~\\&|SOURCE|S|TARGET|T|20261110120000||ADT^A01^ADT_A01|m1|T|2.5.1\rPID|1\r"
    )?;
    // The host has completed the application boundary required by original mode.
    let bytes = ack::build(&request, Reply {
        code: Code::AA,
        control_id: b"ack-1",
        timestamp: b"20261110120000+0300",
        issue: None,
    }, 4096)?;
    let response = ack::correlate(&request, Message::parse(bytes)?, Mode::Original)?;
    assert_eq!(response.code, Code::AA);
    Ok(())
}
```

## Policy and phases

Both MSH-15 and MSH-16 missing/empty select original mode. This initial enhanced contract requires
both fields valued. Explicit null, one blank field and unknown conditions are errors. `AL`, `NE`,
`SU` and `ER` are evaluated independently for acceptance (`CA/CE/CR`) and application (`AA/AE/AR`).
`build` refuses a code suppressed by that policy. These rules follow the targeted
[v2.5.1 control chapter](https://hl7.eu/HL7v2x/v251/std251/ch02.html); they are not a cross-version policy.

`Mode::Original` expects one application ACK. `Mode::EnhancedAcceptOnly` requires MSH-15=`AL`,
MSH-16=`NE` and expects one commit ACK. Conditional suppression, enhanced application ACK delivery
and two-phase synchronous exchanges are not implemented by the connection helper. Parsing a policy
does not enable those routes. A negative ACK is returned as an `Acknowledgement` with its code,
not as a transport error; the host decides whether to reject, quarantine or retry.

## Envelope and encoding

Envelope checks require version 2.5.1, processing ID `P/T/D`, three-character message/event codes,
non-null scalar control IDs and four MSH-2 encoding characters. Repeated MSH-9/11/12 are rejected.
ASCII (including an absent declaration) and a single `UNICODE UTF-8` charset declaration are
supported. Other charsets and alternate charset switching require a separate future contract.

Generation swaps MSH sender/receiver fields, uses `ACK^event^ACK`, retains processing/version,
country and charset fields, and copies request MSH-10 into MSA-2 with its original escaping.
MSH-10 of the response is separate. Generated timestamps must be complete calendar-valid
`YYYYMMDDHHMMSS`, optionally with 1–4 fractional digits and/or a signed HHMM offset. Incoming
clinical partial dates remain untouched by the wire parser. Enhanced ACKs request no further ACK.
Custom separators and diagnostic bytes use the wire scalar encoder and an explicit output budget.

An optional `Issue` writes the v2.5.1 ERR-3 code from HL70357 (0, 100–103, 200–207), ERR-4 severity
and escaped ERR-7 diagnostics. Structured ERR-2 locations and multiple issues are not implemented.
Do not include patient data in diagnostics unless the interface's disclosure policy allows it.

Correlation checks the exact decoded MSA-2, version, trigger, processing ID, ACK structure, one MSA
and the expected phase. Ordinary ACKs cannot be acknowledged through `build`. Sender/receiver
authorization and channel identity belong to the host; these checks are not full schema validation.
Distinct messages need distinct control IDs within the channel's identity scope. Reusing an ID can
make a stale response indistinguishable from a current one.

ACK construction is deterministic for the supplied bytes and parameters. Durable receipt, recorded
responses for retries and clinical application remain the processing layer's responsibility.
