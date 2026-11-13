//! Small dependency-free baseline. Output is local timing, not a throughput guarantee.
use octofhir_hl7v2::wire::{DateTime, Decimal, Edit, Editor, Error, Message, Presence};
use std::{hint::black_box, time::Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let iterations = std::env::var("HL7V2_BENCH_ITERS")
        .ok()
        .and_then(|text| text.parse::<u32>().ok())
        .unwrap_or(10_000)
        .max(1);
    let source = Message::parse(include_bytes!("../tests/fixtures/adt.er7"))?;
    let editor = patches(&source)?;
    let output = editor.serialize()?;
    let revised = Message::parse(&output)?;
    assert_eq!(revised.value(&[5, 1]).decode()?.as_ref(), b"changed|value");
    assert_eq!(
        revised.value(&[5, 4, 2, 2, 2]).decode()?.as_ref(),
        b"created"
    );
    assert_eq!(revised.value(&[5, 6]).presence(), Presence::Null);
    assert_eq!(revised.value(&[5, 3]).presence(), Presence::Empty);
    for (old, new) in source.segments().take(4).zip(revised.segments()) {
        assert_eq!(old.raw(), new.raw());
    }
    let structural = segments(&source)?.finish()?;
    assert_eq!(structural.segments().len(), source.segments().len());
    assert_eq!(structural.segment(2).map(|s| *s.id()), Some(*b"Z99"));
    assert_eq!(structural.value(&[2, 1]).presence(), Presence::Null);
    assert_eq!(
        structural.value(&[2, 3, 2, 2, 2]).decode()?.as_ref(),
        b"generated|^~\\&"
    );
    for old in source.segments().take(source.segments().len() - 1) {
        let new = old.occurrence() + usize::from(old.occurrence() >= 2);
        assert_eq!(structural.segment(new).map(|s| s.raw()), Some(old.raw()));
    }
    let number = b"+00012345678901234567890.000123456789012345678900";
    let timestamp = b"20240229235959.0100-0000";
    let numeric = Decimal::parse(number)?;
    let equivalent = Decimal::parse(b"12345678901234567890.0001234567890123456789")?;
    assert_eq!(numeric.numeric_cmp(&equivalent), std::cmp::Ordering::Equal);
    assert!(numeric.fits_precision_scale(48, 28));
    assert!(!numeric.fits_precision_scale(47, 28));
    assert_eq!(Decimal::parse(number)?.as_bytes(), number);
    assert_eq!(DateTime::parse(timestamp)?.as_bytes(), timestamp);
    for (name, input) in [
        (
            "adt",
            include_bytes!("../tests/fixtures/adt.er7").as_slice(),
        ),
        (
            "oru",
            include_bytes!("../tests/fixtures/oru.er7").as_slice(),
        ),
    ] {
        let start = Instant::now();
        for _ in 0..iterations {
            black_box(Message::parse(black_box(input))?);
        }
        let elapsed = start.elapsed();
        println!(
            "{name}: {iterations} parses, {} bytes/message, {:.0} ns/message, {:.2} MiB/s",
            input.len(),
            elapsed.as_nanos() as f64 / f64::from(iterations),
            input.len() as f64 * f64::from(iterations) / elapsed.as_secs_f64() / 1_048_576.0
        );
    }
    let start = Instant::now();
    for _ in 0..iterations {
        black_box(Decimal::parse(black_box(number))?);
    }
    println!(
        "decimal: {:.0} ns/lexical parse",
        start.elapsed().as_nanos() as f64 / f64::from(iterations)
    );
    let start = Instant::now();
    for _ in 0..iterations {
        black_box(numeric.numeric_cmp(black_box(&equivalent)));
    }
    println!(
        "decimal numeric compare: {:.0} ns/query (parsed views)",
        start.elapsed().as_nanos() as f64 / f64::from(iterations)
    );
    let start = Instant::now();
    for _ in 0..iterations {
        black_box(numeric.fits_precision_scale(black_box(48), black_box(28)));
    }
    println!(
        "decimal precision/scale: {:.0} ns/query (parsed view)",
        start.elapsed().as_nanos() as f64 / f64::from(iterations)
    );
    let start = Instant::now();
    for _ in 0..iterations {
        black_box(DateTime::parse(black_box(timestamp))?);
    }
    println!(
        "DTM: {:.0} ns/lexical parse",
        start.elapsed().as_nanos() as f64 / f64::from(iterations)
    );
    let start = Instant::now();
    for _ in 0..iterations {
        black_box(editor.serialize()?);
    }
    println!(
        "sparse serialize: {} -> {} bytes, {:.0} ns/message (prepared patches, no reindex)",
        source.original().len(),
        output.len(),
        start.elapsed().as_nanos() as f64 / f64::from(iterations)
    );
    let start = Instant::now();
    for _ in 0..iterations {
        black_box(patches(&source)?.finish()?);
    }
    println!(
        "sparse prepare+serialize+reindex: {:.0} ns/message (source already parsed)",
        start.elapsed().as_nanos() as f64 / f64::from(iterations)
    );
    let start = Instant::now();
    for _ in 0..iterations {
        black_box(segments(&source)?.finish()?);
    }
    println!(
        "segment draft+insert+remove+serialize+reindex: {:.0} ns/message (source already parsed)",
        start.elapsed().as_nanos() as f64 / f64::from(iterations)
    );
    Ok(())
}

fn segments(message: &Message) -> Result<Editor<'_>, Error> {
    let mut segment = message.segment_builder(*b"Z99")?;
    segment.set(&[1], Edit::Null)?;
    segment.set(&[3, 2, 2, 2], Edit::Value(b"generated|^~\\&".to_vec()))?;
    let mut editor = message.edit();
    editor
        .insert_segment(2, segment)?
        .remove_segment(message.segments().len())?;
    Ok(editor)
}

fn patches(message: &Message) -> Result<Editor<'_>, Error> {
    let mut editor = message.edit();
    editor.set(&[5, 1], Edit::Value(b"changed|value".to_vec()))?;
    editor.set_or_create(&[5, 4, 2, 2, 2], Edit::Value(b"created".to_vec()))?;
    editor.set_or_create(&[5, 6], Edit::Null)?;
    Ok(editor)
}
