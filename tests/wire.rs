use octofhir_hl7v2::wire::{
    DiagnosticKind, Edit, ErrorKind, Message, ParseOptions, Presence, Value,
};
use std::sync::Arc;

type Result = std::result::Result<(), Box<dyn std::error::Error>>;
const HEADER: &[u8] = b"MSH|^~\\&|SYNTHETIC\r";

#[test]
fn corpus_round_trip_preserves_bytes_and_shared_ownership() -> Result {
    for bytes in [
        include_bytes!("fixtures/adt.er7").as_slice(),
        include_bytes!("fixtures/oru.er7").as_slice(),
        include_bytes!("fixtures/custom.er7").as_slice(),
        include_bytes!("fixtures/binary.er7").as_slice(),
    ] {
        let source: Arc<[u8]> = Arc::from(bytes);
        let message = Message::from_shared(source.clone(), ParseOptions::default())?;
        assert_eq!(message.original().as_ptr(), source.as_ptr());
        assert_eq!(message.clone().original().as_ptr(), source.as_ptr());
        assert_eq!(message.edit().serialize()?, bytes);
        for segment in message.segments() {
            assert_eq!(&bytes[segment.span()], segment.raw());
        }
    }
    Ok(())
}

#[test]
fn positions_preserve_nulls_empty_tails_repetitions_and_unknown_segments() -> Result {
    let message =
        Message::parse([HEADER, b"ZZZ|\"\"||a~~b~|x^^y|p&&q&\rZZZ|last|\rZ99\r"].concat())?;
    assert_eq!(message.value(&[1, 1]).span(), Some(3..4));
    assert_eq!(message.value(&[1, 2]).decode()?.as_ref(), b"^~\\&");
    for (path, expected) in [
        (&[2, 1][..], Presence::Null),
        (&[2, 2], Presence::Empty),
        (&[2, 6], Presence::Missing),
        (&[2, 3], Presence::Value),
        (&[2, 3, 2], Presence::Empty),
        (&[2, 3, 4], Presence::Empty),
        (&[2, 4, 1, 2], Presence::Empty),
        (&[2, 5, 1, 1, 4], Presence::Empty),
        (&[3, 2], Presence::Empty),
        (&[4, 1], Presence::Missing),
    ] {
        assert_eq!(message.value(path).presence(), expected, "{path:?}");
    }
    assert!(
        message
            .value(&[2, 2])
            .span()
            .is_some_and(|span| span.is_empty())
    );
    assert_eq!(message.value(&[2, 6]).span(), None);
    assert_eq!(
        message
            .segment_named(b"ZZZ", 2)
            .ok_or("missing second ZZZ")?
            .occurrence(),
        3
    );
    for path in [
        vec![],
        vec![0, 1],
        vec![1, 0],
        vec![1, usize::MAX],
        vec![1, 1, 1, 1, 1, 1],
    ] {
        assert_eq!(message.value(&path).presence(), Presence::Missing);
    }
    for (path, error) in [
        (&[2, 1][..], ErrorKind::Null),
        (&[2, 3], ErrorKind::Composite),
        (&[99, 1], ErrorKind::Missing),
    ] {
        assert!(matches!(message.value(path).decode(), Err(e) if e.kind == error));
    }
    Ok(())
}

#[test]
fn escapes_decode_once_and_unsupported_data_stays_visible() -> Result {
    let message = Message::parse(
        [
            HEADER,
            b"ZZZ|\\E\\F\\E\\|\\X0D7CFF\\|\\X1\\|\\Zsite|data\\|tail\\oops\r",
        ]
        .concat(),
    )?;
    assert_eq!(message.value(&[2, 1]).decode()?.as_ref(), b"\\F\\");
    assert_eq!(message.value(&[2, 2]).decode()?.as_ref(), b"\r|\xff");
    assert!(
        matches!(message.edit().set(&[2, 1], Edit::Value(b"|".to_vec())), Err(e) if e.kind == ErrorKind::Escape)
    );
    let hidden = Message::parse_with_options(
        message.original(),
        ParseOptions {
            max_diagnostics: 0,
            ..Default::default()
        },
    )?;
    assert!(hidden.diagnostics().is_empty());
    assert!(
        matches!(hidden.edit().set(&[2, 1], Edit::Empty), Err(e) if e.kind == ErrorKind::Escape)
    );
    assert_eq!(message.segments().len(), 2);
    assert_eq!(
        message.value(&[2, 4]).raw(),
        Some(b"\\Zsite|data\\".as_slice())
    );
    assert!(matches!(message.value(&[2, 3]).decode(), Err(e) if e.kind == ErrorKind::InvalidHex));
    assert!(matches!(message.value(&[2, 4]).decode(), Err(e) if e.kind == ErrorKind::Escape));
    assert_eq!(
        message
            .diagnostics()
            .iter()
            .map(|d| d.kind)
            .collect::<Vec<_>>(),
        [
            DiagnosticKind::InvalidHex,
            DiagnosticKind::UnsupportedEscape,
            DiagnosticKind::UnterminatedEscape
        ]
    );
    for diagnostic in message.diagnostics() {
        let span = message
            .value(&diagnostic.path)
            .span()
            .ok_or("missing diagnostic source")?;
        assert!(span.start <= diagnostic.span.start && diagnostic.span.end <= span.end);
    }
    Ok(())
}

