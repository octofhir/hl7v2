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

fn numeric_oracle(bytes: &[u8]) {
    let mut data = [0; 18];
    for (slot, byte) in data.iter_mut().zip(bytes) {
        *slot = *byte;
    }
    let left = i64::from_le_bytes(std::array::from_fn(|i| data[i]));
    let right = i64::from_le_bytes(std::array::from_fn(|i| data[i + 8]));
    let left_scale = u32::from(data[16] % 7);
    let right_scale = u32::from(data[17] % 7);
    let spell = |n: i64, scale: u32| {
        let magnitude = i128::from(n).abs();
        let factor = 10_i128.pow(scale);
        let sign = if n < 0 { "-" } else { "+" };
        format!(
            "{sign}{}.{:0width$}",
            magnitude / factor,
            magnitude % factor,
            width = scale as usize
        )
    };
    let a_text = spell(left, left_scale);
    let b_text = spell(right, right_scale);
    let a = Decimal::parse(a_text.as_bytes());
    let b = Decimal::parse(b_text.as_bytes());
    assert!(a.is_ok() && b.is_ok());
    if let (Ok(a), Ok(b)) = (a, b) {
        let normalized = i128::from(left) * 10_i128.pow(6 - left_scale);
        let other = i128::from(right) * 10_i128.pow(6 - right_scale);
        assert_eq!(a.numeric_cmp(&b), normalized.cmp(&other));
        assert_eq!(b.numeric_cmp(&a), other.cmp(&normalized));
        assert_eq!(a.is_zero(), left == 0);
        let precision = 1 + u32::from(data[0] % 24);
        let scale = u32::from(data[1]) % (precision + 1);
        let fits = normalized.abs() < 10_i128.pow(precision - scale) * 1_000_000
            && (scale >= 6 || normalized % 10_i128.pow(6 - scale) == 0);
        assert_eq!(
            a.fits_precision_scale(precision as usize, scale as usize),
            fits
        );
    }
}

fuzz_target!(|bytes: &[u8]| {
    if bytes.len() > 16_384 {
        return;
    }
    numeric_oracle(bytes);
    // Exercise lexical validation even when arbitrary input is not an ER7 message.
    if let Ok(decimal) = Decimal::parse(bytes) {
        assert_eq!(decimal.as_bytes(), bytes);
        assert_eq!(decimal.as_bytes().as_ptr(), bytes.as_ptr());
        assert!(decimal.integer_digits().len() + decimal.scale() > 0);
        let mut equivalent = Vec::new();
        if let Some(sign) = decimal.sign() {
            equivalent.push(sign);
        }
        equivalent.extend_from_slice(b"000");
        equivalent.extend_from_slice(decimal.integer_digits());
        equivalent.push(b'.');
        equivalent.extend_from_slice(decimal.fraction_digits().unwrap_or(&[]));
        equivalent.extend_from_slice(b"00");
        let alternate = Decimal::parse(&equivalent);
        assert!(alternate.is_ok());
        if let Ok(alternate) = alternate {
            assert_eq!(decimal.numeric_cmp(&alternate), std::cmp::Ordering::Equal);
            let precision = bytes.len() % 32;
            let scale = usize::from(bytes[0] % 32);
            assert_eq!(
                decimal.fits_precision_scale(precision, scale),
                alternate.fits_precision_scale(precision, scale)
            );
        }
        let middle = bytes.len() / 2;
        if let (Ok(left), Ok(right)) = (
            Decimal::parse(&bytes[..middle]),
            Decimal::parse(&bytes[middle..]),
        ) {
            assert_eq!(left.numeric_cmp(&right), right.numeric_cmp(&left).reverse());
        }
        assert_eq!(decimal.as_bytes(), bytes);
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
    let created = Message::create(message.delimiters(), options);
    assert!(created.is_ok());
    if let Ok(created) = created {
        assert_eq!(created.delimiters(), message.delimiters());
        assert_eq!(created.node_count(), 2);
        let draft = created.segment_builder(*b"Z01");
        assert!(draft.is_ok());
        if let Ok(mut draft) = draft {
            assert!(
                draft
                    .set(&[2, 2, 2, 2], Edit::Value(replacement.to_vec()))
                    .is_ok()
            );
            let mut editor = created.edit();
            assert!(editor.insert_segment(2, draft).is_ok());
            let revised = editor.finish();
            assert!(revised.is_ok());
            if let Ok(revised) = revised {
                check(&revised, options);
                assert!(
                    matches!(revised.value(&[2, 2, 2, 2, 2]).decode(), Ok(v) if v.as_ref() == replacement)
                );
            }
        }
    }
    // Structural edits use original coordinates even when a removed segment is the insertion anchor.
    let before = 2 + bytes.len() % message.segments().len();
    let removed = (message.segments().len() > 1 && bytes.len().is_multiple_of(3))
        .then(|| 2 + bytes.len() % (message.segments().len() - 1));
    let draft = message.segment_builder(*b"Z99");
    assert!(draft.is_ok());
    if let Ok(mut draft) = draft
        && draft.set(&[1], Edit::Value(replacement.to_vec())).is_ok()
        && draft
            .set(&[3, 2, 2, 2], Edit::Value(replacement.to_vec()))
            .is_ok()
    {
        let mut structural = message.edit();
        if let Some(removed) = removed {
            let _ = structural.remove_segment(removed);
        }
        if structural.insert_segment(before, draft).is_ok()
            && let Ok(output) = structural.serialize()
        {
            let revised = Message::parse_with_options(output, options);
            assert!(
                revised.is_ok(),
                "structural revision cannot be parsed: {revised:?}"
            );
            if let Ok(revised) = revised {
                check(&revised, options);
                let inserted = before - usize::from(removed.is_some_and(|r| r < before));
                assert_eq!(revised.segment(inserted).map(|s| *s.id()), Some(*b"Z99"));
                for path in [&[inserted, 1][..], &[inserted, 3, 2, 2, 2]] {
                    assert!(
                        matches!(revised.value(path).decode(), Ok(v) if v.as_ref() == replacement)
                    );
                }
                for old in message.segments() {
                    if removed == Some(old.occurrence()) {
                        continue;
                    }
                    let occurrence = old.occurrence() + usize::from(before <= old.occurrence())
                        - usize::from(removed.is_some_and(|r| r < old.occurrence()));
                    assert_eq!(
                        revised.segment(occurrence).map(|s| s.raw()),
                        Some(old.raw())
                    );
                    if !old.terminator().is_empty() {
                        assert_eq!(
                            revised.segment(occurrence).map(|s| s.terminator()),
                            Some(old.terminator())
                        );
                    }
                }
            }
        }
    }
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
