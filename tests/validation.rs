use octofhir_hl7v2::{
    schema::{
        Binding, Catalog, CompileLimits, MatchLimits, Occurs, Rule, Structure,
        fields::{
            Datatype, DatatypeKind as K, Element as E, FieldSchema, IssueKind, Length,
            Primitive as P, SegmentDefinition as S, ValidationLimits,
        },
    },
    wire::{ErrorKind as WireError, Message, Presence},
};
type Result = std::result::Result<(), Box<dyn std::error::Error>>;
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
    S {
        name: "Numeric",
        id: *b"Z99",
        fields: &[E::required(1, "NM")],
    },
    S {
        name: "Calendar",
        id: *b"Z99",
        fields: &[E::required(1, "DT")],
    },
];
const RULES: &[Rule<'_>] = &[
    Rule::typed_segment("msh", *b"MSH", "Header", Occurs::ONCE),
    Rule::typed_segment("observation", *b"OBX", "Observation", Occurs::MANY),
    Rule::typed_segment("number", *b"Z99", "Numeric", Occurs::ONCE),
    Rule::typed_segment("date", *b"Z99", "Calendar", Occurs::ONCE),
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
fn schema() -> std::result::Result<FieldSchema<'static>, octofhir_hl7v2::schema::Error> {
    FieldSchema::compile(TYPES, SEGMENTS, CompileLimits::default())
}
fn message(body: &[u8]) -> std::result::Result<Message, octofhir_hl7v2::wire::Error> {
    Message::parse([HEADER, body].concat())
}
fn obx(code: &str, value: &str) -> std::result::Result<Message, octofhir_hl7v2::wire::Error> {
    message(format!("OBX|1|{code}|c||{value}\r").as_bytes())
}

#[test]
fn typed_roles_and_dynamic_obx_values_validate_without_copying_or_normalizing_source() -> Result {
    let schema = schema()?;
    let catalog = Catalog::compile("TEST", STRUCTURES, BINDINGS, CompileLimits::default())?;
    schema.check_catalog(&catalog)?;
    let source = message(b"OBX|1|NM|c||+000012.3400~-0.000\rOBX|2|DT|c||202402\rOBX|3|DTM|c||20240229235959.0100-0000\rOBX|4|TS|c||202402291200^L\rOBX|5|ST|c||left\\F\\right\rOBX|6|CX|c||id^^^AUTH&urn:oid&ISO\rZ99|3.1400\rZ99|20240229\r")?;
    let original = source.original().to_vec();
    let view = catalog.group(&source, MatchLimits::default())?;
    let report = schema.validate(&view, ValidationLimits::default());
    assert!(report.is_valid(), "{report:?}");
    assert_eq!(
        view.message().original().as_ptr(),
        source.original().as_ptr()
    );
    for (number, datatype) in [
        (2, "NM"),
        (3, "DT"),
        (4, "DTM"),
        (5, "TS"),
        (6, "ST"),
        (7, "CX"),
    ] {
        assert_eq!(
            schema
                .datatype(&source, "Observation", &[number, 5])
                .map_err(|_| "unresolved datatype")?,
            datatype
        );
    }
    assert_eq!(
        source.value(&[2, 5, 1]).decimal()?.as_bytes(),
        b"+000012.3400"
    );
    assert_eq!(
        source.timestamp(&[5, 5])?.precision(),
        octofhir_hl7v2::wire::Precision::Month
    );
    assert_eq!(source.original(), original);
    let bad = message(b"Z99|bad\rZ99|bad\r")?;
    let view = catalog.group(&bad, MatchLimits::default())?;
    let report = schema.validate(&view, ValidationLimits::default());
    assert!(
        report
            .issues
            .iter()
            .any(|i| i.path[..2] == [2, 1] && i.kind == IssueKind::Lexical(WireError::Number))
    );
    assert!(
        report
            .issues
            .iter()
            .any(|i| i.path[..2] == [3, 1] && i.kind == IssueKind::Lexical(WireError::Date))
    );
    Ok(())
}

