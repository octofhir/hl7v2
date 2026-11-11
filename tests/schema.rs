use octofhir_hl7v2::{
    schema::{
        Binding, Catalog, CompileLimits, Content, ErrorKind, MatchLimits, Occurs, Rule, Structure,
    },
    wire::{Edit, Message, Presence},
};
type Result = std::result::Result<(), Box<dyn std::error::Error>>;
const ONCE: Occurs = Occurs::ONCE;
const MANY: Occurs = Occurs::MANY;
const REQUIRED: Occurs = Occurs { min: 1, max: None };
const OBSERVATION: &[Rule<'_>] = &[
    Rule::segment("obx", *b"OBX", ONCE),
    Rule::segment("notes", *b"NTE", MANY),
];
const ORDER: &[Rule<'_>] = &[
    Rule::segment("obr", *b"OBR", ONCE),
    Rule::group("observation", OBSERVATION, MANY),
];
const PATIENT: &[Rule<'_>] = &[
    Rule::segment("pid", *b"PID", ONCE),
    Rule::group("order", ORDER, REQUIRED),
];
const ROOT: &[Rule<'_>] = &[
    Rule::segment("msh", *b"MSH", ONCE),
    Rule::group("patient", PATIENT, REQUIRED),
    Rule::segment("extension", *b"ZZZ", Occurs::OPTIONAL),
];
const SHAPE: &[Rule<'_>] = &[
    Rule::segment("msh", *b"MSH", ONCE),
    Rule::segment("pid", *b"PID", Occurs::OPTIONAL),
];
// Original synthetic subsets. These are not full ADT/ORU definitions.
const STRUCTURES: &[Structure<'_>] = &[
    Structure {
        name: "ADT_A03",
        rules: SHAPE,
    },
    Structure {
        name: "ADT_A01",
        rules: SHAPE,
    },
    Structure {
        name: "SYNTHETIC_ORU",
        rules: ROOT,
    },
];
const BINDINGS: &[Binding<'_>] = &[
    Binding {
        message: "ADT",
        event: "A08",
        structure: "ADT_A01",
    },
    Binding {
        message: "ADT",
        event: "A01",
        structure: "ADT_A01",
    },
    Binding {
        message: "ADT",
        event: "A03",
        structure: "ADT_A03",
    },
    Binding {
        message: "ZZZ",
        event: "Z01",
        structure: "SYNTHETIC_ORU",
    },
];
fn catalog() -> std::result::Result<Catalog<'static>, octofhir_hl7v2::schema::Error> {
    Catalog::compile("2.5.1", STRUCTURES, BINDINGS, CompileLimits::default())
}
fn message(
    kind: &str,
    version: &str,
    body: &[u8],
) -> std::result::Result<Message, octofhir_hl7v2::wire::Error> {
    Message::parse(
        [
            format!("MSH|^~\\&|S|S|T|T|20261111||{kind}|m1|T|{version}\r").as_bytes(),
            body,
        ]
        .concat(),
    )
}

