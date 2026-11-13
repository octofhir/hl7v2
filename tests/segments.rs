use octofhir_hl7v2::wire::{Edit, ErrorKind, Message, ParseOptions, Presence};
type Result = std::result::Result<(), Box<dyn std::error::Error>>;

#[test]
fn sparse_segments_insert_in_order_and_mix_with_original_coordinate_edits_and_removal() -> Result {
    let original = b"MSH|^~\\&|S\rPID|old\rOBX|untouched\rNTE|remove\r";
    let source = Message::parse(original)?;
    let value = b"x|^~\\&\0\r\xff";
    let mut segment = source.segment_builder(*b"PV1")?;
    for path in [&[3, 2, 2, 2][..], &[1]] {
        segment.set(path, Edit::Value(value.to_vec()))?;
    }
    segment
        .set(&[2], Edit::Null)?
        .set(&[4], Edit::Value(b"\"\"".to_vec()))?;
    segment.set(&[5], Edit::Empty)?;
    let mut editor = source.edit();
    editor.insert_segment(3, segment)?;
    editor.insert_segment(3, source.segment_builder(*b"ZZZ")?)?;
    editor.set(&[2, 1], Edit::Value(b"changed".to_vec()))?;
    editor.remove_segment(4)?.remove_segment(4)?;
    let revised = editor.finish()?;
    let ids: Vec<_> = revised.segments().map(|s| *s.id()).collect();
    assert_eq!(ids, [*b"MSH", *b"PID", *b"PV1", *b"ZZZ", *b"OBX"]);
    assert_eq!(revised.value(&[2, 1]).decode()?.as_ref(), b"changed");
    assert_eq!(revised.value(&[3, 1]).decode()?.as_ref(), value);
    assert_eq!(revised.value(&[3, 2]).presence(), Presence::Null);
    assert_eq!(revised.value(&[3, 3, 1]).presence(), Presence::Empty);
    assert_eq!(revised.value(&[3, 3, 2, 2, 1]).presence(), Presence::Empty);
    assert_eq!(revised.value(&[3, 3, 2, 2, 2]).decode()?.as_ref(), value);
    assert_eq!(revised.value(&[3, 4]).presence(), Presence::Value);
    assert_eq!(revised.value(&[3, 4]).decode()?.as_ref(), b"\"\"");
    assert_eq!(revised.value(&[3, 5]).presence(), Presence::Empty);
    assert_eq!(revised.value(&[3, 6]).presence(), Presence::Missing);
    assert_eq!(revised.segment(4).ok_or("new empty segment")?.raw(), b"ZZZ");
    assert_eq!(
        revised.segment(5).ok_or("retained OBX")?.raw(),
        b"OBX|untouched"
    );
    assert_eq!(source.original(), original);
    Ok(())
}

#[test]
fn splices_preserve_custom_delimiters_original_terminators_and_unterminated_tail_boundaries()
-> Result {
    for terminated in [false, true] {
        for remove in 0..4 {
            let mut bytes = b"MSH*$%!?*S\r\nZ01*old\nZ02*tail".to_vec();
            if terminated {
                bytes.push(b'\r');
            }
            let source = Message::parse_with_options(
                &bytes,
                ParseOptions {
                    compatible_line_endings: true,
                    ..Default::default()
                },
            )?;
            let mut editor = source.edit();
            editor.insert_segment(2, source.segment_builder(*b"Z03")?)?;
            for number in [2, 3] {
                if remove & (1 << (number - 2)) != 0 {
                    editor.remove_segment(number)?;
                }
            }
            if remove & 2 == 0 {
                editor.set(&[3, 1], Edit::Value(b"replacement".to_vec()))?;
                editor.set_or_create(&[3, 2], Edit::Null)?;
            }
            let mut tail = source.segment_builder(*b"NTE")?;
            tail.set(&[1, 2, 2, 2], Edit::Value(b"*%$!?\r\n\0".to_vec()))?;
            editor.insert_segment(4, tail)?;
            let output = editor.serialize()?;
            let revised = editor.finish()?;
            assert_eq!(revised.original(), output);
            assert_eq!(revised.delimiters(), source.delimiters());
            assert_eq!(revised.segment(1).ok_or("MSH")?.terminator(), b"\r\n");
            let mut expected = b"MSH*$%!?*S\r\nZ03\r".to_vec();
            if remove & 1 == 0 {
                expected.extend_from_slice(b"Z01*old\n");
            }
            if remove & 2 == 0 {
                expected.extend_from_slice(b"Z02*replacement*\"\"\r");
            }
            assert!(revised.original().starts_with(&expected));
            let last = revised.segments().last().ok_or("last segment")?;
            assert_eq!(last.id(), b"NTE");
            assert_eq!(last.terminator(), b"\r");
            assert_eq!(
                revised
                    .value(&[last.occurrence(), 1, 2, 2, 2])
                    .decode()?
                    .as_ref(),
                b"*%$!?\r\n\0"
            );
            assert_eq!(source.original(), bytes);
        }
    }
    let source = Message::parse(b"MSH|^~\\&|S")?;
    let mut editor = source.edit();
    editor.insert_segment(2, source.segment_builder(*b"Z99")?)?;
    assert_eq!(editor.serialize()?, b"MSH|^~\\&|S\rZ99\r");
    Ok(())
}

