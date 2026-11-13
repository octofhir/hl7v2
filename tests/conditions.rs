use octofhir_hl7v2::{
    schema::{
        CompileLimits, ErrorKind, Occurs,
        fields::{
            Condition, Datatype, DatatypeKind as K, Element as E, FieldSchema, IssueKind,
            Predicate, Primitive as P, Requirement, SegmentDefinition as S, ValidationLimits,
        },
    },
    wire::{ErrorKind as WireError, Message, ParseOptions, Presence},
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
        name: "NM",
        kind: K::Primitive(P::Number),
    },
    Datatype {
        name: "STATUS",
        kind: K::Codes(&["F", "P"]),
    },
    Datatype {
        name: "PAIR",
        kind: K::Composite(&[E::optional(1, "B")]),
    },
];
const FIELDS: &[E<'_>] = &[
    E {
        nullable: true,
        ..E::optional(1, "ID")
    },
    E {
        nullable: true,
        occurs: Occurs::MANY,
        ..E::optional(2, "B")
    },
];
const PROFILES: &[S<'_>] = &[S {
    name: "Local",
    id: *b"Z99",
    fields: FIELDS,
}];
fn message(body: &[u8]) -> std::result::Result<Message, octofhir_hl7v2::wire::Error> {
    Message::parse([b"MSH|^~\\&\r".as_slice(), body, b"\r"].concat())
}
fn rule(when: Predicate<'_>, requirement: Requirement) -> Condition<'_> {
    Condition {
        profile: "Local",
        target: 2,
        when,
        requirement,
    }
}

#[test]
fn presence_conditions_observe_all_four_states_without_mutation_or_order_dependent_effects()
-> Result {
    for state in [
        Presence::Missing,
        Presence::Empty,
        Presence::Null,
        Presence::Value,
    ] {
        for requirement in [Requirement::Required, Requirement::Forbidden] {
            let rules = [rule(Predicate::Is { field: 1, state }, requirement)];
            let schema = FieldSchema::compile_with_conditions(
                TYPES,
                PROFILES,
                &rules,
                CompileLimits::default(),
            )?;
            for body in [
                b"Z99".as_slice(),
                b"Z99|",
                b"Z99|\"\"",
                b"Z99|F",
                b"Z99||",
                b"Z99||\"\"",
                b"Z99||T",
                b"Z99|\"\"|T",
                b"Z99|\"\"|\"\"",
                b"Z99|F|",
                b"Z99|F|\"\"",
                b"Z99|F|T",
                b"Z99|F|~",
                b"Z99|F|T~",
                b"Z99|F|\"\"~\"\"",
            ] {
                let source = message(body)?;
                let target = source.value(&[2, 2]);
                let active = source.value(&[2, 1]).presence() == state;
                let required = target.presence() == Presence::Null
                    || (target.presence() == Presence::Value
                        && (1..=target.child_count()).all(|r| {
                            matches!(target.child(r).presence(), Presence::Value | Presence::Null)
                        }));
                let forbidden = matches!(target.presence(), Presence::Missing | Presence::Empty);
                let valid = !active
                    || match requirement {
                        Requirement::Required => required,
                        Requirement::Forbidden => forbidden,
                    };
                let report =
                    schema.validate_segment(&source, "Local", 2, ValidationLimits::default());
                assert_eq!(
                    report.is_valid(),
                    valid,
                    "{state:?} {requirement:?} {body:?}: {report:?}"
                );
                assert!(report.complete);
                for issue in &report.issues {
                    assert_eq!(
                        issue.kind,
                        IssueKind::Conditional {
                            selector: 1,
                            requirement
                        }
                    );
                    assert_eq!(&issue.path[..2], &[2, 2]);
                    assert!(issue.span.end <= source.original().len());
                }
                assert_eq!(
                    source.original(),
                    [b"MSH|^~\\&\r".as_slice(), body, b"\r"].concat()
                );
            }
        }
    }
    Ok(())
}

#[test]
fn literal_conditions_decode_once_distinguish_null_and_preserve_numeric_spelling_and_wire_failures()
-> Result {
    for (literal, body, valid) in [
        ("F", b"Z99|F".as_slice(), false),
        ("F", b"Z99|\\X46\\", false),
        ("F", b"Z99|f", true),
        ("F", b"Z99|F|T", true),
        ("F", b"Z99|\"\"", true),
        ("\"\"", b"Z99|\"\"", true),
        ("\"\"", b"Z99|\\X2222\\", false),
        ("A|B", b"Z99|A\\F\\B", false),
    ] {
        let rules = [rule(
            Predicate::Equals {
                field: 1,
                value: literal,
            },
            Requirement::Required,
        )];
        let schema = FieldSchema::compile_with_conditions(
            TYPES,
            PROFILES,
            &rules,
            CompileLimits::default(),
        )?;
        let source = message(body)?;
        assert_eq!(
            schema
                .validate_segment(&source, "Local", 2, ValidationLimits::default())
                .is_valid(),
            valid
        );
    }
    let rules = [rule(
        Predicate::Equals {
            field: 1,
            value: "F",
        },
        Requirement::Required,
    )];
    let schema =
        FieldSchema::compile_with_conditions(TYPES, PROFILES, &rules, CompileLimits::default())?;
    let source = message(b"Z99|F~P")?;
    let report = schema.validate_segment(&source, "Local", 2, ValidationLimits::default());
    assert!(
        report
            .issues
            .iter()
            .any(|i| i.kind == IssueKind::ConditionSelector { target: 2 })
    );
    assert!(!report.is_valid());
    let source = Message::parse_with_options(
        b"MSH|^~\\&#\rZ99|F#\r",
        ParseOptions {
            max_diagnostics: 0,
            ..Default::default()
        },
    )?;
    let report = schema.validate_segment(&source, "Local", 2, ValidationLimits::default());
    assert!(
        report
            .issues
            .iter()
            .any(|i| i.kind == IssueKind::Wire(WireError::Truncation))
    );
    let fields = [E::optional(1, "NM"), FIELDS[1]];
    let profiles = [S {
        fields: &fields,
        ..PROFILES[0]
    }];
    let rules = [rule(
        Predicate::Equals {
            field: 1,
            value: "+001.0",
        },
        Requirement::Required,
    )];
    let schema =
        FieldSchema::compile_with_conditions(TYPES, &profiles, &rules, CompileLimits::default())?;
    assert!(
        !schema
            .validate_segment(
                &message(b"Z99|+001.0")?,
                "Local",
                2,
                ValidationLimits::default()
            )
            .is_valid()
    );
    assert!(
        schema
            .validate_segment(&message(b"Z99|1")?, "Local", 2, ValidationLimits::default())
            .is_valid()
    );
    Ok(())
}

#[test]
fn condition_definitions_references_conflicts_and_budgets_are_bounded() -> Result {
    let base = rule(
        Predicate::Equals {
            field: 1,
            value: "F",
        },
        Requirement::Required,
    );
    for invalid in [
        Condition {
            profile: "Unknown",
            ..base
        },
        Condition { target: 3, ..base },
        Condition {
            when: Predicate::Is {
                field: 9,
                state: Presence::Value,
            },
            ..base
        },
        Condition { target: 1, ..base },
        Condition {
            when: Predicate::Equals {
                field: 1,
                value: "",
            },
            ..base
        },
        Condition {
            when: Predicate::Equals {
                field: 1,
                value: "bad\0value",
            },
            ..base
        },
    ] {
        assert!(
            matches!(FieldSchema::compile_with_conditions(TYPES, PROFILES, &[invalid], CompileLimits::default()), Err(e) if e.kind == ErrorKind::Definition)
        );
    }
    for duplicate in [
        base,
        Condition {
            requirement: Requirement::Forbidden,
            ..base
        },
    ] {
        assert!(
            matches!(FieldSchema::compile_with_conditions(TYPES, PROFILES, &[base, duplicate], CompileLimits::default()), Err(e) if e.kind == ErrorKind::Definition)
        );
    }
    for fields in [
        [
            E {
                occurs: Occurs::MANY,
                ..FIELDS[0]
            },
            FIELDS[1],
        ],
        [E::optional(1, "PAIR"), FIELDS[1]],
        [FIELDS[0], E::required(2, "B")],
        [
            FIELDS[0],
            E {
                occurs: Occurs {
                    min: 0,
                    max: Some(0),
                },
                ..FIELDS[1]
            },
        ],
        [E::optional(1, "NM"), FIELDS[1]],
        [E::optional(1, "STATUS"), FIELDS[1]],
    ] {
        let profiles = [S {
            fields: &fields,
            ..PROFILES[0]
        }];
        let invalid = Condition {
            when: Predicate::Equals {
                field: 1,
                value: "X",
            },
            ..base
        };
        assert!(
            matches!(FieldSchema::compile_with_conditions(TYPES, &profiles, &[invalid], CompileLimits::default()), Err(e) if e.kind == ErrorKind::Definition)
        );
    }
    let literal = "F".repeat(4097);
    let rules = [Condition {
        when: Predicate::Equals {
            field: 1,
            value: &literal,
        },
        ..base
    }];
    assert!(
        matches!(FieldSchema::compile_with_conditions(TYPES, PROFILES, &rules, CompileLimits::default()), Err(e) if e.kind == ErrorKind::Limit)
    );
    let rules = [base];
    let schema =
        FieldSchema::compile_with_conditions(TYPES, PROFILES, &rules, CompileLimits::default())?;
    let source = message(b"Z99|P")?;
    let report = schema.validate_segment(
        &source,
        "Local",
        2,
        ValidationLimits {
            max_bytes: 3,
            ..Default::default()
        },
    );
    assert!(!report.complete && !report.is_valid());
    assert!(
        schema
            .validate_segment(
                &source,
                "Local",
                2,
                ValidationLimits {
                    max_bytes: 4,
                    ..Default::default()
                }
            )
            .is_valid()
    );
    let report = schema.validate_segment(
        &message(b"Z99|F")?,
        "Local",
        2,
        ValidationLimits {
            max_issues: 0,
            ..Default::default()
        },
    );
    assert!(report.complete && report.omitted > 0 && !report.is_valid());
    let huge = message(&[b"Z99|".as_slice(), &vec![b'F'; 20_481]].concat())?;
    let report = schema.validate_segment(&huge, "Local", 2, ValidationLimits::default());
    assert!(
        report
            .issues
            .iter()
            .any(|i| i.kind == IssueKind::ConditionSelector { target: 2 })
    );
    assert!(!report.is_valid());
    Ok(())
}

#[cfg(feature = "schema-package")]
#[test]
fn v3_restores_owned_conditions_and_preserves_canonical_identity_and_strict_loading() -> Result {
    use octofhir_hl7v2::{
        schema::{
            Binding, MatchLimits, Rule, Structure,
            fields::Length,
            package::{
                Definitions, Error, FileError, LoadLimits, Manifest, OwnedPackage, Package, Source,
            },
        },
        wire::Edit,
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
            kind: K::Codes(&["P", "F"]),
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
            fields: &[
                E {
                    length: Some(Length::Utf8Characters(1)),
                    ..E::required(1, "STATUS")
                },
                E {
                    nullable: true,
                    ..E::optional(2, "ID")
                },
            ],
        },
    ];
    const STRUCTURES: &[Structure<'_>] = &[Structure {
        name: "CONDITIONS",
        rules: &[
            Rule::typed_segment("msh", *b"MSH", "Header", Occurs::ONCE),
            Rule::typed_segment("value", *b"ZZZ", "Value", Occurs::ONCE),
        ],
    }];
    const BINDINGS: &[Binding<'_>] = &[Binding {
        message: "ZZZ",
        event: "Z01",
        structure: "CONDITIONS",
    }];
    const SOURCES: &[Source<'_>] = &[Source {
        name: "synthetic",
        location: "urn:test:conditions",
        revision: "1",
        sha256: [4; 32],
        license: "MIT OR Apache-2.0",
        notice: "Original synthetic fixture",
    }];
    let manifest = Manifest {
        name: "synthetic-conditions",
        release: "TEST",
        revision: "1",
        producer: "test-author@1",
        scope: "Same-segment conditional presence",
        limitations: "No standard catalog or cross-segment rules",
        sources: SOURCES,
    };
    let definitions = || Definitions {
        structures: STRUCTURES,
        bindings: BINDINGS,
        datatypes: TYPES,
        segments: SEGMENTS,
    };
    let rules = [
        Condition {
            profile: "Value",
            target: 2,
            when: Predicate::Equals {
                field: 1,
                value: "F",
            },
            requirement: Requirement::Required,
        },
        Condition {
            profile: "Value",
            target: 2,
            when: Predicate::Equals {
                field: 1,
                value: "P",
            },
            requirement: Requirement::Forbidden,
        },
    ];
    let pin = [
        0x47, 0xcc, 0xa6, 0x23, 0x56, 0x0e, 0x9e, 0x6c, 0x4c, 0xf4, 0x7e, 0x3e, 0x0b, 0x56, 0xfd,
        0x60, 0x53, 0xbc, 0xa1, 0xf9, 0xa5, 0x87, 0x10, 0x10, 0xa5, 0x24, 0x4d, 0x46, 0x6a, 0x04,
        0x74, 0x69,
    ];
    let file = include_bytes!("fixtures/native-package-v3.bin");
    let native = Package::compile_with_conditions(
        manifest,
        definitions(),
        &rules,
        CompileLimits::default(),
    )?
    .require_digest(pin)?;
    assert_eq!(native.format_version(), 3);
    assert_eq!(native.encode(file.len())?, file);
    let owned = {
        let bytes = file.to_vec();
        OwnedPackage::decode(&bytes, pin, LoadLimits::default())?
    };
    assert_eq!(owned.format_version(), 3);
    assert_eq!(owned.encode(file.len())?, file);
    for (code, target, valid) in [
        ("F", Edit::Empty, false),
        ("F", Edit::Null, true),
        ("F", Edit::Value(b"present".to_vec()), true),
        ("P", Edit::Empty, true),
        ("P", Edit::Null, false),
        ("P", Edit::Value(b"present".to_vec()), false),
    ] {
        let source = Message::parse(include_bytes!("fixtures/native-conditions.er7"))?;
        let mut edit = source.edit();
        edit.set(&[2, 1], Edit::Value(code.as_bytes().to_vec()))?;
        edit.set(&[2, 2], target)?;
        let edited = edit.finish()?;
        let a = owned.fields().validate(
            &owned.catalog().group(&edited, MatchLimits::default())?,
            ValidationLimits::default(),
        );
        let b = native.fields().validate(
            &native.catalog().group(&edited, MatchLimits::default())?,
            ValidationLimits::default(),
        );
        assert_eq!(a.issues, b.issues);
        assert_eq!(a.is_valid(), valid);
    }
    let reversed = [rules[1], rules[0]];
    assert_eq!(
        Package::compile_with_conditions(
            manifest,
            definitions(),
            &reversed,
            CompileLimits::default()
        )?
        .require_digest(pin)?
        .encode(file.len())?,
        file
    );
    let changed = [
        Condition {
            requirement: Requirement::Forbidden,
            ..rules[0]
        },
        rules[1],
    ];
    assert!(matches!(
        Package::compile_with_conditions(
            manifest,
            definitions(),
            &changed,
            CompileLimits::default()
        )?
        .require_digest(pin),
        Err(Error::DigestMismatch)
    ));
    // Presence predicates survive the file path too, including the null/value distinction.
    for state in [
        Presence::Missing,
        Presence::Empty,
        Presence::Null,
        Presence::Value,
    ] {
        let conditions = [Condition {
            when: Predicate::Is { field: 1, state },
            ..rules[0]
        }];
        let package = Package::compile_with_conditions(
            manifest,
            definitions(),
            &conditions,
            CompileLimits::default(),
        )?;
        let bytes = package.encode(8192)?;
        let loaded = OwnedPackage::decode(&bytes, package.digest(), LoadLimits::default())?;
        assert_eq!(loaded.encode(8192)?, bytes);
        let mut bad_state = bytes.clone();
        let at = bad_state.len() - 2;
        bad_state[at] = 4;
        assert!(matches!(
            OwnedPackage::decode(&bad_state, package.digest(), LoadLimits::default()),
            Err(FileError::Format { .. })
        ));
        let source = Message::parse(b"MSH|^~\\&\rZZZ|F\r")?;
        assert_eq!(
            loaded
                .fields()
                .validate_segment(&source, "Value", 2, ValidationLimits::default())
                .is_valid(),
            state != Presence::Value
        );
    }
    for end in 0..file.len() {
        assert!(OwnedPackage::decode(&file[..end], pin, LoadLimits::default()).is_err());
    }
    let mut downgrade = file.to_vec();
    downgrade[8..16].copy_from_slice(&2_u64.to_be_bytes());
    downgrade[16..24].copy_from_slice(&2_u64.to_be_bytes());
    assert!(OwnedPackage::decode(&downgrade, pin, LoadLimits::default()).is_err());
    // The final two conditional records are fixed independently of the Rust encoder.
    let first = file.len() - 54;
    assert_eq!(&file[first..first + 8], &2_u64.to_be_bytes());
    for offset in [first + 16, first + 26] {
        let mut bad = file.to_vec();
        bad[offset] = 255;
        assert!(matches!(
            OwnedPackage::decode(&bad, pin, LoadLimits::default()),
            Err(FileError::Format { .. })
        ));
    }
    let mut excessive = file.to_vec();
    excessive[first - 8..first].copy_from_slice(&u64::MAX.to_be_bytes());
    assert!(matches!(
        OwnedPackage::decode(&excessive, pin, LoadLimits::default()),
        Err(FileError::Limit)
    ));
    Ok(())
}