#[test]
fn selectors_presence_and_lexical_errors_keep_their_exact_wire_locations() -> Result {
    let schema = schema()?;
    for (code, value, kind) in [
        ("NM", "\\X31\\.2.3", IssueKind::Lexical(WireError::Number)),
        ("DT", "20260229", IssueKind::Lexical(WireError::Date)),
        (
            "DTM",
            "202402292400",
            IssueKind::Lexical(WireError::DateTime),
        ),
        ("NM", "\\X2222\\", IssueKind::Lexical(WireError::Number)),
    ] {
        let source = obx(code, value)?;
        let report =
            schema.validate_segment(&source, "Observation", 2, ValidationLimits::default());
        let issue = report
            .issues
            .iter()
            .find(|i| i.kind == kind)
            .ok_or("missing lexical diagnostic")?;
        assert_eq!(issue.path, [2, 5, 1, 0, 0]);
        assert_eq!(Some(issue.span.clone()), source.value(&[2, 5, 1]).span());
        assert_eq!(issue.presence, Presence::Value);
    }
    for (code, presence, kind) in [
        ("", Presence::Empty, IssueKind::Selector { target: 5 }),
        ("\"\"", Presence::Null, IssueKind::Selector { target: 5 }),
        (
            "UNKNOWN",
            Presence::Value,
            IssueKind::UnknownDatatype { target: 5 },
        ),
        (
            "ID",
            Presence::Value,
            IssueKind::UnknownDatatype { target: 5 },
        ),
    ] {
        let source = obx(code, "123")?;
        let issue = schema
            .datatype(&source, "Observation", &[2, 5])
            .err()
            .ok_or("invalid selector resolved")?;
        assert_eq!(issue.kind, kind);
        assert_eq!(issue.presence, presence);
        assert_eq!(issue.path, [2, 2, 0, 0, 0]);
        assert_eq!(Some(issue.span), source.value(&[2, 2]).span());
        assert!(
            !schema
                .validate_segment(&source, "Observation", 2, ValidationLimits::default())
                .is_valid()
        );
    }
    let missing = message(b"OBX|1\r")?;
    assert_eq!(
        schema
            .datatype(&missing, "Observation", &[2, 5])
            .err()
            .ok_or("missing selector resolved")?
            .presence,
        Presence::Missing
    );
    for value in ["", "\"\""] {
        let source = obx("", value)?;
        assert!(
            schema
                .validate_segment(&source, "Observation", 2, ValidationLimits::default())
                .is_valid()
        );
        assert_eq!(
            source.value(&[2, 5]).presence(),
            if value.is_empty() {
                Presence::Empty
            } else {
                Presence::Null
            }
        );
    }
    let repeated = obx("NM~DT", "123")?;
    assert!(
        matches!(schema.datatype(&repeated, "Observation", &[2, 5]), Err(i) if i.kind == IssueKind::Wire(WireError::Composite))
    );
    let malformed = obx("TS", "2024^\\X1\\")?;
    let report = schema.validate_segment(&malformed, "Observation", 2, ValidationLimits::default());
    assert!(
        report
            .issues
            .iter()
            .any(|i| i.kind == IssueKind::Lexical(WireError::InvalidHex)
                && Some(i.span.clone()) == malformed.value(&[2, 5, 1, 2]).span())
    );
    let truncated = Message::parse_with_options(
        b"MSH|^~\\&.|S\rOBX|1|NM|c||12.\r",
        octofhir_hl7v2::wire::ParseOptions {
            max_diagnostics: 0,
            ..Default::default()
        },
    )?;
    assert!(
        schema
            .validate_segment(&truncated, "Observation", 2, ValidationLimits::default())
            .issues
            .iter()
            .any(|i| i.kind == IssueKind::Wire(WireError::Truncation))
    );
    let legacy = obx("TS", "2024^H")?;
    let report = schema.validate_segment(&legacy, "Observation", 2, ValidationLimits::default());
    assert!(
        report
            .issues
            .iter()
            .any(|i| i.kind == IssueKind::Lexical(WireError::Timestamp)
                && i.span == legacy.value(&[2, 5, 1, 2]).span().unwrap_or(0..0))
    );
    Ok(())
}