#[test]
fn conflicts_source_identity_header_protection_and_failed_operations_are_explicit() -> Result {
    let source = Message::parse(b"MSH|^~\\&|S\rZ99|old\r")?;
    for id in [*b"MSH", *b"bad", *b"Z|Z"] {
        assert!(source.segment_builder(id).is_err());
    }
    let mut draft = source.segment_builder(*b"Z01")?;
    for path in [&[][..], &[0], &[1, 1, 1, 1, 1], &[usize::MAX]] {
        assert!(draft.set(path, Edit::Empty).is_err());
    }
    draft.set(&[2, 1, 2], Edit::Null)?;
    assert!(matches!(draft.set(&[2], Edit::Empty), Err(e) if e.kind == ErrorKind::EditConflict));
    draft.set(&[2, 1, 2], Edit::Unset)?.set(&[2], Edit::Empty)?;
    assert!(
        matches!(draft.set(&[2, 1, 2], Edit::Null), Err(e) if e.kind == ErrorKind::EditConflict)
    );
    draft.set(&[2], Edit::Unset)?;
    let mut editor = source.edit();
    for before in [0, 1, 4, usize::MAX] {
        assert!(
            editor
                .insert_segment(before, source.segment_builder(*b"ZZZ")?)
                .is_err()
        );
    }
    for number in [0, 1, 3, usize::MAX] {
        assert!(editor.remove_segment(number).is_err());
    }
    let other = Message::parse(source.original())?;
    assert!(
        matches!(editor.insert_segment(2, other.segment_builder(*b"ZZZ")?), Err(e) if e.kind == ErrorKind::EditConflict)
    );
    assert_eq!(editor.serialize()?, source.original());
    editor.set(&[2, 1], Edit::Empty)?;
    assert!(matches!(editor.remove_segment(2), Err(e) if e.kind == ErrorKind::EditConflict));
    let mut editor = source.edit();
    editor.remove_segment(2)?;
    assert!(matches!(editor.set(&[2, 1], Edit::Null), Err(e) if e.kind == ErrorKind::EditConflict));
    let clone = source.clone();
    editor.insert_segment(2, clone.segment_builder(*b"Z01")?)?;
    editor.insert_segment(3, draft)?;
    assert_eq!(editor.serialize()?, b"MSH|^~\\&|S\rZ01\rZ01\r");
    let broken = Message::parse_with_options(
        b"MSH|^~\\&|S\rZ99|bad\\escape\r",
        ParseOptions {
            max_diagnostics: 0,
            ..Default::default()
        },
    )?;
    assert!(
        matches!(broken.edit().insert_segment(2, broken.segment_builder(*b"ZZZ")?), Err(e) if e.kind == ErrorKind::Escape)
    );
    assert!(matches!(broken.edit().remove_segment(2), Err(e) if e.kind == ErrorKind::Escape));
    Ok(())
}

#[test]
fn structural_growth_shares_byte_segment_and_node_budgets_with_sparse_field_creation() -> Result {
    let original = b"MSH|^~\\&|S\rZ99|old\r";
    let base = Message::parse(original)?;
    let source = Message::parse_with_options(
        original,
        ParseOptions {
            max_segments: 2,
            ..Default::default()
        },
    )?;
    let mut editor = source.edit();
    editor.insert_segment(2, source.segment_builder(*b"Z01")?)?;
    assert!(matches!(editor.serialize(), Err(e) if e.kind == ErrorKind::Limit));
    editor.remove_segment(2)?;
    assert_eq!(editor.finish()?.segments().len(), 2);
    let source = Message::parse_with_options(
        original,
        ParseOptions {
            max_nodes: base.node_count() + 4,
            ..Default::default()
        },
    )?;
    let mut draft = source.segment_builder(*b"ZZZ")?;
    draft.set(&[1], Edit::Null)?;
    let mut editor = source.edit();
    editor.insert_segment(3, draft)?;
    assert_eq!(editor.serialize()?.len(), original.len() + 7);
    editor.set_or_create(&[2, 2], Edit::Empty)?;
    assert!(matches!(editor.serialize(), Err(e) if e.kind == ErrorKind::Limit));
    // Removing original nodes gives no capacity credit within one sparse revision.
    editor.set(&[2, 2], Edit::Unset)?;
    editor.remove_segment(2)?;
    assert_eq!(editor.finish()?.node_count(), base.node_count());
    let source = Message::parse_with_options(
        original,
        ParseOptions {
            max_bytes: original.len() + 3,
            ..Default::default()
        },
    )?;
    let mut editor = source.edit();
    editor.insert_segment(3, source.segment_builder(*b"ZZZ")?)?;
    assert!(matches!(editor.serialize(), Err(e) if e.kind == ErrorKind::Limit));
    editor.remove_segment(2)?;
    assert!(editor.finish()?.original().ends_with(b"ZZZ\r"));
    let source = Message::parse_with_options(
        b"MSH|^~\\&|S",
        ParseOptions {
            max_bytes: 14,
            ..Default::default()
        },
    )?;
    let mut editor = source.edit();
    editor.insert_segment(2, source.segment_builder(*b"ZZZ")?)?;
    assert!(matches!(editor.serialize(), Err(e) if e.kind == ErrorKind::Limit)); // Includes the added boundary CR.
    let bounded = Message::parse_with_options(
        original,
        ParseOptions {
            max_nodes: base.node_count(),
            ..Default::default()
        },
    )?;
    let mut huge = bounded.segment_builder(*b"ZZZ")?;
    assert!(
        matches!(huge.set(&[base.node_count() + 1], Edit::Empty), Err(e) if e.kind == ErrorKind::Limit)
    );
    Ok(())
}

