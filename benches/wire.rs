//! Small dependency-free baseline. Output is local timing, not a throughput guarantee.
use octofhir_hl7v2::wire::{DateTime, Decimal, Message};
use std::{hint::black_box, time::Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let iterations = std::env::var("HL7V2_BENCH_ITERS")
        .ok()
        .and_then(|text| text.parse::<u32>().ok())
        .unwrap_or(10_000)
        .max(1);
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
    let number = b"+00012345678901234567890.000123456789012345678900";
    let timestamp = b"20240229235959.0100-0000";
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
        black_box(DateTime::parse(black_box(timestamp))?);
    }
    println!(
        "DTM: {:.0} ns/lexical parse",
        start.elapsed().as_nanos() as f64 / f64::from(iterations)
    );
    Ok(())
}
