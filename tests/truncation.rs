use octofhir_hl7v2::wire::{ErrorKind, Message, ParseOptions};
type Result = std::result::Result<(), Box<dyn std::error::Error>>;

#[test]
fn truncated_scalars_never_decode_as_complete_values_even_without_diagnostics() -> Result {
    let input = b"MSH|^~\\&.|S\rZZZ|12.|12\\P\\|12\\X2E\\|\\X31\\.|1.\\X32\\|\\Z.\\|12.50\r";
    let message = Message::parse_with_options(
        input,
        ParseOptions {
            max_diagnostics: 0,
            ..Default::default()
        },
    )?;
    assert!(message.diagnostics().is_empty());
    for field in [1, 4] {
        let value = message.value(&[2, field]);
        let raw = value.raw().ok_or("missing value")?;
        let start = value.span().ok_or("missing span")?.start;
        let dot = raw
            .iter()
            .position(|&b| b == b'.')
            .ok_or("missing marker")?;
        let error = value
            .decimal()
            .err()
            .ok_or("truncation interpreted as a number")?;
        assert_eq!(error.kind, ErrorKind::Truncation);
        assert_eq!(error.span, start + dot..start + dot + 1);
    }
    for field in [2, 3] {
        assert_eq!(message.value(&[2, field]).decimal()?.as_bytes(), b"12.");
    }
    assert!(matches!(message.value(&[2, 6]).decode(), Err(e) if e.kind == ErrorKind::Escape));
    assert_eq!(message.value(&[1, 2]).decode()?.as_ref(), b"^~\\&.");
    assert_eq!(message.value(&[2, 5]).decimal()?.as_bytes(), b"1.2");
    assert_eq!(message.value(&[2, 7]).decimal()?.as_bytes(), b"12.50");
    let diagnosed = Message::parse(input)?;
    assert_eq!(
        diagnosed
            .diagnostics()
            .iter()
            .filter(|d| d.kind == octofhir_hl7v2::wire::DiagnosticKind::Truncation)
            .count(),
        2
    );
    assert_eq!(message.edit().serialize()?, input);
    Ok(())
}