#[test]
fn messages_can_be_created_from_explicit_delimiters_without_template_bytes() -> Result {
    use octofhir_hl7v2::wire::Delimiters;
    for delimiters in [
        Delimiters {
            field: b'|',
            encoding: *b"^~\\&",
            truncation: None,
        },
        Delimiters {
            field: b'*',
            encoding: *b"$%!?",
            truncation: Some(b'#'),
        },
    ] {
        let source = Message::create(delimiters, ParseOptions::default())?;
        assert_eq!(source.node_count(), 2);
        assert_eq!(source.segments().len(), 1);
        assert!(source.diagnostics().is_empty());
        let mut value = source.segment_builder(*b"ZZZ")?;
        value
            .set(&[1], Edit::Value(b"NM".to_vec()))?
            .set(&[2], Edit::Value(b"+0001.2300".to_vec()))?;
        let mut note = source.segment_builder(*b"NTE")?;
        note.set(&[3], Edit::Value(b"literal|*#".to_vec()))?;
        let mut editor = source.edit();
        for (path, text) in [
            (&[1, 9, 1, 1][..], b"ZZZ".as_slice()),
            (&[1, 9, 1, 2], b"Z01"),
            (&[1, 9, 1, 3], b"SYNTHETIC"),
            (&[1, 12], b"TEST"),
        ] {
            editor.set_or_create(path, Edit::Value(text.to_vec()))?;
        }
        editor.insert_segment(2, value)?.insert_segment(2, note)?;
        let message = editor.finish()?;
        assert_eq!(message.delimiters(), delimiters);
        assert_eq!(
            message.value(&[1, 9, 1, 3]).decode()?.as_ref(),
            b"SYNTHETIC"
        );
        assert_eq!(message.value(&[1, 12]).decode()?.as_ref(), b"TEST");
        assert_eq!(message.value(&[2, 2]).decimal()?.as_bytes(), b"+0001.2300");
        assert_eq!(message.value(&[3, 3]).decode()?.as_ref(), b"literal|*#");
        #[cfg(feature = "schema-package")]
        {
            use octofhir_hl7v2::schema::{
                MatchLimits,
                fields::ValidationLimits,
                package::{LoadLimits, OwnedPackage},
            };
            let file = include_bytes!("fixtures/native-package-v1.bin");
            let pin = [
                0x01, 0x61, 0x08, 0x1c, 0x3e, 0xb1, 0x55, 0x36, 0xd2, 0x63, 0xba, 0x1b, 0x1f, 0x35,
                0x77, 0x25, 0xb0, 0xef, 0xc4, 0x3b, 0x7e, 0x2d, 0x02, 0xa5, 0x8b, 0x54, 0x1d, 0xda,
                0x1e, 0x66, 0x9c, 0xd1,
            ];
            let package = OwnedPackage::decode(file, pin, LoadLimits::default())?;
            let view = package.catalog().group(&message, MatchLimits::default())?;
            assert!(
                package
                    .fields()
                    .validate(&view, ValidationLimits::default())
                    .is_valid()
            );
        }
    }
    let standard = Delimiters {
        field: b'|',
        encoding: *b"^~\\&",
        truncation: None,
    };
    for bad in [
        Delimiters {
            field: b'"',
            ..standard
        },
        Delimiters {
            encoding: *b"^^\\&",
            ..standard
        },
        Delimiters {
            truncation: Some(b'|'),
            ..standard
        },
        Delimiters {
            truncation: Some(b'A'),
            ..standard
        },
    ] {
        assert!(Message::create(bad, ParseOptions::default()).is_err());
    }
    for limits in [
        ParseOptions {
            max_bytes: 8,
            ..Default::default()
        },
        ParseOptions {
            max_nodes: 1,
            ..Default::default()
        },
        ParseOptions {
            max_segments: 0,
            ..Default::default()
        },
    ] {
        assert!(matches!(Message::create(standard, limits), Err(e) if e.kind == ErrorKind::Limit));
    }
    Ok(())
}