#[test]
fn compatibility_and_limits_are_explicit() -> Result {
    let input = b"MSH|^~\\&| APP \r\nZZZ|value  \n";
    assert!(matches!(Message::parse(input), Err(e) if e.kind == ErrorKind::LineEnding));
    let message = Message::parse_with_options(
        input,
        ParseOptions {
            compatible_line_endings: true,
            ..Default::default()
        },
    )?;
    assert_eq!(message.edit().serialize()?, input);
    assert_eq!(message.value(&[2, 1]).raw(), Some(b"value  ".as_slice()));
    for options in [
        ParseOptions {
            max_bytes: 4,
            ..Default::default()
        },
        ParseOptions {
            max_nodes: 2,
            ..Default::default()
        },
        ParseOptions {
            max_segments: 1,
            ..Default::default()
        },
    ] {
        assert!(
            matches!(Message::parse_with_options(input, ParseOptions { compatible_line_endings: true, ..options }), Err(e) if e.kind == ErrorKind::Limit)
        );
    }
    let message = Message::parse_with_options(
        [HEADER, b"ZZZ|\\Q\\|\\W\\|\\Y\\\r"].concat(),
        ParseOptions {
            max_diagnostics: 1,
            ..Default::default()
        },
    )?;
    assert_eq!(message.diagnostics().len(), 1);
    assert_eq!(message.omitted_diagnostics(), 2);
    for input in [
        b"".as_slice(),
        b"MSH",
        b"MSH|^^\\&|x",
        b"MSH|^~\\&|x\r\r",
        b"MSH|^~\\&|x\rMSH|^~\\&|y",
    ] {
        assert!(Message::parse(input).is_err());
    }
    let message = Message::parse_with_options(
        b"MSH|^~\\&|x\rZZZ|old\r",
        ParseOptions {
            max_bytes: 26,
            ..Default::default()
        },
    )?;
    let mut editor = message.edit();
    assert!(
        matches!(editor.set(&[2, 1], Edit::Value(vec![b'|'; 26])), Err(e) if e.kind == ErrorKind::Limit)
    );
    editor.set(&[2, 1], Edit::Value(vec![b'a'; 20]))?;
    assert!(matches!(editor.serialize(), Err(e) if e.kind == ErrorKind::Limit));
    Ok(())
}

