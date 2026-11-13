//! Separate synthetic grouping/field-check timings; no terminology, FHIR or durability validation.
use octofhir_hl7v2::{
    schema::{
        Binding, Catalog, CompileLimits, MatchLimits, Occurs, Rule, Structure,
        fields::{
            Condition, Datatype, DatatypeKind as K, Element as E, FieldSchema, Predicate,
            Primitive as P, Requirement, SegmentDefinition as S, ValidationLimits,
        },
    },
    wire::Message,
};
use std::{hint::black_box, time::Instant};

const OBS: &[Rule<'_>] = &[
    Rule::typed_segment("obx", *b"OBX", "Observation", Occurs::ONCE),
    Rule::typed_segment("nte", *b"NTE", "Note", Occurs::MANY),
];
const ORDER: &[Rule<'_>] = &[
    Rule::typed_segment("obr", *b"OBR", "Order", Occurs::ONCE),
    Rule::group("observation", OBS, Occurs::MANY),
];
const PATIENT: &[Rule<'_>] = &[
    Rule::typed_segment("pid", *b"PID", "Patient", Occurs::ONCE),
    Rule::group("order", ORDER, Occurs::MANY),
];
const ROOT: &[Rule<'_>] = &[
    Rule::typed_segment("msh", *b"MSH", "Header", Occurs::ONCE),
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
const TYPES: &[Datatype<'_>] = &[
    Datatype {
        name: "B",
        kind: K::Primitive(P::Bytes),
    },
    Datatype {
        name: "ID",
        kind: K::Primitive(P::Ascii),
    },
    Datatype {
        name: "ST",
        kind: K::Primitive(P::Utf8),
    },
    Datatype {
        name: "NM",
        kind: K::Primitive(P::Number),
    },
    Datatype {
        name: "MSG",
        kind: K::Composite(&[
            E::required(1, "ID"),
            E::required(2, "ID"),
            E::required(3, "ID"),
        ]),
    },
];
const PROFILES: &[S<'_>] = &[
    S {
        name: "Header",
        id: *b"MSH",
        fields: &[
            E::required(1, "B"),
            E::required(2, "B"),
            E::optional(3, "ST"),
            E::optional(4, "ST"),
            E::optional(5, "ST"),
            E::optional(6, "ST"),
            E::optional(7, "B"),
            E::optional(8, "B"),
            E::required(9, "MSG"),
            E::optional(10, "ID"),
            E::optional(11, "ID"),
            E::required(12, "ID"),
        ],
    },
    S {
        name: "Patient",
        id: *b"PID",
        fields: &[E::required(1, "ST")],
    },
    S {
        name: "Order",
        id: *b"OBR",
        fields: &[E::required(1, "ST")],
    },
    S {
        name: "Observation",
        id: *b"OBX",
        fields: &[
            E::optional(1, "ID"),
            E::required(2, "ID"),
            E::optional(3, "ST"),
            E::optional(4, "ID"),
            E::by_field(5, 2, &["NM"], Occurs::MANY),
        ],
    },
    S {
        name: "Note",
        id: *b"NTE",
        fields: &[E::optional(1, "ID"), E::optional(3, "ST")],
    },
];
type Result = std::result::Result<(), Box<dyn std::error::Error>>;

fn main() -> Result {
    let iterations = std::env::var("HL7V2_BENCH_ITERS")
        .ok()
        .and_then(|text| text.parse::<u32>().ok())
        .unwrap_or(10_000)
        .max(1);
    code_timing(iterations)?;
    condition_timing(iterations)?;
    let catalog = Catalog::compile("TEST", STRUCTURES, BINDINGS, CompileLimits::default())?;
    let fields = FieldSchema::compile(TYPES, PROFILES, CompileLimits::default())?;
    fields.check_catalog(&catalog)?;
    #[cfg(feature = "schema-package")]
    package_timing(iterations)?;
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
    run(&catalog, &fields, 1, iterations)?;
    run(&catalog, &fields, 1000, (iterations / 1000).max(1))
}
fn code_timing(iterations: u32) -> Result {
    let codes: Vec<_> = (0..256).map(|n| format!("CODE{n:03}")).collect();
    let refs: Vec<_> = codes.iter().map(String::as_str).collect();
    let types = [Datatype {
        name: "CODE",
        kind: K::Codes(&refs),
    }];
    let fields = [E::required(1, "CODE")];
    let profiles = [S {
        name: "Value",
        id: *b"ZZZ",
        fields: &fields,
    }];
    let schema = FieldSchema::compile(&types, &profiles, CompileLimits::default())?;
    let message = Message::parse(b"MSH|^~\\&\rZZZ|CODE127\r")?;
    assert!(
        schema
            .validate_segment(&message, "Value", 2, ValidationLimits::default())
            .is_valid()
    );
    let start = Instant::now();
    for _ in 0..iterations {
        black_box(schema.validate_segment(
            black_box(&message),
            "Value",
            2,
            ValidationLimits::default(),
        ));
    }
    println!(
        "closed code-set checks (256 codes): {:.0} ns/message (cached indexed source, no grouping)",
        start.elapsed().as_nanos() as f64 / f64::from(iterations)
    );
    Ok(())
}
fn condition_timing(iterations: u32) -> Result {
    let types = [Datatype {
        name: "ID",
        kind: K::Primitive(P::Ascii),
    }];
    let fields = [E::required(1, "ID"), E::optional(2, "ID")];
    let profiles = [S {
        name: "Local",
        id: *b"Z99",
        fields: &fields,
    }];
    let rules = [Condition {
        profile: "Local",
        target: 2,
        when: Predicate::Equals {
            field: 1,
            value: "F",
        },
        requirement: Requirement::Required,
    }];
    let schema =
        FieldSchema::compile_with_conditions(&types, &profiles, &rules, CompileLimits::default())?;
    let message = Message::parse(b"MSH|^~\\&\rZ99|F|present\r")?;
    assert!(
        schema
            .validate_segment(&message, "Local", 2, ValidationLimits::default())
            .is_valid()
    );
    let start = Instant::now();
    for _ in 0..iterations {
        black_box(schema.validate_segment(
            black_box(&message),
            "Local",
            2,
            ValidationLimits::default(),
        ));
    }
    println!(
        "conditional profile checks: {:.0} ns/message (cached indexed source, no grouping)",
        start.elapsed().as_nanos() as f64 / f64::from(iterations)
    );
    Ok(())
}
#[cfg(feature = "schema-package")]
fn package_timing(iterations: u32) -> Result {
    use octofhir_hl7v2::schema::package::{Definitions, Manifest, Package, Source};
    let sources = [Source {
        name: "synthetic",
        location: "urn:octofhir:benchmark",
        revision: "1",
        sha256: [0; 32],
        license: "MIT OR Apache-2.0",
        notice: "Declared synthetic benchmark evidence",
    }];
    let manifest = Manifest {
        name: "synthetic",
        release: "TEST",
        revision: "1",
        producer: "schema-benchmark@1",
        scope: "Synthetic structure, event, field and lexical constraints",
        limitations: "No standard catalog, conditional rules or terminology",
        sources: &sources,
    };
    let start = Instant::now();
    for _ in 0..iterations {
        black_box(Package::compile(
            manifest,
            Definitions {
                structures: STRUCTURES,
                bindings: BINDINGS,
                datatypes: TYPES,
                segments: PROFILES,
            },
            CompileLimits::default(),
        )?);
    }
    println!(
        "compile+fingerprint synthetic package: {:.0} ns/package",
        start.elapsed().as_nanos() as f64 / f64::from(iterations)
    );
    Ok(())
}
fn run(
    catalog: &Catalog<'_>,
    fields: &FieldSchema<'_>,
    patients: usize,
    iterations: u32,
) -> Result {
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
    assert!(
        fields
            .validate(&view, ValidationLimits::default())
            .is_valid()
    );
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
    let start = Instant::now();
    for _ in 0..iterations {
        black_box(fields.validate(&view, ValidationLimits::default()));
    }
    println!(
        "  field checks cached view: {:.0} ns/message",
        start.elapsed().as_nanos() as f64 / f64::from(iterations)
    );
    let start = Instant::now();
    for _ in 0..iterations {
        let source = Message::parse(black_box(&bytes))?;
        let view = catalog.group(&source, MatchLimits::default())?;
        black_box(fields.validate(&view, ValidationLimits::default()));
    }
    println!(
        "  parse+resolve+group+field checks: {:.0} ns/message",
        start.elapsed().as_nanos() as f64 / f64::from(iterations)
    );
    Ok(())
}
