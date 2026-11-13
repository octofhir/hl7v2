use octofhir_hl7v2::{
    schema::{
        CompileLimits, ErrorKind, Occurs,
        fields::{
            Datatype, DatatypeKind as K, Element as E, FieldSchema, IssueKind, Length,
            Primitive as P, SegmentDefinition as S, ValidationLimits,
        },
    },
    wire::{Edit, ErrorKind as WireError, Message, ParseOptions, Presence},
};
type Result = std::result::Result<(), Box<dyn std::error::Error>>;
const CODES: &[&str] = &["P", "F", "C", "A|B", "A*B", "\"\"", "é", " ", "F#"];
const TYPES: &[Datatype<'_>] = &[
    Datatype {
        name: "CODE",
        kind: K::Codes(CODES),
    },
    Datatype {
        name: "PAIR",
        kind: K::Composite(&[E::required(1, "CODE"), E::optional(2, "CODE")]),
    },
    Datatype {
        name: "DEEP",
        kind: K::Composite(&[E::required(1, "PAIR")]),
    },
    Datatype {
        name: "SELECT",
        kind: K::Codes(&["CODE", "NM"]),
    },
    Datatype {
        name: "NM",
        kind: K::Primitive(P::Number),
    },
];
const PROFILES: &[S<'_>] = &[
    S {
        name: "Value",
        id: *b"Z99",
        fields: &[E {
            nullable: true,
            occurs: Occurs { min: 1, max: None },
            ..E::required(1, "CODE")
        }],
    },
    S {
        name: "Nested",
        id: *b"Z99",
        fields: &[E::required(1, "DEEP")],
    },
    S {
        name: "Dynamic",
        id: *b"Z99",
        fields: &[
            E::required(1, "SELECT"),
            E::by_field(2, 1, &["CODE", "NM"], Occurs::ONCE),
        ],
    },
];
fn schema() -> std::result::Result<FieldSchema<'static>, octofhir_hl7v2::schema::Error> {
    FieldSchema::compile(TYPES, PROFILES, CompileLimits::default())
}
fn message(body: &[u8]) -> std::result::Result<Message, octofhir_hl7v2::wire::Error> {
    Message::parse([b"MSH|^~\\&\r".as_slice(), body].concat())
}

#[test]
fn closed_codes_preserve_exact_decoded_values_presence_repetitions_and_source_diagnostics() -> Result
{
    let schema = schema()?;
    for (value, valid) in [
        (b"F".as_slice(), true),
        (b"\\X46\\", true),
        (b"A\\F\\B", true),
        (b"\\X2222\\", true),
        ("é".as_bytes(), true),
        (b" ", true),
        (b"f", false),
        (b"F ", false),
        ("e\u{301}".as_bytes(), false),
        (b"01", false),
        (b"", false),
    ] {
        let source = message(&[b"Z99|", value, b"\r"].concat())?;
        let report = schema.validate_segment(&source, "Value", 2, ValidationLimits::default());
        assert_eq!(report.is_valid(), valid, "{value:?}: {report:?}");
        assert_eq!(source.value(&[2, 1]).raw(), Some(value));
        assert!(report.complete);
    }
    let source = message(b"Z99|F~secret-invalid~\\X2222\\~\"\"~\r")?;
    let report = schema.validate_segment(&source, "Value", 2, ValidationLimits::default());
    assert_eq!(report.issues.len(), 2);
    assert_eq!(report.issues[0].kind, IssueKind::Code);
    assert_eq!(report.issues[0].path, [2, 1, 2, 0, 0]);
    assert_eq!(
        source.original().get(report.issues[0].span.clone()),
        Some(b"secret-invalid".as_slice())
    );
    assert_eq!(report.issues[0].presence, Presence::Value);
    assert_eq!(report.issues[1].kind, IssueKind::Required);
    assert!(!format!("{report:?}").contains("secret-invalid"));
    let missing = message(b"Z99\r")?;
    let report = schema.validate_segment(&missing, "Value", 2, ValidationLimits::default());
    assert_eq!(report.issues[0].presence, Presence::Missing);
    let invalid_utf8 = message(b"Z99|\xff\r")?;
    let report = schema.validate_segment(&invalid_utf8, "Value", 2, ValidationLimits::default());
    assert_eq!(report.issues[0].kind, IssueKind::Encoding);
    let truncated = Message::parse_with_options(
        b"MSH|^~\\&#\rZ99|F#\r",
        ParseOptions {
            max_diagnostics: 0,
            ..Default::default()
        },
    )?;
    let report = schema.validate_segment(&truncated, "Value", 2, ValidationLimits::default());
    assert_eq!(
        report.issues[0].kind,
        IssueKind::Wire(WireError::Truncation)
    );
    let mut editor = truncated.edit();
    editor.set(&[2, 1], Edit::Value(b"F#".to_vec()))?;
    let escaped = editor.finish()?;
    assert!(
        schema
            .validate_segment(&escaped, "Value", 2, ValidationLimits::default())
            .is_valid()
    );
    Ok(())
}

