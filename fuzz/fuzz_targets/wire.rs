#![no_main]
use libfuzzer_sys::fuzz_target;
use octofhir_hl7v2::{
    ack::{self, Code, Mode, Reply},
    wire::{DiagnosticKind, Edit, Message, ParseOptions, Presence, Value},
};
use std::ops::Range;

fn value(value: Value<'_>, bytes: &[u8], parent: Range<usize>, depth: usize) {
    assert!(depth <= 3);
    let span = value.span();
    assert!(span.is_some(), "indexed value has no source span");
    let Some(span) = span else { return };
    assert!(parent.start <= span.start && span.end <= parent.end);
    assert_eq!(value.raw(), Some(&bytes[span.clone()]));
    let _ = value.decode();
    assert_eq!(value.child(0).presence(), Presence::Missing);
    assert_eq!(value.child(usize::MAX).presence(), Presence::Missing);
    for n in 1..=value.child_count() {
        self::value(value.child(n), bytes, span.clone(), depth + 1);
    }
}

fn check(message: &Message, options: ParseOptions) {
    let bytes = message.original();
    assert!(bytes.len() <= options.max_bytes);
    assert!(message.node_count() <= options.max_nodes);
    assert!(message.diagnostics().len() <= options.max_diagnostics);
    assert!(message.segments().len() <= options.max_segments);
    for diagnostic in message.diagnostics() {
        assert!(diagnostic.span.start <= diagnostic.span.end && diagnostic.span.end <= bytes.len());
    }
    for segment in message.segments() {
        assert_eq!(segment.raw(), &bytes[segment.span()]);
        for n in 1..=segment.field_count() {
            value(segment.field(n), bytes, segment.span(), 0);
        }
    }
    assert_eq!(
        message.value(&[usize::MAX; 5]).presence(),
        Presence::Missing
    );
}

fuzz_target!(|bytes: &[u8]| {
    let options = ParseOptions {
        compatible_line_endings: bytes.len().is_multiple_of(2),
        max_bytes: 16_384,
        max_segments: 256,
        max_nodes: 8192,
        max_diagnostics: bytes.len() % 17,
    };
    let Ok(message) = Message::parse_with_options(bytes, options) else {
        return;
    };
    check(&message, options);
    assert_eq!(message.edit().serialize().as_deref(), Ok(bytes));
    let replacement = &bytes[..bytes.len().min(128)];
    let mut scalar = message.edit();
    if scalar
        .set(&[1, 3], Edit::Value(replacement.to_vec()))
        .is_ok()
        && let Ok(output) = scalar.serialize()
    {
        let revised = Message::parse_with_options(output, options);
        assert!(
            revised.is_ok(),
            "scalar revision cannot be parsed: {revised:?}"
        );
        if let Ok(revised) = revised {
            assert!(
                matches!(revised.value(&[1, 3]).decode(), Ok(value) if value.as_ref() == replacement)
            );
        }
    }
    let mut editor = message.edit();
    for parameters in bytes.chunks(6).take(8) {
        let segment = usize::from(parameters[0]) % message.segments().len() + 1;
        let mut path = vec![segment];
        let depth = usize::from(parameters.get(5).copied().unwrap_or(0)) % 4 + 2;
        for &n in parameters[1..].iter().take(depth - 1) {
            path.push(usize::from(n) % 8 + 1);
        }
        let edit = match parameters[0] % 4 {
            0 => Edit::Value(bytes[..bytes.len().min(128)].to_vec()),
            1 => Edit::Empty,
            2 => Edit::Null,
            _ => Edit::Unset,
        };
        let _ = editor.set(&path, edit);
    }
    if let Ok(output) = editor.serialize() {
        let revised = Message::parse_with_options(&output, options);
        assert!(
            revised.is_ok(),
            "serialized revision cannot be parsed: {revised:?}"
        );
        if let Ok(revised) = revised {
            check(&revised, options);
        }
    }
    for (mode, code) in [
        (Mode::Original, Code::AA),
        (Mode::EnhancedAcceptOnly, Code::CA),
    ] {
        if ack::validate_request(&message, mode).is_err() {
            continue;
        }
        let reply = Reply {
            code,
            control_id: b"fuzz-ack",
            timestamp: b"20261111120000",
            issue: None,
        };
        if let Ok(output) = ack::build(&message, reply, options.max_bytes) {
            let response = Message::parse(&output);
            assert!(
                response.is_ok(),
                "generated ACK cannot be parsed: {response:?}"
            );
            if let Ok(response) = response {
                assert!(!response.diagnostics().iter().any(|d| matches!(
                    d.kind,
                    DiagnosticKind::UnterminatedEscape | DiagnosticKind::InvalidHex
                )));
                assert!(ack::correlate(&message, response, mode).is_ok());
            }
        }
    }
});
