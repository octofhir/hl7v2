//! Synthetic grouping timings; no field, terminology, FHIR or durability validation.
use octofhir_hl7v2::{
    schema::{Binding, Catalog, CompileLimits, MatchLimits, Occurs, Rule, Structure},
    wire::Message,
};
use std::{hint::black_box, time::Instant};

const OBS: &[Rule<'_>] = &[
    Rule::segment("obx", *b"OBX", Occurs::ONCE),
    Rule::segment("nte", *b"NTE", Occurs::MANY),
];
const ORDER: &[Rule<'_>] = &[
    Rule::segment("obr", *b"OBR", Occurs::ONCE),
    Rule::group("observation", OBS, Occurs::MANY),
];
const PATIENT: &[Rule<'_>] = &[
    Rule::segment("pid", *b"PID", Occurs::ONCE),
    Rule::group("order", ORDER, Occurs::MANY),
];
const ROOT: &[Rule<'_>] = &[
    Rule::segment("msh", *b"MSH", Occurs::ONCE),
    Rule::group("patient", PATIENT, Occurs::MANY),
];
const STRUCTURES: &[Structure<'_>] = &[Structure {
    name: "SYNTHETIC",
    rules: ROOT,
}];
const BINDINGS: &[Binding<'_>] = &[Binding {
    message: "ZZZ",
    event: "Z01",
    structure: "SYNTHETIC",
}];
type Result = std::result::Result<(), Box<dyn std::error::Error>>;

fn main() -> Result {
    let iterations = std::env::var("HL7V2_BENCH_ITERS")
        .ok()
        .and_then(|text| text.parse::<u32>().ok())
        .unwrap_or(10_000)
        .max(1);
    let catalog = Catalog::compile("TEST", STRUCTURES, BINDINGS, CompileLimits::default())?;
    let start = Instant::now();
    for _ in 0..iterations {
        black_box(Catalog::compile(
            "TEST",
            black_box(STRUCTURES),
            BINDINGS,
            CompileLimits::default(),
        )?);
    }
    println!(
        "compile synthetic catalog: {:.0} ns/catalog",
        start.elapsed().as_nanos() as f64 / f64::from(iterations)
    );
    run(&catalog, 1, iterations)?;
    run(&catalog, 1000, (iterations / 1000).max(1))
}
fn run(catalog: &Catalog<'_>, patients: usize, iterations: u32) -> Result {
    let mut bytes = b"MSH|^~\\&|S|S|T|T|20261111||ZZZ^Z01^SYNTHETIC|id|T|TEST\r".to_vec();
    for _ in 0..patients {
        bytes.extend_from_slice(b"PID|synthetic-patient\rOBR|synthetic-order\r");
        for _ in 0..3 {
            bytes.extend_from_slice(b"OBX|1|NM|synthetic-code||0001.2300\rNTE|1||synthetic note\r");
        }
    }
    let message = Message::parse(&bytes)?;
    let view = catalog.group(&message, MatchLimits::default())?;
    assert_eq!(
        view.message().original().as_ptr(),
        message.original().as_ptr()
    );
    assert_eq!(view.root().segment_range(), 1..message.segments().len() + 1);
    assert_eq!(
        view.root()
            .children()
            .filter(|n| n.name() == "patient")
            .count(),
        patients
    );
    assert_eq!(view.occurrence_count(), 2 + 13 * patients);
    println!(
        "synthetic: {patients} patients, {} bytes, {} segments, {iterations} iterations",
        bytes.len(),
        message.segments().len()
    );
    let start = Instant::now();
    for _ in 0..iterations {
        black_box(catalog.resolve(black_box(&message))?);
    }
    println!(
        "  resolve cached source: {:.0} ns/message",
        start.elapsed().as_nanos() as f64 / f64::from(iterations)
    );
    let start = Instant::now();
    for _ in 0..iterations {
        black_box(catalog.group(black_box(&message), MatchLimits::default())?);
    }
    println!(
        "  resolve+group cached source: {:.0} ns/message",
        start.elapsed().as_nanos() as f64 / f64::from(iterations)
    );
    let start = Instant::now();
    for _ in 0..iterations {
        let source = Message::parse(black_box(&bytes))?;
        black_box(catalog.group(&source, MatchLimits::default())?);
    }
    println!(
        "  parse+resolve+group: {:.0} ns/message",
        start.elapsed().as_nanos() as f64 / f64::from(iterations)
    );
    Ok(())
}
