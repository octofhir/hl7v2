use octofhir_hl7v2::wire::{Date, DateTime, Decimal, ErrorKind, Message, Precision, Presence};
type Result = std::result::Result<(), Box<dyn std::error::Error>>;

#[test]
fn decimals_preserve_sign_scale_and_arbitrary_precision() -> Result {
    for (bytes, sign, integer, fraction) in [
        (
            b"+00012.3400".as_slice(),
            Some(b'+'),
            b"00012".as_slice(),
            Some(b"3400".as_slice()),
        ),
        (b"-0.000", Some(b'-'), b"0", Some(b"000".as_slice())),
        (b"001", None, b"001", None),
        (b".5", None, b"", Some(b"5".as_slice())),
        (b"5.", None, b"5", Some(b"".as_slice())),
    ] {
        let value = Decimal::parse(bytes)?;
        assert_eq!(value.as_bytes(), bytes);
        assert_eq!(value.as_bytes().as_ptr(), bytes.as_ptr());
        assert_eq!(value.sign(), sign);
        assert_eq!(value.integer_digits(), integer);
        assert_eq!(value.fraction_digits(), fraction);
        assert_eq!(value.scale(), fraction.map_or(0, <[u8]>::len));
    }
    let long = format!("-{}.{}", "9".repeat(80_000), "0".repeat(20_000));
    let value = Decimal::parse(long.as_bytes())?;
    assert_eq!(value.integer_digits().len(), 80_000);
    assert_eq!(value.scale(), 20_000);
    assert_eq!(value.as_bytes().as_ptr(), long.as_ptr());
    for bytes in [
        b"".as_slice(),
        b"+",
        b"-.",
        b".",
        b"1.2.3",
        b"--1",
        b"1-",
        b"1e3",
        b"NaN",
        b"inf",
        b" 1",
        b"1\0",
        b"<12",
        b"1,2",
        "１２".as_bytes(),
    ] {
        let error = Decimal::parse(bytes)
            .err()
            .ok_or("invalid decimal accepted")?;
        assert_eq!(error.kind, ErrorKind::Number);
        assert_eq!(error.span, 0..bytes.len());
    }
    Ok(())
}

#[test]
fn temporal_views_keep_partial_components_fraction_and_offset_spelling() -> Result {
    let whole = b"20240229235958.0100-0000";
    let expected = [
        Precision::Year,
        Precision::Month,
        Precision::Day,
        Precision::Hour,
        Precision::Minute,
        Precision::Second,
        Precision::Fraction(1),
        Precision::Fraction(2),
        Precision::Fraction(3),
        Precision::Fraction(4),
    ];
    for (len, precision) in [4, 6, 8, 10, 12, 14, 16, 17, 18, 19]
        .into_iter()
        .zip(expected)
    {
        let bytes = &whole[..len];
        let value = DateTime::parse(bytes)?;
        assert_eq!(value.as_bytes().as_ptr(), bytes.as_ptr());
        assert_eq!(value.precision(), precision);
        assert_eq!(value.year(), 2024);
        assert_eq!(value.month(), (len >= 6).then_some(2));
        assert_eq!(value.day(), (len >= 8).then_some(29));
        assert_eq!(value.hour(), (len >= 10).then_some(23));
        assert_eq!(value.minute(), (len >= 12).then_some(59));
        assert_eq!(value.second(), (len >= 14).then_some(58));
        assert_eq!(value.fraction_digits(), (len > 14).then(|| &whole[15..len]));
        assert_eq!(value.offset_minutes(), None);
        if len <= 8 {
            let date = Date::parse(bytes)?;
            assert_eq!(date.precision(), precision);
            assert_eq!(
                (date.year(), date.month(), date.day()),
                (value.year(), value.month(), value.day())
            );
            assert_eq!(date.as_bytes().as_ptr(), bytes.as_ptr());
        } else {
            assert!(Date::parse(bytes).is_err());
        }
    }
    for (suffix, minutes) in [("+0300", 180), ("-0930", -570), ("+0000", 0), ("-0000", 0)] {
        for body in ["2024", "202402", "20240229", "20240229235958.0100"] {
            let bytes = format!("{body}{suffix}");
            let value = DateTime::parse(bytes.as_bytes())?;
            assert_eq!(value.as_bytes(), bytes.as_bytes());
            assert_eq!(value.offset_minutes(), Some(minutes));
            assert!(Date::parse(bytes.as_bytes()).is_err());
        }
    }
    assert_eq!(
        DateTime::parse(whole)?.fraction_digits(),
        Some(b"0100".as_slice())
    );
    assert_eq!(DateTime::parse(b"202402290000")?.hour(), Some(0));
    for date in [
        b"00010101",
        b"19000228",
        b"20000229",
        b"21000228",
        b"99991231",
    ] {
        assert_eq!(Date::parse(date)?.as_bytes(), date);
    }
    for bytes in [
        b"".as_slice(),
        b"0000",
        b"202",
        b"20240",
        b"202400",
        b"202413",
        b"20240200",
        b"19000229",
        b"21000229",
        b"20260431",
        b"20240230",
        b"2024022924",
        b"202402292360",
        b"20240229235960",
        b"20240229235959.",
        b"20240229235959.12345",
        b"20240229.1",
        b"20240229235959+2400",
        b"20240229235959+0060",
        b"20240229235959+030",
        b"2024+0x00",
        b"2024-0100x",
        b"2024+0000+0000",
        b"20240229235959.1.2",
        b"2024\xff",
        b"2024 ",
    ] {
        let error = DateTime::parse(bytes).err().ok_or("invalid DTM accepted")?;
        assert_eq!(error.kind, ErrorKind::DateTime);
        assert_eq!(error.span, 0..bytes.len());
    }
    Ok(())
}