#[test]
fn exact_resolution_never_guesses_structures_releases_or_profile_variants() -> Result {
    let catalog = catalog()?;
    for kind in [
        "ADT^A08",
        "ADT^A08^",
        "ADT^A08^ADT_A01",
        "ADT^A08^ADT\\X5F\\A01",
    ] {
        let source = message(kind, "2.5.1", b"PID|1\r")?;
        assert_eq!(catalog.resolve(&source)?.name(), "ADT_A01");
        assert_eq!(
            catalog
                .group(&source, MatchLimits::default())?
                .root()
                .name(),
            "ADT_A01"
        );
    }
    for (kind, version, error) in [
        ("ADT^A08^ADT_A08", "2.5.1", ErrorKind::UnknownStructure),
        ("ADT^A08^ADT_A03", "2.5.1", ErrorKind::StructureConflict),
        ("ADT^Z99", "2.5.1", ErrorKind::UnknownEvent),
        ("ADT^A08^ADT_A01", "2.5", ErrorKind::Version),
        ("ADT^A08^ADT_A01", "2.6", ErrorKind::Version),
        ("ADT^A08^ADT_A01", "2.5.1^USA", ErrorKind::VersionVariant),
        ("ADT^A08^\"\"", "2.5.1", ErrorKind::Header),
        ("ADT^A08^ADT_A01^extra", "2.5.1", ErrorKind::Header),
        ("ADT^A08~ADT^A01", "2.5.1", ErrorKind::Header),
        ("ADT^A08^ADT_A01", "\"\"", ErrorKind::Header),
        ("ADT^A08^ADT_A01", "2.5.1~2.5", ErrorKind::Header),
    ] {
        let source = message(kind, version, b"PID|1\r")?;
        let original = source.original().to_vec();
        let failure = catalog
            .resolve(&source)
            .err()
            .ok_or("bad envelope resolved")?;
        assert_eq!(failure.kind, error);
        assert!(failure.span.end <= original.len());
        assert_eq!(source.original(), original);
    }
    let source = message("ADT^A08", "2.5.1", b"")?;
    let custom = Message::parse(b"MSH*$%!?*S*S*T*T*20261111**ADT$A08$ADT_A01*m1*T*2.5.1\rPID*1\r")?;
    assert_eq!(catalog.resolve(&custom)?.name(), "ADT_A01");
    let mut edit = source.edit();
    edit.set_or_create(&[1, 21], Edit::Value(b"unregistered-profile".to_vec()))?;
    assert!(
        matches!(catalog.resolve(&edit.finish()?), Err(e) if e.kind == ErrorKind::UnsupportedProfile)
    );
    let extra = [
        Binding {
            message: "ADT",
            event: "A08",
            structure: "ADT_A01",
        },
        Binding {
            message: "ADT",
            event: "A08",
            structure: "ADT_A03",
        },
    ];
    let catalog = Catalog::compile("2.5.1", STRUCTURES, &extra, CompileLimits::default())?;
    assert!(matches!(catalog.resolve(&source), Err(e) if e.kind == ErrorKind::AmbiguousEvent));
    assert_eq!(
        catalog
            .resolve(&message("ADT^A08^ADT_A03", "2.5.1", b"")?)?
            .name(),
        "ADT_A03"
    );
    Ok(())
}

#[test]
fn deterministic_grouping_matches_the_explicit_bounded_language() -> Result {
    let pair = [
        Rule::segment("optional", *b"PID", Occurs::OPTIONAL),
        Rule::segment("required", *b"OBR", ONCE),
    ];
    let rules = [
        Rule::segment("msh", *b"MSH", ONCE),
        Rule::group(
            "pair",
            &pair,
            Occurs {
                min: 1,
                max: Some(2),
            },
        ),
        Rule::segment("tail", *b"OBX", ONCE),
    ];
    let definitions = [Structure {
        name: "TEST",
        rules: &rules,
    }];
    let bindings = [Binding {
        message: "ZZZ",
        event: "Z01",
        structure: "TEST",
    }];
    let catalog = Catalog::compile("TEST", &definitions, &bindings, CompileLimits::default())?;
    let accepted = [
        vec![1, 2],
        vec![0, 1, 2],
        vec![1, 1, 2],
        vec![0, 1, 1, 2],
        vec![1, 0, 1, 2],
        vec![0, 1, 0, 1, 2],
    ];
    let ids = [b"PID\r", b"OBR\r", b"OBX\r"];
    for length in 0..=5 {
        for mut word in 0..3usize.pow(length) {
            let mut sequence = Vec::new();
            let mut body = Vec::new();
            for _ in 0..length {
                let id = word % 3;
                sequence.push(id);
                body.extend_from_slice(ids[id]);
                word /= 3;
            }
            let source = message("ZZZ^Z01^TEST", "TEST", &body)?;
            assert_eq!(
                catalog.group(&source, MatchLimits::default()).is_ok(),
                accepted.contains(&sequence),
                "{sequence:?}"
            );
        }
    }
    Ok(())
}

