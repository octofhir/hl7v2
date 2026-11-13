#![no_main]
use libfuzzer_sys::fuzz_target;
use octofhir_hl7v2::{
    schema::{
        Binding, CompileLimits, MatchLimits, Occurs, Rule, Structure,
        fields::{
            Condition, Datatype, DatatypeKind as K, Element as E, FieldSchema, Length, Predicate,
            Primitive as P, Report, Requirement, SegmentDefinition as S, ValidationLimits,
        },
        package::{Definitions, Manifest, Package, Source},
    },
    wire::{Edit, Message, ParseOptions, Presence},
};
const CODE_VALUES: &[&str] = &["F", "P", "C", "é", "A|B", "\"\""];
const TYPES: &[Datatype<'_>] = &[
    Datatype {
        name: "STATUS",
        kind: K::Codes(CODE_VALUES),
    },
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
const CONDITIONS: &[Condition<'_>] = &[
    Condition {
        profile: "Conditional",
        target: 2,
        when: Predicate::Equals {
            field: 1,
            value: "F",
        },
        requirement: Requirement::Required,
    },
    Condition {
        profile: "Conditional",
        target: 2,
        when: Predicate::Equals {
            field: 1,
            value: "P",
        },
        requirement: Requirement::Forbidden,
    },
];
const SEGMENTS: &[S<'_>] = &[
    S {
        name: "Conditional",
        id: *b"Z98",
        fields: &[
            E::required(1, "STATUS"),
            E {
                nullable: true,
                ..E::optional(2, "B")
            },
        ],
    },
    S {
        name: "Status",
        id: *b"Z99",
        fields: &[E {
            nullable: true,
            ..E::required(1, "STATUS")
        }],
    },
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
    let sources = [Source {
        name: "synthetic",
        location: "urn:octofhir:fuzz",
        revision: "1",
        sha256: [42; 32],
        license: "MIT OR Apache-2.0",
        notice: "Declared synthetic evidence",
    }];
    let manifest = Manifest {
        name: "fuzz",
        release: "TEST",
        revision: "1",
        producer: "validation-fuzzer@1",
        scope: "Synthetic grouping, fields and dynamic lexical types",
        limitations: "No standard catalog or external terminology",
        sources: &sources,
    };
    let definitions = || Definitions {
        structures: STRUCTURES,
        bindings: BINDINGS,
        datatypes: TYPES,
        segments: SEGMENTS,
    };
    let package = Package::compile_with_conditions(manifest, definitions(), CONDITIONS, compile);
    assert!(package.is_ok());
    let Ok(package) = package else {
        return;
    };
    let digest = package.digest();
    let reconstructed =
        Package::compile_with_conditions(manifest, definitions(), CONDITIONS, compile);
    assert!(reconstructed.is_ok());
    if let Ok(reconstructed) = reconstructed {
        assert!(reconstructed.require_digest(digest).is_ok());
    }
    // Metadata is interpreted only as manifest text, never as a path to read or fetch.
    if let Ok(text) = std::str::from_utf8(bytes) {
        let changed = Package::compile_with_conditions(
            Manifest {
                scope: text,
                ..manifest
            },
            definitions(),
            CONDITIONS,
            compile,
        );
        if let Ok(changed) = changed {
            assert_eq!(changed.digest() == digest, text == manifest.scope);
            assert_eq!(
                changed.require_digest(digest).is_ok(),
                text == manifest.scope
            );
        }
    }
    let schema = package.fields();
    let catalog = package.catalog();
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
    let status_source = Message::parse([HEADER, b"Z99|F\r"].concat());
    assert!(status_source.is_ok());
    if let Ok(status_source) = status_source {
        let payload = &bytes[..bytes.len().min(512)];
        let (edit, valid) = match parameter >> 4 & 3 {
            0 => (
                Edit::Value(payload.to_vec()),
                CODE_VALUES.iter().any(|s| s.as_bytes() == payload),
            ),
            1 => (Edit::Empty, false),
            2 => (Edit::Null, true),
            _ => (
                Edit::Value(
                    CODE_VALUES[usize::from(parameter) % CODE_VALUES.len()]
                        .as_bytes()
                        .to_vec(),
                ),
                true,
            ),
        };
        let mut editor = status_source.edit();
        assert!(editor.set(&[2, 1], edit).is_ok());
        let edited = editor.finish();
        assert!(edited.is_ok());
        if let Ok(edited) = edited {
            let report = schema.validate_segment(&edited, "Status", 2, limits);
            check(&report, &edited, limits);
            assert_eq!(report.is_valid(), valid);
            let bounded = ValidationLimits {
                max_steps: usize::from(parameter),
                max_bytes: bytes.len() % 128,
                max_issues: bytes.len() % 4,
            };
            check(
                &schema.validate_segment(&edited, "Status", 2, bounded),
                &edited,
                bounded,
            );
        }
    }
    let source = Message::parse([HEADER, b"Z98|F\r"].concat());
    assert!(source.is_ok());
    if let Ok(source) = source {
        let code = if parameter & 1 == 0 { "F" } else { "P" };
        let target = match parameter >> 1 & 3 {
            0 => Edit::Empty,
            1 => Edit::Null,
            _ => Edit::Value(bytes[..bytes.len().min(512)].to_vec()),
        };
        let mut edit = source.edit();
        assert!(
            edit.set(&[2, 1], Edit::Value(code.as_bytes().to_vec()))
                .is_ok()
        );
        assert!(edit.set_or_create(&[2, 2], target).is_ok());
        let edited = edit.finish();
        assert!(edited.is_ok());
        if let Ok(edited) = edited {
            let populated = matches!(
                edited.value(&[2, 2]).presence(),
                Presence::Null | Presence::Value
            );
            let report = schema.validate_segment(&edited, "Conditional", 2, limits);
            check(&report, &edited, limits);
            assert_eq!(report.is_valid(), populated == (code == "F"));
            let bounded = ValidationLimits {
                max_steps: usize::from(parameter),
                max_bytes: bytes.len() % 128,
                max_issues: bytes.len() % 4,
            };
            check(
                &schema.validate_segment(&edited, "Conditional", 2, bounded),
                &edited,
                bounded,
            );
        }
    }
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