#[test]
fn codes_work_in_nested_components_dynamic_selectors_and_explicit_length_units() -> Result {
    let schema = schema()?;
    for (body, profile, valid) in [
        (b"Z99|F&\xc3\xa9\r".as_slice(), "Nested", true),
        (b"Z99|F&f\r", "Nested", false),
        (b"Z99|CODE|A\\F\\B\r", "Dynamic", true),
        (b"Z99|NM|+0001.2300\r", "Dynamic", true),
        (b"Z99|UNKNOWN|F\r", "Dynamic", false),
    ] {
        let source = message(body)?;
        let report = schema.validate_segment(&source, profile, 2, ValidationLimits::default());
        assert_eq!(report.is_valid(), valid, "{profile}: {report:?}");
        if profile == "Nested" && !valid {
            assert_eq!(report.issues[0].path, [2, 1, 1, 1, 2]);
        }
    }
    let custom = Message::parse(b"MSH*$%!?\rZ99*A!F!B\r")?;
    assert!(
        schema
            .validate_segment(&custom, "Value", 2, ValidationLimits::default())
            .is_valid()
    );
    for (length, valid) in [
        (Length::WireBytes(1), false),
        (Length::ScalarBytes(1), false),
        (Length::Utf8Characters(1), true),
    ] {
        let fields = [E {
            length: Some(length),
            ..E::required(1, "CODE")
        }];
        let profiles = [S {
            name: "Value",
            id: *b"Z99",
            fields: &fields,
        }];
        let schema = FieldSchema::compile(TYPES, &profiles, CompileLimits::default())?;
        let source = message("Z99|é\r".as_bytes())?;
        assert_eq!(
            schema
                .validate_segment(&source, "Value", 2, ValidationLimits::default())
                .is_valid(),
            valid
        );
        let null = message(b"Z99|\"\"\r")?;
        assert_eq!(
            schema
                .validate_segment(&null, "Value", 2, ValidationLimits::default())
                .issues[0]
                .kind,
            IssueKind::Null
        );
    }
    Ok(())
}