#[test]
fn nested_groups_borrow_occurrences_without_losing_repetitions_nulls_or_z_segments() -> Result {
    let catalog = catalog()?;
    let source = message("ZZZ^Z01^SYNTHETIC_ORU", "2.5.1", b"PID|1\rOBR|a\rOBX|1|NM|code||0001.2300\rNTE|1||left\\F\\right\rNTE|2||\"\"\rOBX|2\rOBR|b\rOBX|3\rPID|2\rOBR|c\rOBX|4\rZZZ|\xff|a~~b\r")?;
    let view = catalog.group(&source, MatchLimits::default())?;
    assert_eq!(
        view.message().original().as_ptr(),
        source.original().as_ptr()
    );
    assert_eq!(view.root().segment_range(), 1..14);
    let root: Vec<_> = view.root().children().collect();
    assert_eq!(
        root.iter().map(|n| n.name()).collect::<Vec<_>>(),
        ["msh", "patient", "patient", "extension"]
    );
    assert_eq!(root[1].segment_range(), 2..10);
    assert_eq!(root[2].segment_range(), 10..13);
    let patient: Vec<_> = root[1].children().collect();
    assert_eq!(
        patient.iter().map(|n| n.name()).collect::<Vec<_>>(),
        ["pid", "order", "order"]
    );
    assert_eq!(patient[1].segment_range(), 3..8);
    let observations: Vec<_> = patient[1].children().skip(1).collect();
    assert_eq!(observations[0].segment_range(), 4..7);
    let notes: Vec<_> = observations[0].children().skip(1).collect();
    assert_eq!(
        notes
            .iter()
            .map(|n| n.segment().map(|s| s.occurrence()))
            .collect::<Vec<_>>(),
        [Some(5), Some(6)]
    );
    assert_eq!(
        notes[0]
            .segment()
            .ok_or("missing NTE")?
            .field(3)
            .decode()?
            .as_ref(),
        b"left|right"
    );
    assert_eq!(
        notes[1].segment().ok_or("missing NTE")?.field(3).presence(),
        Presence::Null
    );
    assert_eq!(
        root[3].segment().ok_or("missing ZZZ")?.raw(),
        source.segment(13).ok_or("missing source")?.raw()
    );
    assert_eq!(
        root[1]
            .segments()
            .map(|s| s.occurrence())
            .collect::<Vec<_>>(),
        (2..10).collect::<Vec<_>>()
    );
    assert!(source.segment(0).is_none());
    assert!(source.segment(usize::MAX).is_none());
    Ok(())
}

#[test]
fn grammar_rejects_ambiguity_nullable_groups_cycles_and_invalid_definitions_with_limits() -> Result
{
    let oversized = "X".repeat(65);
    let definitions = [Structure {
        name: &oversized,
        rules: SHAPE,
    }];
    assert!(
        matches!(Catalog::compile("TEST", &definitions, &[], CompileLimits::default()), Err(e) if e.kind == ErrorKind::Definition)
    );
    let rules = [
        Rule::segment("msh", *b"MSH", ONCE),
        Rule::segment(&oversized, *b"PID", ONCE),
    ];
    let definitions = [Structure {
        name: "TEST",
        rules: &rules,
    }];
    let error = Catalog::compile("TEST", &definitions, &[], CompileLimits::default())
        .err()
        .ok_or("oversized role accepted")?;
    assert_eq!(error.kind, ErrorKind::Definition);
    assert_eq!(error.rule, Some(2));
    let optional = [Rule::segment("optional", *b"PID", Occurs::OPTIONAL)];
    let cyclic: &[Rule<'_>] = &CYCLE;
    for rules in [
        vec![
            Rule::segment("msh", *b"MSH", ONCE),
            Rule::segment("a", *b"PID", MANY),
            Rule::segment("b", *b"PID", ONCE),
        ],
        vec![
            Rule::segment("msh", *b"MSH", ONCE),
            Rule::group("empty", &optional, ONCE),
        ],
        vec![
            Rule::segment("msh", *b"MSH", ONCE),
            Rule::group("cyclic", cyclic, ONCE),
        ],
        vec![
            Rule::segment("msh", *b"MSH", ONCE),
            Rule::segment(
                "a",
                *b"PID",
                Occurs {
                    min: 2,
                    max: Some(1),
                },
            ),
        ],
        vec![
            Rule::segment("msh", *b"MSH", ONCE),
            Rule::segment("same", *b"PID", ONCE),
            Rule::segment("same", *b"OBX", ONCE),
        ],
    ] {
        let definitions = [Structure {
            name: "TEST",
            rules: &rules,
        }];
        assert!(Catalog::compile("TEST", &definitions, &[], CompileLimits::default()).is_err());
    }
    for limits in [
        CompileLimits {
            max_rules: 2,
            ..Default::default()
        },
        CompileLimits {
            max_work: 1,
            ..Default::default()
        },
        CompileLimits {
            max_depth: 1,
            ..Default::default()
        },
    ] {
        assert!(
            matches!(Catalog::compile("2.5.1", STRUCTURES, BINDINGS, limits), Err(e) if e.kind == ErrorKind::Limit)
        );
    }
    let optional_tail = [
        Rule::segment("start", *b"PID", ONCE),
        Rule::segment("end", *b"OBX", Occurs::OPTIONAL),
    ];
    let rules = [
        Rule::segment("msh", *b"MSH", ONCE),
        Rule::group("patient", &optional_tail, MANY),
        Rule::segment("following", *b"OBX", ONCE),
    ];
    let definitions = [Structure {
        name: "TEST",
        rules: &rules,
    }];
    assert!(
        matches!(Catalog::compile("TEST", &definitions, &[], CompileLimits::default()), Err(e) if e.kind == ErrorKind::AmbiguousGrammar)
    );
    let bad = [Binding {
        message: "ADT",
        event: "A08",
        structure: "UNKNOWN",
    }];
    assert!(
        matches!(Catalog::compile("2.5.1", STRUCTURES, &bad, CompileLimits::default()), Err(e) if e.kind == ErrorKind::Definition)
    );
    Ok(())
}
static CYCLE: [Rule<'static>; 1] = [Rule {
    name: "cycle",
    occurs: ONCE,
    content: Content::Group(&CYCLE),
}];

