use octofhir_hl7v2::wire::{Edit, ErrorKind, Message, ParseOptions, Presence};
type Result = std::result::Result<(), Box<dyn std::error::Error>>;

#[test]
fn sparse_creation_is_order_independent_and_preserves_positions_at_every_depth() -> Result {
    let input = b"MSH|^~\\&|S\rZZZ|base\rZ99\r";
    let message = Message::parse(input)?;
    let operations = [
        (&[2, 1, 1, 2, 3][..], b"x|^~\\&\0".as_slice(), 0),
        (&[2, 1, 3, 2], b"r3", 0),
        (&[2, 4, 2, 2, 2], b"far", 0),
        (&[2, 2], b"", 1),
        (&[2, 3, 1, 2], b"", 2),
        (&[3, 2], b"\"\"", 0),
    ];
    let expected = b"MSH|^~\\&|S\rZZZ|base^&&x\\F\\\\S\\\\R\\\\E\\\\T\\\\X00\\~~^r3||^\"\"|~^&far\rZ99||\\X2222\\\r";
    for order in [[0, 1, 2, 3, 4, 5], [5, 4, 3, 2, 1, 0], [2, 0, 4, 1, 5, 3]] {
        let mut editor = message.edit();
        for index in order {
            let (path, bytes, kind) = operations[index];
            let edit = match kind {
                1 => Edit::Empty,
                2 => Edit::Null,
                _ => Edit::Value(bytes.to_vec()),
            };
            editor.set_or_create(path, edit)?;
        }
        assert_eq!(editor.serialize()?, expected);
        let revised = editor.finish()?;
        assert_eq!(revised.segments().len(), 3);
        assert_eq!(revised.value(&[2, 1, 1, 1]).decode()?.as_ref(), b"base");
        assert_eq!(revised.value(&[2, 1, 2]).presence(), Presence::Empty);
        assert_eq!(revised.value(&[2, 4, 1]).presence(), Presence::Empty);
        assert_eq!(revised.value(&[3, 1]).presence(), Presence::Empty);
        for (path, bytes, kind) in operations {
            match kind {
                1 => assert_eq!(revised.value(path).presence(), Presence::Empty),
                2 => assert_eq!(revised.value(path).presence(), Presence::Null),
                _ => assert_eq!(revised.value(path).decode()?.as_ref(), bytes),
            }
        }
    }
    assert_eq!(message.original(), input);
    let empty = Message::parse(b"MSH|^~\\&|S\rZZZ|\r")?;
    let mut editor = empty.edit();
    for (path, bytes) in [
        (&[2, 2][..], b"f".as_slice()),
        (&[2, 1, 2], b"r"),
        (&[2, 1, 1, 2], b"c"),
        (&[2, 1, 1, 1, 1], b"tail"),
    ] {
        editor.set_or_create(path, Edit::Value(bytes.to_vec()))?;
    }
    assert_eq!(editor.serialize()?, b"MSH|^~\\&|S\rZZZ|tail^c~r|f\r");
    Ok(())
}

#[test]
fn creations_cannot_shift_after_unset_or_expand_null_and_are_bounded_before_growth() -> Result {
    let input = b"MSH|^~\\&|S\rZZZ|a\r";
    let message = Message::parse(input)?;
    let mut editor = message.edit();
    editor.set_or_create(&[2, 3, 1, 2], Edit::Null)?;
    let before = editor.serialize()?;
    assert!(
        matches!(editor.set_or_create(&[2, 3], Edit::Empty), Err(e) if e.kind == ErrorKind::EditConflict)
    );
    assert!(
        matches!(editor.set(&[2, 1], Edit::Unset), Err(e) if e.kind == ErrorKind::EditConflict)
    );
    assert_eq!(editor.serialize()?, before);
    editor.set_or_create(&[2, 3, 1, 2], Edit::Value(b"replaced".to_vec()))?;
    editor.set(&[2, 3, 1, 2], Edit::Unset)?;
    assert_eq!(editor.serialize()?, input);
    let mut editor = message.edit();
    editor.set(&[2, 1], Edit::Unset)?;
    assert!(
        matches!(editor.set_or_create(&[2, 3], Edit::Empty), Err(e) if e.kind == ErrorKind::EditConflict)
    );
    assert_eq!(editor.serialize()?, b"MSH|^~\\&|S\rZZZ\r");
    for path in [&[99, 1][..], &[1, 2, 1, 2], &[2, 0], &[2, usize::MAX]] {
        assert!(message.edit().set_or_create(path, Edit::Empty).is_err());
    }
    let null = Message::parse(b"MSH|^~\\&|S\rZZZ|\"\"\r")?;
    assert!(
        matches!(null.edit().set_or_create(&[2, 1, 2], Edit::Empty), Err(e) if e.kind == ErrorKind::Null)
    );
    let bounded = Message::parse_with_options(
        input,
        ParseOptions {
            max_nodes: message.node_count() + 1,
            ..Default::default()
        },
    )?;
    let mut editor = bounded.edit();
    editor.set_or_create(&[2, 1, 1, 1, 2], Edit::Empty)?;
    assert_eq!(editor.finish()?.node_count(), bounded.node_count() + 1);
    let mut editor = bounded.edit();
    editor.set_or_create(&[2, 2], Edit::Empty)?;
    assert!(matches!(editor.serialize(), Err(e) if e.kind == ErrorKind::Limit));
    let bounded = Message::parse_with_options(
        input,
        ParseOptions {
            max_bytes: input.len() + 2,
            ..Default::default()
        },
    )?;
    let mut editor = bounded.edit();
    editor.set_or_create(&[2, 10], Edit::Empty)?;
    assert!(matches!(editor.serialize(), Err(e) if e.kind == ErrorKind::Limit));
    Ok(())
}

#[test]
fn creation_preserves_compatible_terminators_and_refuses_hidden_unterminated_escapes() -> Result {
    for input in [
        b"MSH*$%!?*S\r\nZZZ*old\r\n".as_slice(),
        b"MSH*$%!?*S\nZZZ*old\n",
        b"MSH*$%!?*S\rZZZ*old",
    ] {
        let message = Message::parse_with_options(
            input,
            ParseOptions {
                compatible_line_endings: true,
                ..Default::default()
            },
        )?;
        let mut editor = message.edit();
        editor.set_or_create(&[2, 3, 2, 2, 2], Edit::Value(b"*$%!?\r\xff".to_vec()))?;
        let revised = editor.finish()?;
        assert_eq!(
            revised.value(&[2, 3, 2, 2, 2]).decode()?.as_ref(),
            b"*$%!?\r\xff"
        );
        assert_eq!(revised.value(&[2, 2]).presence(), Presence::Empty);
        for (old, new) in message.segments().zip(revised.segments()) {
            assert_eq!(old.terminator(), new.terminator());
        }
    }
    let malformed = Message::parse_with_options(
        b"MSH|^~\\&|S\rZZZ|x\\oops\r",
        ParseOptions {
            max_diagnostics: 0,
            ..Default::default()
        },
    )?;
    assert!(
        matches!(malformed.edit().set_or_create(&[2, 2], Edit::Empty), Err(e) if e.kind == ErrorKind::Escape)
    );
    Ok(())
}