#[test]
fn invalid_sets_and_compile_runtime_report_budgets_fail_closed() -> Result {
    let fields = [E::required(1, "CODE")];
    let profiles = [S {
        name: "Value",
        id: *b"Z99",
        fields: &fields,
    }];
    for values in [&[][..], &[""], &["A", "A"], &["\0"], &["A\nB"], &["\u{85}"]] {
        let types = [Datatype {
            name: "CODE",
            kind: K::Codes(values),
        }];
        assert!(
            matches!(FieldSchema::compile(&types, &profiles, CompileLimits::default()), Err(e) if e.kind == ErrorKind::Definition)
        );
    }
    let too_long = "X".repeat(4097);
    let values = [too_long.as_str()];
    let types = [Datatype {
        name: "CODE",
        kind: K::Codes(&values),
    }];
    assert!(
        matches!(FieldSchema::compile(&types, &profiles, CompileLimits::default()), Err(e) if e.kind == ErrorKind::Limit)
    );
    let long = "X".repeat(1024);
    let values = [long.as_str()];
    let types = [Datatype {
        name: "CODE",
        kind: K::Codes(&values),
    }];
    assert!(
        matches!(FieldSchema::compile(&types, &profiles, CompileLimits { max_work: 1024, ..Default::default() }), Err(e) if e.kind == ErrorKind::Limit)
    );
    let types = [Datatype {
        name: "CODE",
        kind: K::Codes(&["A"]),
    }];
    assert!(
        matches!(FieldSchema::compile(&types, &profiles, CompileLimits { max_rules: 3, ..Default::default() }), Err(e) if e.kind == ErrorKind::Limit)
    );
    let schema = FieldSchema::compile(&types, &profiles, CompileLimits::default())?;
    let source = message(b"Z99|A\r")?;
    let report = schema.validate_segment(
        &source,
        "Value",
        2,
        ValidationLimits {
            max_bytes: 1,
            ..Default::default()
        },
    );
    assert!(!report.complete && !report.is_valid());
    assert_eq!(report.issues[0].kind, IssueKind::Limit);
    let report = schema.validate_segment(
        &source,
        "Value",
        2,
        ValidationLimits {
            max_bytes: 3,
            ..Default::default()
        },
    );
    assert!(report.is_valid());
    let invalid = message(b"Z99|B\r")?;
    let report = schema.validate_segment(
        &invalid,
        "Value",
        2,
        ValidationLimits {
            max_issues: 0,
            ..Default::default()
        },
    );
    assert!(report.complete && report.omitted > 0 && !report.is_valid());
    Ok(())
}

