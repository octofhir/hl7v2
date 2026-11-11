#![no_main]
use libfuzzer_sys::fuzz_target;
use octofhir_hl7v2::{
    ack::{self, Code, Mode, Reply},
    wire::{Date, DateTime, Decimal, DiagnosticKind, Edit, Message, ParseOptions, Presence, Value},
};
use std::{collections::BTreeMap, ops::Range};

fn value(value: Value<'_>, bytes: &[u8], parent: Range<usize>, depth: usize) {
    assert!(depth <= 3);
    let span = value.span();
    assert!(span.is_some(), "indexed value has no source span");
    let Some(span) = span else { return };
    assert!(parent.start <= span.start && span.end <= parent.end);
    assert_eq!(value.raw(), Some(&bytes[span.clone()]));
    let _ = value.decode();
    if let Ok(decimal) = value.decimal() {
        assert!(matches!(value.decode(), Ok(bytes) if bytes.as_ref() == decimal.as_bytes()));
    }
    if let Ok(date) = value.date() {
        assert!(matches!(value.decode(), Ok(bytes) if bytes.as_ref() == date.as_bytes()));
    }
    if let Ok(timestamp) = value.date_time() {
        assert!(matches!(value.decode(), Ok(bytes) if bytes.as_ref() == timestamp.as_bytes()));
    }
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
            let field = [segment.occurrence(), n];
            timestamp(message, &field);
            for repetition in 1..=segment.field(n).child_count() {
                timestamp(message, &[field[0], n, repetition]);
                for component in 1..=segment.field(n).child(repetition).child_count() {
                    timestamp(message, &[field[0], n, repetition, component]);
                }
            }
        }
    }
    assert_eq!(
        message.value(&[usize::MAX; 5]).presence(),
        Presence::Missing
    );
}

fn timestamp(message: &Message, path: &[usize]) {
    if let Ok(value) = message.timestamp(path) {
        assert_eq!(value.source().raw(), message.value(path).raw());
        assert!(value.precision() <= value.encoded_time().precision());
        assert_eq!(
            value.declared_precision().is_some(),
            value.precision_presence() == Presence::Value
        );
    }
}

fuzz_target!(|bytes: &[u8]| {
    if bytes.len() > 16_384 {
        return;
    }
    // Exercise lexical validation even when arbitrary input is not an ER7 message.
    if let Ok(decimal) = Decimal::parse(bytes) {
        assert_eq!(decimal.as_bytes(), bytes);
        assert_eq!(decimal.as_bytes().as_ptr(), bytes.as_ptr());
        assert!(decimal.integer_digits().len() + decimal.scale() > 0);
    }
    if let Ok(date) = Date::parse(bytes) {
        assert_eq!(date.as_bytes(), bytes);
        assert!(DateTime::parse(bytes).is_ok());
    }
    if let Ok(timestamp) = DateTime::parse(bytes) {
        assert_eq!(timestamp.as_bytes(), bytes);
        assert!((1..=9999).contains(&timestamp.year()));
        if let Some(fraction) = timestamp.fraction_digits() {
            assert!((1..=4).contains(&fraction.len()));
        }
    }
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
    let mut creation = message.edit();
    if creation
        .set_or_create(&[1, 24, 2, 2, 2], Edit::Value(replacement.to_vec()))
        .is_ok()
        && let Ok(output) = creation.serialize()
    {
        let revised = Message::parse_with_options(output, options);
        assert!(
            revised.is_ok(),
            "created revision cannot be parsed: {revised:?}"
        );
        if let Ok(revised) = revised {
            assert!(
                matches!(revised.value(&[1, 24, 2, 2, 2]).decode(), Ok(value) if value.as_ref() == replacement)
            );
        }
    }
    if let Some(header) = message.segments().next() {
        let mut creation = message.edit();
        let mut expected = BTreeMap::new();
        for parameters in bytes.as_chunks::<4>().0.iter().take(8) {
            let path = [
                1,
                header.field_count() + 1 + usize::from(parameters[0] % 4),
                1 + usize::from(parameters[1] % 4),
                1 + usize::from(parameters[2] % 4),
                1 + usize::from(parameters[3] % 4),
            ];
            if creation
                .set_or_create(&path, Edit::Value(replacement.to_vec()))
                .is_ok()
            {
                expected.insert(path, replacement);
            }
        }
        if let Ok(output) = creation.serialize() {
            let revised = Message::parse_with_options(output, options);
            assert!(
                revised.is_ok(),
                "batch creation cannot be parsed: {revised:?}"
            );
            if let Ok(revised) = revised {
                for (path, bytes) in expected {
                    assert!(
                        matches!(revised.value(&path).decode(), Ok(value) if value.as_ref() == bytes)
                    );
                }
            }
        }
    }
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
        if parameters[0] & 4 == 0 {
            let _ = editor.set(&path, edit);
        } else {
            let _ = editor.set_or_create(&path, edit);
        }
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
