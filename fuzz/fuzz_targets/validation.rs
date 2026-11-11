#![no_main]
use libfuzzer_sys::fuzz_target;
use octofhir_hl7v2::{
    schema::{
        Binding, Catalog, CompileLimits, MatchLimits, Occurs, Rule, Structure,
        fields::{
            Datatype, DatatypeKind as K, Element as E, FieldSchema, Length, Primitive as P, Report,
            SegmentDefinition as S, ValidationLimits,
        },
    },
    wire::{Edit, Message, ParseOptions, Presence},
};
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
        name: "DT",
        kind: K::Primitive(P::Date),
    },
    Datatype {
        name: "DTM",
        kind: K::Primitive(P::DateTime),
    },
    Datatype {
        name: "TS",
        kind: K::Timestamp,
    },
    Datatype {
        name: "HD",
        kind: K::Composite(&[
            E::required(1, "ST"),
            E::optional(2, "ST"),
            E::optional(3, "ID"),
        ]),
    },
    Datatype {
        name: "CX",
        kind: K::Composite(&[E::required(1, "ST"), E::optional(4, "HD")]),
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
const ALLOWED: &[&str] = &["NM", "DT", "DTM", "TS", "ST", "CX"];
const OBX: &[E<'_>] = &[
    E::optional(1, "ID"),
    E::optional(2, "ID"),
    E::optional(3, "B"),
    E::optional(4, "ID"),
    E {
        nullable: true,
        ..E::by_field(5, 2, ALLOWED, Occurs::MANY)
    },
];
const SEGMENTS: &[S<'_>] = &[
    S {
        name: "Header",
        id: *b"MSH",
        fields: &[
            E::required(1, "B"),
            E::required(2, "B"),
            E::required(9, "MSG"),
            E::required(12, "ID"),
        ],
    },
    S {
        name: "Observation",
        id: *b"OBX",
        fields: OBX,
    },
];
const RULES: &[Rule<'_>] = &[
    Rule::typed_segment("msh", *b"MSH", "Header", Occurs::ONCE),
    Rule::typed_segment("obx", *b"OBX", "Observation", Occurs::MANY),
];
const STRUCTURES: &[Structure<'_>] = &[Structure {
    name: "TEST",
    rules: RULES,
}];
const BINDINGS: &[Binding<'_>] = &[Binding {
    message: "ZZZ",
    event: "Z01",
    structure: "TEST",
}];
const HEADER: &[u8] = b"MSH|^~\\&|||||||ZZZ^Z01^TEST|||TEST\r";
fn check(report: &Report, message: &Message, limits: ValidationLimits) {
    assert!(report.issues.len() <= limits.max_issues);
    assert!(report.steps <= limits.max_steps);
    assert!(report.bytes <= limits.max_bytes);
    if !report.complete || report.omitted > 0 {
        assert!(!report.is_valid());
    }
    for issue in &report.issues {
        assert!(issue.span.start <= issue.span.end && issue.span.end <= message.original().len());
        let length = issue
            .path
            .iter()
            .rposition(|&n| n != 0)
            .map_or(0, |n| n + 1);
        if length >= 2 {
            assert_eq!(
                issue.presence,
                message.value(&issue.path[..length]).presence()
            );
        }
    }
}
fuzz_target!(|bytes: &[u8]| {
    if bytes.len() > 16_384 {
        return;
    }
    let compile = CompileLimits {
        max_rules: 128,
        max_depth: 8,
        max_work: 4096,
    };
    let schema = FieldSchema::compile(TYPES, SEGMENTS, compile);
    let catalog = Catalog::compile("TEST", STRUCTURES, BINDINGS, compile);
    assert!(schema.is_ok() && catalog.is_ok());
    let (Ok(schema), Ok(catalog)) = (schema, catalog) else {
        return;
    };
    assert!(schema.check_catalog(&catalog).is_ok());
    let cases = [
        ("NM", "+0001.2300"),
        ("DT", "202402"),
        ("DTM", "20240229235959.0100-0000"),
        ("TS", "202402291234^L"),
        ("ST", "text"),
        ("CX", "id^^^AUTH&urn&ISO"),
        ("UNKNOWN", "text"),
        ("", "text"),
    ];
    let parameter = bytes.first().copied().unwrap_or(0);
    let (code, value) = cases[usize::from(parameter) % cases.len()];
    let options = ParseOptions {
        max_bytes: 16_384,
        max_segments: 512,
        max_nodes: 8192,
        max_diagnostics: 8,
        ..Default::default()
    };
    let source = [HEADER, format!("OBX|1|{code}|c||{value}\r").as_bytes()].concat();
    let base = Message::parse_with_options(source, options);
    assert!(base.is_ok());
    let Ok(base) = base else {
        return;
    };
    let limits = ValidationLimits {
        max_steps: 4096,
        max_bytes: 65_536,
        max_issues: 16,
    };
    if usize::from(parameter) % cases.len() < 6 {
        assert!(
            schema
                .validate_segment(&base, "Observation", 2, limits)
                .is_valid()
        );
    }
    let mut editor = base.edit();
    let edit = match parameter >> 3 & 3 {
        0 => Edit::Value(bytes[..bytes.len().min(512)].to_vec()),
        1 => Edit::Empty,
        2 => Edit::Null,
        _ => Edit::Value(b"\\\"\"|^&~\r\xff".to_vec()),
    };
    let path: &[usize] = if parameter >> 3 & 3 == 0 && code == "CX" {
        &[2, 5, 1, 4, 2]
    } else if parameter >> 3 & 3 == 0 && code == "TS" {
        &[2, 5, 1, 1]
    } else {
        &[2, 5]
    };
    assert!(editor.set(path, edit).is_ok());
    let message = editor.finish();
    assert!(message.is_ok());
    let Ok(message) = message else {
        return;
    };
    let view = catalog.group(
        &message,
        MatchLimits {
            max_occurrences: 1024,
            max_steps: 4096,
        },
    );
    assert!(view.is_ok());
    if let Ok(view) = view {
        let report = schema.validate(&view, limits);
        check(&report, &message, limits);
        if report.is_valid() && message.value(&[2, 5]).presence() == Presence::Value {
            assert_eq!(
                schema.datatype(&message, "Observation", &[2, 5]).ok(),
                Some(code)
            );
            match code {
                "NM" => assert!(message.value(&[2, 5]).decimal().is_ok()),
                "DT" => assert!(message.value(&[2, 5]).date().is_ok()),
                "DTM" => assert!(message.value(&[2, 5]).date_time().is_ok()),
                "TS" => assert!(message.timestamp(&[2, 5]).is_ok()),
                _ => {}
            }
        }
        let bounded = ValidationLimits {
            max_steps: usize::from(parameter),
            max_bytes: bytes.len() % 128,
            max_issues: bytes.len() % 4,
        };
        check(&schema.validate(&view, bounded), &message, bounded);
    }
    let fields = [
        E::optional(1, "ID"),
        E::optional(2, "ID"),
        E::optional(3, "B"),
        E::optional(4, "ID"),
        E {
            occurs: Occurs {
                min: usize::from(parameter % 3),
                max: (parameter & 1 == 1).then_some(usize::from((parameter >> 2) % 4)),
            },
            nullable: parameter & 4 == 0,
            length: Some(match parameter % 3 {
                0 => Length::WireBytes(bytes.len() % 32),
                1 => Length::ScalarBytes(bytes.len() % 32),
                _ => Length::Utf8Characters(bytes.len() % 32),
            }),
            ..OBX[4]
        },
    ];
    let definitions = [S {
        name: "Observation",
        id: *b"OBX",
        fields: &fields,
    }];
    if let Ok(schema) = FieldSchema::compile(TYPES, &definitions, compile) {
        check(
            &schema.validate_segment(&message, "Observation", 2, limits),
            &message,
            limits,
        );
    }
    if let Ok(raw) = Message::parse_with_options(bytes, options) {
        for segment in raw.segments().take(8).filter(|s| s.id() == b"OBX") {
            check(
                &schema.validate_segment(&raw, "Observation", segment.occurrence(), limits),
                &raw,
                limits,
            );
        }
    }
});