#[cfg(feature = "schema-package")]
#[test]
fn code_packages_use_v2_while_historical_v1_identity_and_exports_remain_exact() -> Result {
    use octofhir_hl7v2::schema::{
        Binding, MatchLimits, Rule, Structure,
        package::{
            Definitions, Error, FileError, LoadLimits, Manifest, OwnedPackage, Package, Source,
        },
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
            name: "MSG",
            kind: K::Composite(&[
                E::required(1, "ID"),
                E::required(2, "ID"),
                E::required(3, "ID"),
            ]),
        },
        Datatype {
            name: "STATUS",
            kind: K::Codes(&["P", "é", "C", "F"]),
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
            name: "Value",
            id: *b"ZZZ",
            fields: &[E {
                length: Some(Length::Utf8Characters(1)),
                ..E::required(1, "STATUS")
            }],
        },
    ];
    const STRUCTURES: &[Structure<'_>] = &[Structure {
        name: "CODES",
        rules: &[
            Rule::typed_segment("msh", *b"MSH", "Header", Occurs::ONCE),
            Rule::typed_segment("value", *b"ZZZ", "Value", Occurs::ONCE),
        ],
    }];
    const BINDINGS: &[Binding<'_>] = &[Binding {
        message: "ZZZ",
        event: "Z01",
        structure: "CODES",
    }];
    const SOURCES: &[Source<'_>] = &[Source {
        name: "synthetic",
        location: "urn:test:codes",
        revision: "1",
        sha256: [3; 32],
        license: "MIT OR Apache-2.0",
        notice: "Original synthetic fixture",
    }];
    let manifest = Manifest {
        name: "synthetic-codes",
        release: "TEST",
        revision: "1",
        producer: "test-author@1",
        scope: "Closed synthetic field codes",
        limitations: "No standard catalog or terminology service",
        sources: SOURCES,
    };
    let definitions = |types| Definitions {
        structures: STRUCTURES,
        bindings: BINDINGS,
        datatypes: types,
        segments: SEGMENTS,
    };
    let pin = [
        0x95, 0x99, 0x42, 0xd4, 0x03, 0x06, 0x16, 0x1c, 0xf3, 0x28, 0xd5, 0x07, 0x7b, 0x24, 0x2b,
        0x94, 0x66, 0x24, 0x45, 0xb0, 0x57, 0xb1, 0xf6, 0xfb, 0x25, 0x3b, 0x91, 0xb5, 0x6e, 0xb6,
        0x82, 0xef,
    ];
    let file = include_bytes!("fixtures/native-package-v2.bin");
    let native = Package::compile(manifest, definitions(TYPES), CompileLimits::default())?
        .require_digest(pin)?;
    assert_eq!(native.format_version(), 2);
    assert_eq!(native.encode(file.len())?, file);
    let owned = {
        let bytes = file.to_vec();
        OwnedPackage::decode(&bytes, pin, LoadLimits::default())?
    };
    assert_eq!(owned.format_version(), 2);
    assert_eq!(owned.encode(file.len())?, file);
    for value in ["F", "é", "f", "FF", ""] {
        let source = Message::parse(include_bytes!("fixtures/native-codes.er7"))?;
        let mut edit = source.edit();
        edit.set(&[2, 1], Edit::Value(value.as_bytes().to_vec()))?;
        let edited = edit.finish()?;
        let view = owned.catalog().group(&edited, MatchLimits::default())?;
        let a = owned.fields().validate(&view, ValidationLimits::default());
        let native_view = native.catalog().group(&edited, MatchLimits::default())?;
        let b = native
            .fields()
            .validate(&native_view, ValidationLimits::default());
        assert_eq!(a.issues, b.issues);
        assert_eq!(a.is_valid(), matches!(value, "F" | "é"));
    }
    let mut types: Vec<_> = TYPES
        .iter()
        .map(|t| Datatype {
            name: t.name,
            kind: t.kind,
        })
        .collect();
    types[3].kind = K::Codes(&["F", "C", "é", "P"]);
    assert_eq!(
        Package::compile(manifest, definitions(&types), CompileLimits::default())?
            .require_digest(pin)?
            .encode(file.len())?,
        file
    );
    let mut changed: Vec<_> = TYPES
        .iter()
        .map(|t| Datatype {
            name: t.name,
            kind: t.kind,
        })
        .collect();
    changed[3].kind = K::Codes(&["F", "C", "é", "X"]);
    assert!(matches!(
        Package::compile(manifest, definitions(&changed), CompileLimits::default())?
            .require_digest(pin),
        Err(Error::DigestMismatch)
    ));
    let legacy_pin = [
        0x01, 0x61, 0x08, 0x1c, 0x3e, 0xb1, 0x55, 0x36, 0xd2, 0x63, 0xba, 0x1b, 0x1f, 0x35, 0x77,
        0x25, 0xb0, 0xef, 0xc4, 0x3b, 0x7e, 0x2d, 0x02, 0xa5, 0x8b, 0x54, 0x1d, 0xda, 0x1e, 0x66,
        0x9c, 0xd1,
    ];
    let legacy_file = include_bytes!("fixtures/native-package-v1.bin");
    let legacy = OwnedPackage::decode(legacy_file, legacy_pin, LoadLimits::default())?;
    assert_eq!(legacy.format_version(), 1);
    assert_eq!(legacy.encode(legacy_file.len())?, legacy_file);
    for (bytes, pin, version) in [
        (file.as_slice(), pin, 1_u64),
        (legacy_file.as_slice(), legacy_pin, 2),
    ] {
        let mut wrong = bytes.to_vec();
        wrong[8..16].copy_from_slice(&version.to_be_bytes());
        wrong[16..24].copy_from_slice(&version.to_be_bytes());
        assert!(OwnedPackage::decode(&wrong, pin, LoadLimits::default()).is_err());
    }
    for end in 0..file.len() {
        assert!(OwnedPackage::decode(&file[..end], pin, LoadLimits::default()).is_err());
    }
    let at = file
        .windows(7)
        .position(|s| s == b"STATUS\x03")
        .ok_or("missing code kind")?
        + 6;
    let mut unknown = file.to_vec();
    unknown[at] = 4;
    assert!(matches!(
        OwnedPackage::decode(&unknown, pin, LoadLimits::default()),
        Err(FileError::Format { .. })
    ));
    let mut excessive = file.to_vec();
    excessive[at + 1..at + 9].copy_from_slice(&u64::MAX.to_be_bytes());
    assert!(matches!(
        OwnedPackage::decode(&excessive, pin, LoadLimits::default()),
        Err(FileError::Limit)
    ));
    Ok(())
}