#[test]
fn length_units_nested_components_and_custom_delimiters_are_explicit() -> Result {
    let schema = schema()?;
    let nested = obx("CX", "id^unsupported")?;
    let report = schema.validate_segment(&nested, "Observation", 2, ValidationLimits::default());
    assert!(
        report
            .issues
            .iter()
            .any(|i| i.kind == IssueKind::Unmodeled && i.path == [2, 5, 1, 2, 0])
    );
    let custom = Message::parse(b"MSH.$%!?..\rOBX.1.NM.c..0001!F!2300\r")?;
    assert!(
        schema
            .validate_segment(&custom, "Observation", 2, ValidationLimits::default())
            .is_valid()
    );
    let parts = [E::optional(1, "ID"), E::required(2, "TS")];
    let mut types: Vec<_> = TYPES
        .iter()
        .map(|t| Datatype {
            name: t.name,
            kind: t.kind,
        })
        .collect();
    types.push(Datatype {
        name: "MixedTime",
        kind: K::Composite(&parts),
    });
    let definitions = [S {
        name: "Mixed",
        id: *b"ZZZ",
        fields: &[E::required(1, "MixedTime")],
    }];
    let mixed = FieldSchema::compile(&types, &definitions, CompileLimits::default())?;
    let source = message(b"ZZZ|label^202402291200&L\r")?;
    assert!(
        mixed
            .validate_segment(&source, "Mixed", 2, ValidationLimits::default())
            .is_valid()
    );
    for (length, valid) in [
        (Length::WireBytes(1), false),
        (Length::ScalarBytes(1), false),
        (Length::Utf8Characters(1), true),
    ] {
        let fields = [E {
            length: Some(length),
            ..E::optional(1, "ST")
        }];
        let definitions = [S {
            name: "Text",
            id: *b"ZZZ",
            fields: &fields,
        }];
        let schema = FieldSchema::compile(TYPES, &definitions, CompileLimits::default())?;
        let source = message(b"ZZZ|\\XCEBB\\\r")?;
        let report = schema.validate_segment(&source, "Text", 2, ValidationLimits::default());
        assert_eq!(report.is_valid(), valid, "{report:?}");
        if !valid {
            assert_eq!(report.issues[0].kind, IssueKind::Length);
        }
        let invalid_utf8 = message(b"ZZZ|\\XFF\\\r")?;
        assert!(
            schema
                .validate_segment(&invalid_utf8, "Text", 2, ValidationLimits::default())
                .issues
                .iter()
                .any(|i| i.kind == IssueKind::Encoding)
        );
    }
    let fields = [E {
        length: Some(Length::ScalarBytes(16)),
        ..E::required(1, "NM")
    }];
    let definitions = [S {
        name: "Number",
        id: *b"ZZZ",
        fields: &fields,
    }];
    let schema = FieldSchema::compile(TYPES, &definitions, CompileLimits::default())?;
    let source = message(b"ZZZ|123456789012345678901234567890\r")?;
    let report = schema.validate_segment(&source, "Number", 2, ValidationLimits::default());
    assert_eq!(report.issues[0].kind, IssueKind::Length);
    assert_eq!(
        source.value(&[2, 1]).decimal()?.as_bytes(),
        b"123456789012345678901234567890"
    );
    let null = message(b"ZZZ|\"\"\r")?;
    assert_eq!(
        schema
            .validate_segment(&null, "Number", 2, ValidationLimits::default())
            .issues[0]
            .kind,
        IssueKind::Null
    );
    for body in [b"ZZZ\r".as_slice(), b"ZZZ|\r"] {
        let source = message(body)?;
        assert_eq!(
            schema
                .validate_segment(&source, "Number", 2, ValidationLimits::default())
                .issues[0]
                .kind,
            IssueKind::Required
        );
    }
    Ok(())
}

