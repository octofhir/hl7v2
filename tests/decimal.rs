use octofhir_hl7v2::wire::{Decimal, Message};
use std::cmp::Ordering;
type Result = std::result::Result<(), Box<dyn std::error::Error>>;

#[test]
fn numeric_order_ignores_only_numerically_insignificant_spelling() -> Result {
    let classes: &[&[&[u8]]] = &[
        &[b"-1000", b"-01000.000"],
        &[b"-1", b"-0001.0"],
        &[b"-.10", b"-0.10000"],
        &[b"-0.00001"],
        &[b"0", b"-0.", b"+.0", b"00000.00000", b"-000.000"],
        &[b".00001", b"+000.0000100"],
        &[b".10", b"+0.10000"],
        &[b"1", b"+0001.0", b"01."],
        &[b"9007199254740992", b"+09007199254740992.000"],
        &[b"9007199254740993"],
    ];
    for (i, left) in classes.iter().enumerate() {
        for &left in *left {
            let a = Decimal::parse(left)?;
            assert_eq!(a.is_zero(), i == 4);
            for (j, right) in classes.iter().enumerate() {
                for &right in *right {
                    let b = Decimal::parse(right)?;
                    assert_eq!(a.numeric_cmp(&b), i.cmp(&j));
                    assert_eq!(b.numeric_cmp(&a), j.cmp(&i));
                }
            }
            assert_eq!(a.as_bytes(), left);
            assert_eq!(a.as_bytes().as_ptr(), left.as_ptr());
        }
    }
    Ok(())
}

#[test]
fn representability_checks_exact_value_without_rounding_or_changing_lexical_scale() -> Result {
    for (text, precision, scale, expected) in [
        ("+0001.2300", 3, 2, true),
        ("+0001.2300", 3, 1, false),
        ("-999.99000", 5, 2, true),
        ("1000", 5, 2, false),
        (".999", 3, 3, true),
        ("1", 3, 3, false),
        ("0.00100", 3, 3, true),
        ("0.00100", 3, 2, false),
        ("-0.00000", 1, 0, true),
        ("-0.", 1, 1, true),
        ("5.", 1, 0, true),
        (".0", 0, 0, false),
        ("0", 2, 3, false),
        ("1.23", usize::MAX, usize::MAX, false),
        ("1.23", usize::MAX, usize::MAX - 1, true),
        (".0", usize::MAX, usize::MAX, true),
    ] {
        let value = Decimal::parse(text.as_bytes())?;
        let original_scale = value.scale();
        assert_eq!(
            value.fits_precision_scale(precision, scale),
            expected,
            "{text}, {precision}, {scale}"
        );
        assert_eq!(value.scale(), original_scale);
        assert_eq!(value.as_bytes(), text.as_bytes());
    }
    // The views work across borrowed standalone bytes and owned escape decoding without changing spans.
    let wire = b"MSH|^~\\&\rZZZ|\\X2B3030312E32333030\\\r";
    let message = Message::parse(wire)?;
    let field = message.value(&[2, 1]);
    let span = field.span();
    let decoded = field.decimal()?;
    assert_eq!(
        decoded.numeric_cmp(&Decimal::parse(b"1.23")?),
        Ordering::Equal
    );
    assert!(decoded.fits_precision_scale(3, 2));
    assert_eq!(decoded.as_bytes(), b"+001.2300");
    assert_eq!(decoded.scale(), 4);
    assert_eq!(field.span(), span);
    assert_eq!(message.original(), wire);
    Ok(())
}

#[test]
fn decimal_queries_agree_with_an_independent_bounded_integer_oracle_and_handle_long_inputs()
-> Result {
    // The oracle uses scaled i128 arithmetic, independent of decimal string comparison.
    let cases: Vec<_> = (0_i128..128)
        .map(|i| {
            let coefficient = (i * 7919) % 2_000_000 - 1_000_000;
            let scale = (i % 7) as u32;
            let factor = 10_i128.pow(scale);
            let magnitude = coefficient.abs();
            let sign = if coefficient < 0 { "-" } else { "+" };
            let text = format!(
                "{sign}000{}.{}00",
                magnitude / factor,
                format_args!("{:0width$}", magnitude % factor, width = scale as usize)
            );
            (text, coefficient * 10_i128.pow(6 - scale))
        })
        .collect();
    let values: Vec<_> = cases
        .iter()
        .map(|(s, _)| Decimal::parse(s.as_bytes()))
        .collect::<std::result::Result<_, _>>()?;
    for (i, value) in values.iter().enumerate() {
        for (j, other) in values.iter().enumerate() {
            assert_eq!(value.numeric_cmp(other), cases[i].1.cmp(&cases[j].1));
        }
        for precision in 0..=8_u32 {
            for scale in 0..=8_u32 {
                let expected = precision > 0
                    && scale <= precision
                    && cases[i].1.abs() < 10_i128.pow(precision.saturating_sub(scale)) * 1_000_000
                    && (scale >= 6 || cases[i].1 % 10_i128.pow(6 - scale) == 0);
                assert_eq!(
                    value.fits_precision_scale(precision as usize, scale as usize),
                    expected
                );
            }
        }
    }
    let whole = format!("{}1{}.000", "0".repeat(50_000), "0".repeat(49_999));
    let smaller = "9".repeat(49_999);
    let large = Decimal::parse(whole.as_bytes())?;
    assert_eq!(
        large.numeric_cmp(&Decimal::parse(smaller.as_bytes())?),
        Ordering::Greater
    );
    assert!(large.fits_precision_scale(50_000, 0));
    assert!(!large.fits_precision_scale(49_999, 0));
    let fraction = format!("0.{}1{}", "0".repeat(50_000), "0".repeat(50_000));
    let tiny = Decimal::parse(fraction.as_bytes())?;
    assert!(!tiny.is_zero());
    assert!(!tiny.fits_precision_scale(50_000, 50_000));
    assert!(tiny.fits_precision_scale(50_001, 50_001));
    assert_eq!(tiny.scale(), 100_001);
    assert_eq!(tiny.as_bytes().as_ptr(), fraction.as_ptr());
    Ok(())
}