#[test]
fn patches_preserve_untouched_wire_and_never_shift_positions() -> Result {
    let original = [HEADER, b"ZZZ|a^b|\\X41\\|private-marker|last\r"].concat();
    let message = Message::parse(&original)?;
    let mut editor = message.edit();
    editor.set(&[2, 1, 1, 2], Edit::Value(b"|^~\\&\r\n\xff".to_vec()))?;
    editor.set(&[2, 3], Edit::Null)?;
    editor.set(&[2, 4], Edit::Unset)?;
    let revised = editor.finish()?;
    assert_eq!(
        revised.value(&[2, 1, 1, 2]).decode()?.as_ref(),
        b"|^~\\&\r\n\xff"
    );
    assert_eq!(revised.value(&[2, 2]).raw(), Some(b"\\X41\\".as_slice()));
    assert_eq!(revised.value(&[2, 3]).presence(), Presence::Null);
    assert_eq!(revised.value(&[2, 4]).presence(), Presence::Missing);
    assert_eq!(message.original(), original);
    assert!(!format!("{message:?}").contains("private-marker"));
    let mut editor = message.edit();
    assert!(
        matches!(editor.set(&[2, 1], Edit::Unset), Err(e) if e.kind == ErrorKind::InteriorUnset)
    );
    assert!(editor.set(&[1, 2], Edit::Empty).is_err());
    assert!(editor.set(&[2, 99], Edit::Empty).is_err());
    editor.set(&[2, 99, 1, 2], Edit::Unset)?;
    editor.set(&[2, 1], Edit::Empty)?;
    assert!(
        matches!(editor.set(&[2, 1, 1, 2], Edit::Null), Err(e) if e.kind == ErrorKind::EditConflict)
    );
    assert_eq!(editor.finish()?.value(&[2, 1]).presence(), Presence::Empty);
    let mut editor = message.edit();
    editor.set(&[2, 1, 1, 2], Edit::Empty)?;
    assert!(matches!(editor.set(&[2, 1], Edit::Null), Err(e) if e.kind == ErrorKind::EditConflict));
    let mut editor = message.edit();
    editor.set(&[2, 1], Edit::Value(b"\"\"".to_vec()))?;
    let revised = editor.finish()?;
    assert_eq!(revised.value(&[2, 1]).presence(), Presence::Value);
    assert_eq!(revised.value(&[2, 1]).decode()?.as_ref(), b"\"\"");
    let empty = Message::parse([HEADER, b"ZZZ||\r"].concat())?;
    let mut editor = empty.edit();
    editor.set(&[2, 1], Edit::Value(b"keep".to_vec()))?;
    editor.set(&[2, 2], Edit::Unset)?;
    assert_eq!(editor.finish()?.value(&[2, 1]).decode()?.as_ref(), b"keep");
    Ok(())
}

fn random(seed: &mut u64) -> usize {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed as usize
}

fn check_spans(value: Value<'_>, bytes: &[u8]) -> Result {
    let span = value.span().ok_or("missing indexed value")?;
    assert_eq!(value.raw(), Some(&bytes[span.clone()]));
    let _ = value.decode();
    for number in 1..=value.child_count() {
        let child = value.child(number);
        let child_span = child.span().ok_or("missing indexed child")?;
        assert!(span.start <= child_span.start && child_span.end <= span.end);
        check_spans(child, bytes)?;
    }
    Ok(())
}

#[test]
fn generated_inputs_and_custom_delimiter_edits_are_bounded() -> Result {
    let mut seed = 0x9a_13_65_7c_31;
    let options = ParseOptions {
        max_bytes: 1024,
        max_nodes: 512,
        max_diagnostics: 8,
        ..Default::default()
    };
    for _ in 0..10_000 {
        let length = random(&mut seed) % 512;
        let bytes: Vec<_> = HEADER
            .iter()
            .copied()
            .chain(b"ZZZ|".iter().copied())
            .chain((0..length).map(|_| random(&mut seed) as u8))
            .collect();
        if let Ok(message) = Message::parse_with_options(&bytes, options) {
            assert_eq!(message.edit().serialize()?, bytes);
            assert!(message.node_count() <= options.max_nodes);
            assert!(message.diagnostics().len() <= options.max_diagnostics);
            for segment in message.segments() {
                for number in 1..=segment.field_count() {
                    check_spans(segment.field(number), &bytes)?;
                }
            }
        }
    }
    let mut delimiters = b"!#$%&'()*+,-./:;<=>?@[\\]^_`{|}~".to_vec();
    for _ in 0..2000 {
        for i in (1..delimiters.len()).rev() {
            let n = random(&mut seed) % (i + 1);
            delimiters.swap(i, n);
        }
        let mut bytes = b"MSH".to_vec();
        bytes.extend_from_slice(&delimiters[..6]);
        bytes.push(delimiters[0]);
        bytes.extend_from_slice(b"APP\rZZZ");
        bytes.push(delimiters[0]);
        bytes.extend_from_slice(b"old\r");
        let message = Message::parse(bytes)?;
        let value: Vec<_> = (0..random(&mut seed) % 128)
            .map(|_| random(&mut seed) as u8)
            .collect();
        let mut editor = message.edit();
        editor.set(&[2, 1], Edit::Value(value.clone()))?;
        let revised = editor.finish()?;
        assert_eq!(revised.value(&[2, 1]).decode()?.as_ref(), value);
        assert!(revised.diagnostics().is_empty());
    }
    Ok(())
}