#[test]
fn matching_refuses_unassigned_content_checks_budgets_and_keeps_same_ids_in_distinct_roles()
-> Result {
    let catalog = catalog()?;
    for (body, kind, segment) in [
        (b"PID\rZ99\rOBR\r".as_slice(), ErrorKind::MissingSegment, 3),
        (b"PID\rOBR\rZ99\r", ErrorKind::UnexpectedSegment, 4),
        (b"PID\r", ErrorKind::MissingSegment, 3),
    ] {
        let source = message("ZZZ^Z01", "2.5.1", body)?;
        let error = catalog
            .group(&source, MatchLimits::default())
            .err()
            .ok_or("unexpected segment ignored")?;
        assert_eq!(error.kind, kind);
        assert_eq!(error.path[0], segment);
        assert!(error.span.end <= source.original().len());
    }
    let source = message("ZZZ^Z01", "2.5.1", b"PID\rOBR\rOBX\r")?;
    for limits in [
        MatchLimits {
            max_steps: 1,
            ..Default::default()
        },
        MatchLimits {
            max_occurrences: 2,
            ..Default::default()
        },
    ] {
        assert!(matches!(catalog.group(&source, limits), Err(e) if e.kind == ErrorKind::Limit));
    }
    let rules = [
        Rule::segment("msh", *b"MSH", ONCE),
        Rule::segment("first", *b"OBX", ONCE),
        Rule::segment("second", *b"OBX", ONCE),
    ];
    let definitions = [Structure {
        name: "TEST",
        rules: &rules,
    }];
    let catalog = Catalog::compile("TEST", &definitions, &[], CompileLimits::default())?;
    // Direct structure access is intentionally unavailable; an explicit binding is required for resolution.
    assert!(matches!(catalog.resolve(&source), Err(e) if e.kind == ErrorKind::Version));
    let bindings = [Binding {
        message: "ZZZ",
        event: "Z01",
        structure: "TEST",
    }];
    let catalog = Catalog::compile("TEST", &definitions, &bindings, CompileLimits::default())?;
    let source = message("ZZZ^Z01", "TEST", b"OBX|first\rOBX|second\r")?;
    let view = catalog.group(&source, MatchLimits::default())?;
    let roles: Vec<_> = view.root().children().skip(1).collect();
    assert_eq!(
        roles.iter().map(|r| r.name()).collect::<Vec<_>>(),
        ["first", "second"]
    );
    assert_eq!(
        roles[0]
            .segment()
            .ok_or("missing first")?
            .field(1)
            .decode()?
            .as_ref(),
        b"first"
    );
    assert_eq!(
        roles[1]
            .segment()
            .ok_or("missing second")?
            .field(1)
            .decode()?
            .as_ref(),
        b"second"
    );
    Ok(())
}