#[test]
fn typed_reads_preserve_wire_presence_escapes_positions_and_invalid_source() -> Result {
    let input = b"MSH|^~\\&|S\rZDT|20240229|20260229|20240229235959.0100-0000\rZNM|+00012.3400||\"\"|\\X2222\\|1^2|\\X31\\.2.3|\\Zlocal\\\r";
    let message = Message::parse(input)?;
    assert_eq!(message.value(&[2, 1]).date()?.day(), Some(29));
    let invalid_date = message.value(&[2, 2]);
    let error = invalid_date.date().err().ok_or("invalid date accepted")?;
    assert_eq!(error.kind, ErrorKind::Date);
    assert_eq!(Some(error.span), invalid_date.span());
    assert_eq!(
        message.value(&[2, 3]).date_time()?.fraction_digits(),
        Some(b"0100".as_slice())
    );
    let numeric = message.value(&[3, 1]);
    assert_eq!(
        numeric.decimal()?.as_bytes().as_ptr(),
        numeric.raw().ok_or("missing raw")?.as_ptr()
    );
    for (field, kind) in [
        (2, ErrorKind::Empty),
        (3, ErrorKind::Null),
        (4, ErrorKind::Number),
        (5, ErrorKind::Composite),
        (6, ErrorKind::Number),
        (7, ErrorKind::Escape),
        (8, ErrorKind::Missing),
    ] {
        let value = message.value(&[3, field]);
        let error = value.decimal().err().ok_or("invalid scalar accepted")?;
        assert_eq!(error.kind, kind);
        if field != 7 {
            assert_eq!(error.span, value.span().unwrap_or(0..0));
        }
    }
    assert_eq!(message.value(&[3, 4]).presence(), Presence::Value);
    assert_eq!(message.value(&[3, 5, 1, 2]).decimal()?.as_bytes(), b"2");
    assert_eq!(message.original(), input);
    // The decimal point is a field separator here; typed reads own only decoded scalar bytes.
    let custom = Message::parse(b"MSH.$%!?..\rZNM.+00012!F!3400.20240229235959!F!0100-0000\r")?;
    assert_eq!(custom.value(&[2, 1]).decimal()?.as_bytes(), b"+00012.3400");
    assert_eq!(
        custom.value(&[2, 2]).date_time()?.fraction_digits(),
        Some(b"0100".as_slice())
    );
    Ok(())
}