#[test]
fn report_caps_and_work_budgets_cannot_turn_skipped_validation_into_success() -> Result {
    let schema = schema()?;
    let source = obx("NM", "invalid|\"\"")?;
    for cap in [0, 1] {
        let report = schema.validate_segment(
            &source,
            "Observation",
            2,
            ValidationLimits {
                max_issues: cap,
                ..Default::default()
            },
        );
        assert!(!report.is_valid());
        assert!(report.complete);
        assert!(report.omitted > 0);
        assert!(report.issues.len() <= cap);
    }
    let repeated = obx("NM", &format!("{}bad", "~".repeat(2048)))?;
    for limits in [
        ValidationLimits {
            max_steps: 30,
            ..Default::default()
        },
        ValidationLimits {
            max_bytes: 0,
            ..Default::default()
        },
        ValidationLimits {
            max_steps: 0,
            max_issues: 0,
            ..Default::default()
        },
    ] {
        let report = schema.validate_segment(&repeated, "Observation", 2, limits);
        assert!(!report.is_valid());
        assert!(!report.complete);
        assert!(report.steps <= limits.max_steps);
        assert!(report.bytes <= limits.max_bytes);
        assert!(report.omitted > 0 || report.issues.iter().any(|i| i.kind == IssueKind::Limit));
    }
    let single = [E {
        occurs: Occurs {
            min: 2,
            max: Some(2),
        },
        ..E::optional(1, "ST")
    }];
    let defs = [S {
        name: "Repeated",
        id: *b"ZZZ",
        fields: &single,
    }];
    let schema = FieldSchema::compile(TYPES, &defs, CompileLimits::default())?;
    for body in [b"ZZZ|one\r".as_slice(), b"ZZZ|one~~three\r"] {
        let source = message(body)?;
        assert!(
            schema
                .validate_segment(&source, "Repeated", 2, ValidationLimits::default())
                .issues
                .iter()
                .any(|i| i.kind == IssueKind::Cardinality)
        );
    }
    Ok(())
}

#[test]
fn invalid_type_graphs_selectors_and_profile_bindings_fail_before_activation() -> Result {
    for fields in [
        vec![E::optional(1, "MISSING")],
        vec![E::optional(1, "ST"), E::optional(1, "NM")],
        vec![E::by_field(5, 2, ALLOWED, Occurs::MANY)],
        vec![
            E::optional(2, "NM"),
            E::by_field(5, 2, ALLOWED, Occurs::MANY),
        ],
        vec![E {
            length: Some(Length::ScalarBytes(2)),
            ..E::optional(1, "CX")
        }],
    ] {
        let defs = [S {
            name: "Invalid",
            id: *b"OBX",
            fields: &fields,
        }];
        assert!(FieldSchema::compile(TYPES, &defs, CompileLimits::default()).is_err());
    }
    let cycle = [Datatype {
        name: "Loop",
        kind: K::Composite(&[E::optional(1, "Loop")]),
    }];
    assert!(FieldSchema::compile(&cycle, &[], CompileLimits::default()).is_err());
    // A previously visited child cannot bypass a smaller remaining depth budget.
    for (scalar, composite) in [("A", "Z"), ("Z", "A")] {
        let components = [E::optional(1, scalar)];
        let types = [
            Datatype {
                name: scalar,
                kind: K::Primitive(P::Bytes),
            },
            Datatype {
                name: composite,
                kind: K::Composite(&components),
            },
        ];
        for max_depth in [1, 2] {
            let result = FieldSchema::compile(
                &types,
                &[],
                CompileLimits {
                    max_depth,
                    ..Default::default()
                },
            );
            assert_eq!(result.is_ok(), max_depth == 2);
        }
    }
    let deep = [Datatype {
        name: "TooDeep",
        kind: K::Composite(&[E::optional(1, "CX")]),
    }];
    let mut types = Vec::new();
    for t in TYPES {
        types.push(Datatype {
            name: t.name,
            kind: t.kind,
        });
    }
    types.extend(deep);
    assert!(FieldSchema::compile(&types, SEGMENTS, CompileLimits::default()).is_err());
    for limits in [
        CompileLimits {
            max_rules: 1,
            ..Default::default()
        },
        CompileLimits {
            max_work: 1,
            ..Default::default()
        },
    ] {
        assert!(FieldSchema::compile(TYPES, SEGMENTS, limits).is_err());
    }
    let rules = [Rule::segment("msh", *b"MSH", Occurs::ONCE)];
    let definitions = [Structure {
        name: "TEST",
        rules: &rules,
    }];
    let catalog = Catalog::compile("TEST", &definitions, BINDINGS, CompileLimits::default())?;
    let schema = schema()?;
    assert!(schema.check_catalog(&catalog).is_err());
    let source = message(b"")?;
    let view = catalog.group(&source, MatchLimits::default())?;
    assert_eq!(
        schema.validate(&view, ValidationLimits::default()).issues[0].kind,
        IssueKind::Unmodeled
    );
    assert!(
        !schema
            .validate_segment(&source, "Observation", 1, ValidationLimits::default())
            .is_valid()
    );
    Ok(())
}
