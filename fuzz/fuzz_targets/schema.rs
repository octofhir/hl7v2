#![no_main]
use libfuzzer_sys::fuzz_target;
use octofhir_hl7v2::{
    schema::{
        Binding, Catalog, CompileLimits, MatchLimits, Occurrence, Occurs, Rule, Structure, View,
    },
    wire::{Message, ParseOptions},
};

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
const NAMES: [&str; 16] = [
    "r0", "r1", "r2", "r3", "r4", "r5", "r6", "r7", "r8", "r9", "r10", "r11", "r12", "r13", "r14",
    "r15",
];
const IDS: [[u8; 3]; 5] = [*b"PID", *b"OBR", *b"OBX", *b"NTE", *b"ZZZ"];
const HEADER: &[u8] = b"MSH|^~\\&|S|S|T|T|20261111||ZZZ^Z01^SYNTHETIC|m1|T|TEST\r";

fn visit(
    node: Occurrence<'_, '_, '_>,
    parent: std::ops::Range<usize>,
    leaves: &mut Vec<usize>,
    depth: usize,
) {
    assert!(depth <= 8);
    let range = node.segment_range();
    assert!(parent.start <= range.start && range.start < range.end && range.end <= parent.end);
    assert_eq!(node.segments().count(), range.len());
    if let Some(segment) = node.segment() {
        assert_eq!(range, segment.occurrence()..segment.occurrence() + 1);
        leaves.push(segment.occurrence());
    }
    for child in node.children() {
        visit(child, range.clone(), leaves, depth + 1);
    }
}
fn check(
    result: Result<View<'_, '_>, octofhir_hl7v2::schema::Error>,
    message: &Message,
    limits: MatchLimits,
) {
    match result {
        Ok(view) => {
            assert_eq!(
                view.message().original().as_ptr(),
                message.original().as_ptr()
            );
            assert!(view.occurrence_count() <= limits.max_occurrences);
            let range = 1..message.segments().len() + 1;
            assert_eq!(view.root().segment_range(), range);
            let mut leaves = Vec::new();
            visit(view.root(), range.clone(), &mut leaves, 0);
            assert_eq!(leaves, range.collect::<Vec<_>>());
        }
        Err(error) => assert!(
            error.span.start <= error.span.end && error.span.end <= message.original().len()
        ),
    }
}

fuzz_target!(|bytes: &[u8]| {
    if bytes.len() > 16_384 {
        return;
    }
    let options = ParseOptions {
        max_bytes: 16_384,
        max_segments: 512,
        max_nodes: 8192,
        max_diagnostics: 8,
        ..Default::default()
    };
    let limits = MatchLimits {
        max_occurrences: 1024,
        max_steps: 4096,
    };
    let compile = CompileLimits {
        max_rules: 64,
        max_depth: 8,
        max_work: 4096,
    };
    let binding = [Binding {
        message: "ZZZ",
        event: "Z01",
        structure: "SYNTHETIC",
    }];
    let definitions = [Structure {
        name: "SYNTHETIC",
        rules: ROOT,
    }];
    let catalog = Catalog::compile("TEST", &definitions, &binding, compile);
    assert!(catalog.is_ok());
    if let Ok(catalog) = catalog {
        let mut source = HEADER.to_vec();
        for parameters in bytes.as_chunks::<5>().0.iter().take(8) {
            source.extend_from_slice(b"PID|a~~b|\"\"\r");
            for _ in 0..parameters[0] % 3 {
                source.extend_from_slice(b"OBR|order\r");
                for _ in 0..parameters[1] % 4 {
                    source.extend_from_slice(b"OBX|observation\r");
                    for _ in 0..parameters[2] % 3 {
                        source.extend_from_slice(b"NTE|note\r");
                    }
                }
            }
        }
        let message = Message::parse_with_options(source, options);
        assert!(message.is_ok());
        if let Ok(message) = message {
            let result = catalog.group(&message, limits);
            assert!(result.is_ok());
            check(result, &message, limits);
            let bounded = MatchLimits {
                max_occurrences: usize::from(bytes.first().copied().unwrap_or(0)),
                max_steps: bytes.len() % 128,
            };
            check(catalog.group(&message, bounded), &message, bounded);
            if let Ok(raw) = Message::parse_with_options(bytes, options) {
                check(catalog.group(&raw, limits), &raw, limits);
                if let Ok(schema) = catalog.resolve(&message) {
                    check(schema.group(&raw, limits), &raw, limits);
                }
            }
        }
    }
    // Arbitrary flat cardinalities/IDs exercise rejection and exact-count repeated roles.
    let mut rules = vec![Rule::segment("msh", *b"MSH", Occurs::ONCE)];
    let mut source = HEADER.to_vec();
    for (i, &byte) in bytes.iter().take(16).enumerate() {
        let occurs = Occurs {
            min: usize::from((byte >> 3) % 3),
            max: (byte & 1 == 1).then_some(usize::from((byte >> 5) % 4)),
        };
        let id = IDS[usize::from(byte) % IDS.len()];
        rules.push(Rule::segment(NAMES[i], id, occurs));
        for _ in 0..usize::from((byte >> 3) % 3) {
            source.extend_from_slice(&id);
            source.push(b'\r');
        }
    }
    let definitions = [Structure {
        name: "SYNTHETIC",
        rules: &rules,
    }];
    if let Ok(catalog) = Catalog::compile("TEST", &definitions, &binding, compile)
        && let Ok(message) = Message::parse_with_options(source, options)
    {
        let result = catalog.group(&message, limits);
        assert!(result.is_ok(), "minimum-cardinality instance did not match");
        check(result, &message, limits);
    }
});
