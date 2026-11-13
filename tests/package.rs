#![cfg(feature = "schema-package")]
use octofhir_hl7v2::{
    schema::{
        Binding, CompileLimits, Content, MatchLimits, Occurs, Rule, Structure,
        fields::{
            Datatype, DatatypeKind as K, Element as E, Length, Primitive as P,
            SegmentDefinition as S, ValidationLimits,
        },
        package::{Definitions, Error, FORMAT_VERSION, Manifest, Package, Source},
    },
    wire::Message,
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
        name: "MSG",
        kind: K::Composite(&[
            E::required(1, "ID"),
            E::required(2, "ID"),
            E::required(3, "ID"),
        ]),
    },
    Datatype {
        name: "NM",
        kind: K::Primitive(P::Number),
    },
    Datatype {
        name: "ST",
        kind: K::Primitive(P::Utf8),
    },
    Datatype {
        name: "TS",
        kind: K::Timestamp,
    },
];
const VALUE: &[E<'_>] = &[
    E::required(1, "ID"),
    E {
        nullable: true,
        length: Some(Length::WireBytes(64)),
        ..E::by_field(2, 1, &["NM", "ST", "TS"], Occurs::ONCE)
    },
];
const PROFILES: &[S<'_>] = &[
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
        name: "Note",
        id: *b"NTE",
        fields: &[E::optional(1, "ID"), E::optional(3, "ST")],
    },
    S {
        name: "Value",
        id: *b"ZZZ",
        fields: VALUE,
    },
];
const RECORD: &[Rule<'_>] = &[
    Rule::typed_segment("value", *b"ZZZ", "Value", Occurs::ONCE),
    Rule::typed_segment("notes", *b"NTE", "Note", Occurs::MANY),
];
const RULES: &[Rule<'_>] = &[
    Rule::typed_segment("msh", *b"MSH", "Header", Occurs::ONCE),
    Rule::group("record", RECORD, Occurs::MANY),
];
const STRUCTURES: &[Structure<'_>] = &[Structure {
    name: "SYNTHETIC",
    rules: RULES,
}];
const BINDINGS: &[Binding<'_>] = &[
    Binding {
        message: "ZZZ",
        event: "Z01",
        structure: "SYNTHETIC",
    },
    Binding {
        message: "ZZZ",
        event: "Z02",
        structure: "SYNTHETIC",
    },
];
// Deliberately declared test-only evidence, not verified standard artifacts.
const SOURCES: &[Source<'_>] = &[
    Source {
        name: "a",
        location: "urn:test:a",
        revision: "1",
        sha256: [1; 32],
        license: "MIT OR Apache-2.0",
        notice: "Original synthetic fixture",
    },
    Source {
        name: "b",
        location: "urn:test:b",
        revision: "1",
        sha256: [2; 32],
        license: "MIT OR Apache-2.0",
        notice: "Original synthetic fixture",
    },
];
fn manifest() -> Manifest<'static> {
    Manifest {
        name: "synthetic",
        release: "TEST",
        revision: "1",
        producer: "test-author@1",
        scope: "Synthetic groups, fields, lexical types and event bindings",
        limitations: "No standard catalog, conditional rules or terminology",
        sources: SOURCES,
    }
}
fn definitions() -> Definitions<'static> {
    Definitions {
        structures: STRUCTURES,
        bindings: BINDINGS,
        datatypes: TYPES,
        segments: PROFILES,
    }
}
fn compile<'a>(
    manifest: Manifest<'a>,
    definitions: Definitions<'a>,
) -> std::result::Result<Package<'a>, Error> {
    Package::compile(manifest, definitions, CompileLimits::default())
}

#[test]
fn deterministic_identity_survives_table_order_and_old_packages_remain_usable() -> Result {
    let old = compile(manifest(), definitions())?;
    let digest = old.digest();
    // Independently encoded from the documented V1 contract with Python hashlib/struct.
    assert_eq!(
        digest,
        [
            0x01, 0x61, 0x08, 0x1c, 0x3e, 0xb1, 0x55, 0x36, 0xd2, 0x63, 0xba, 0x1b, 0x1f, 0x35,
            0x77, 0x25, 0xb0, 0xef, 0xc4, 0x3b, 0x7e, 0x2d, 0x02, 0xa5, 0x8b, 0x54, 0x1d, 0xda,
            0x1e, 0x66, 0x9c, 0xd1
        ]
    );
    assert_eq!(FORMAT_VERSION, 3);
    assert_eq!(old.format_version(), 1);
    assert_eq!(old.manifest().release, old.catalog().version());
    let mut types: Vec<_> = TYPES
        .iter()
        .rev()
        .map(|t| Datatype {
            name: t.name,
            kind: t.kind,
        })
        .collect();
    let components = [
        E::required(3, "ID"),
        E::required(2, "ID"),
        E::required(1, "ID"),
    ];
    for datatype in &mut types {
        if datatype.name == "MSG" {
            datatype.kind = K::Composite(&components);
        }
    }
    let sources: Vec<_> = SOURCES.iter().rev().copied().collect();
    let bindings: Vec<_> = BINDINGS
        .iter()
        .rev()
        .map(|b| Binding {
            message: b.message,
            event: b.event,
            structure: b.structure,
        })
        .collect();
    let header: Vec<_> = PROFILES[0].fields.iter().rev().copied().collect();
    let fields = [
        E {
            datatype: octofhir_hl7v2::schema::fields::TypeRef::ByField {
                selector: 1,
                allowed: &["TS", "ST", "NM"],
            },
            ..VALUE[1]
        },
        VALUE[0],
    ];
    let profiles = [
        S {
            name: "Value",
            id: *b"ZZZ",
            fields: &fields,
        },
        S {
            name: "Note",
            id: *b"NTE",
            fields: PROFILES[1].fields,
        },
        S {
            name: "Header",
            id: *b"MSH",
            fields: &header,
        },
    ];
    let reordered = Package::compile(
        Manifest {
            sources: &sources,
            ..manifest()
        },
        Definitions {
            datatypes: &types,
            bindings: &bindings,
            segments: &profiles,
            ..definitions()
        },
        CompileLimits {
            max_work: 100_000,
            ..Default::default()
        },
    )?
    .require_digest(digest)?;
    assert_eq!(reordered.digest(), digest);
    assert_eq!(reordered.encode(FILE.len())?, FILE);
    let narrower = [
        VALUE[0],
        E {
            length: Some(Length::WireBytes(2)),
            ..VALUE[1]
        },
    ];
    let profiles = [
        S {
            fields: &narrower,
            ..PROFILES[2]
        },
        S {
            name: "Header",
            id: *b"MSH",
            fields: PROFILES[0].fields,
        },
        S {
            name: "Note",
            id: *b"NTE",
            fields: PROFILES[1].fields,
        },
    ];
    let new = compile(
        Manifest {
            revision: "2",
            ..manifest()
        },
        Definitions {
            segments: &profiles,
            ..definitions()
        },
    )?;
    assert_ne!(new.digest(), digest);
    let message = Message::parse(
        b"MSH|^~\\&|||||||ZZZ^Z01^SYNTHETIC|||TEST\rZZZ|NM|0001.2300\rNTE|1||note\r",
    )?;
    for (package, valid) in [(&old, true), (&reordered, true), (&new, false)] {
        let view = package.catalog().group(&message, MatchLimits::default())?;
        assert_eq!(
            package
                .fields()
                .validate(&view, ValidationLimits::default())
                .is_valid(),
            valid
        );
        assert_eq!(
            view.message().original().as_ptr(),
            message.original().as_ptr()
        );
    }
    assert!(matches!(
        new.require_digest(digest),
        Err(Error::DigestMismatch)
    ));
    assert_eq!(old.digest(), digest);
    Ok(())
}

#[test]
fn metadata_and_schema_semantics_are_content_bound() -> Result {
    let digest = compile(manifest(), definitions())?.digest();
    for metadata in [
        Manifest {
            name: "other",
            ..manifest()
        },
        Manifest {
            release: "TEST2",
            ..manifest()
        },
        Manifest {
            revision: "2",
            ..manifest()
        },
        Manifest {
            producer: "test-author@2",
            ..manifest()
        },
        Manifest {
            scope: "Other coverage",
            ..manifest()
        },
        Manifest {
            limitations: "Other gaps",
            ..manifest()
        },
    ] {
        assert!(matches!(
            compile(metadata, definitions())?.require_digest(digest),
            Err(Error::DigestMismatch)
        ));
    }
    for source in [
        Source {
            name: "c",
            ..SOURCES[0]
        },
        Source {
            location: "urn:other",
            ..SOURCES[0]
        },
        Source {
            revision: "2",
            ..SOURCES[0]
        },
        Source {
            sha256: [3; 32],
            ..SOURCES[0]
        },
        Source {
            license: "MIT",
            ..SOURCES[0]
        },
        Source {
            notice: "Other attribution",
            ..SOURCES[0]
        },
    ] {
        let sources = [source, SOURCES[1]];
        assert_ne!(
            compile(
                Manifest {
                    sources: &sources,
                    ..manifest()
                },
                definitions()
            )?
            .digest(),
            digest
        );
    }
    for element in [
        E {
            position: 3,
            ..VALUE[1]
        },
        E {
            nullable: false,
            ..VALUE[1]
        },
        E {
            occurs: Occurs::MANY,
            ..VALUE[1]
        },
        E {
            length: None,
            ..VALUE[1]
        },
        E {
            length: Some(Length::WireBytes(63)),
            ..VALUE[1]
        },
        E::optional(2, "NM"),
        E::by_field(2, 1, &["NM"], Occurs::ONCE),
    ] {
        let fields = [VALUE[0], element];
        let profiles = [
            S {
                fields: &fields,
                ..PROFILES[2]
            },
            S {
                name: "Header",
                id: *b"MSH",
                fields: PROFILES[0].fields,
            },
            S {
                name: "Note",
                id: *b"NTE",
                fields: PROFILES[1].fields,
            },
        ];
        assert_ne!(
            compile(
                manifest(),
                Definitions {
                    segments: &profiles,
                    ..definitions()
                }
            )?
            .digest(),
            digest
        );
    }
    for kind in [
        K::Primitive(P::Date),
        K::Primitive(P::DateTime),
        K::Timestamp,
        K::Composite(&[E::required(1, "ID")]),
    ] {
        let mut types: Vec<_> = TYPES
            .iter()
            .map(|t| Datatype {
                name: t.name,
                kind: t.kind,
            })
            .collect();
        types[3].kind = kind;
        assert_ne!(
            compile(
                manifest(),
                Definitions {
                    datatypes: &types,
                    ..definitions()
                }
            )?
            .digest(),
            digest
        );
    }
    let changed_bindings = [
        Binding {
            event: "Z03",
            ..BINDINGS[0]
        },
        Binding {
            message: "ZZZ",
            event: "Z02",
            structure: "SYNTHETIC",
        },
    ];
    assert_ne!(
        compile(
            manifest(),
            Definitions {
                bindings: &changed_bindings,
                ..definitions()
            }
        )?
        .digest(),
        digest
    );
    for record in [
        [
            Rule {
                name: "other_role",
                ..RECORD[0]
            },
            RECORD[1],
        ],
        [
            RECORD[0],
            Rule {
                occurs: Occurs::OPTIONAL,
                ..RECORD[1]
            },
        ],
    ] {
        let rules = [RULES[0], Rule::group("record", &record, Occurs::MANY)];
        let structures = [Structure {
            name: "SYNTHETIC",
            rules: &rules,
        }];
        assert_ne!(
            compile(
                manifest(),
                Definitions {
                    structures: &structures,
                    ..definitions()
                }
            )?
            .digest(),
            digest
        );
    }
    Ok(())
}

#[test]
fn invalid_evidence_and_incompatible_definitions_fail_before_activation() -> Result {
    for metadata in [
        Manifest {
            name: "",
            ..manifest()
        },
        Manifest {
            scope: " \t",
            ..manifest()
        },
        Manifest {
            limitations: "bad\0text",
            ..manifest()
        },
        Manifest {
            sources: &[],
            ..manifest()
        },
    ] {
        assert!(matches!(
            compile(metadata, definitions()),
            Err(Error::Manifest)
        ));
    }
    let duplicates = [SOURCES[0]; 2];
    assert!(matches!(
        compile(
            Manifest {
                sources: &duplicates,
                ..manifest()
            },
            definitions()
        ),
        Err(Error::Manifest)
    ));
    let too_many = [SOURCES[0]; 33];
    assert!(matches!(
        compile(
            Manifest {
                sources: &too_many,
                ..manifest()
            },
            definitions()
        ),
        Err(Error::Limit)
    ));
    let large = "x".repeat(4097);
    assert!(matches!(
        compile(
            Manifest {
                scope: &large,
                ..manifest()
            },
            definitions()
        ),
        Err(Error::Limit)
    ));
    let long = "x".repeat(4096);
    let names: Vec<_> = (0..32).map(|n| n.to_string()).collect();
    let sources: Vec<_> = names
        .iter()
        .map(|name| Source {
            name,
            location: &long,
            revision: &long,
            license: &long,
            notice: &long,
            ..SOURCES[0]
        })
        .collect();
    assert!(matches!(
        compile(
            Manifest {
                sources: &sources,
                ..manifest()
            },
            definitions()
        ),
        Err(Error::Limit)
    ));
    let untyped = [Rule {
        content: Content::Segment(*b"MSH"),
        ..RULES[0]
    }];
    let structures = [Structure {
        name: "SYNTHETIC",
        rules: &untyped,
    }];
    assert!(matches!(
        compile(
            manifest(),
            Definitions {
                structures: &structures,
                ..definitions()
            }
        ),
        Err(Error::Schema(_))
    ));
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
        assert!(matches!(
            Package::compile(manifest(), definitions(), limits),
            Err(Error::Schema(_))
        ));
    }
    Ok(())
}

const FILE: &[u8] = include_bytes!("fixtures/native-package-v1.bin");
const PIN: [u8; 32] = [
    0x01, 0x61, 0x08, 0x1c, 0x3e, 0xb1, 0x55, 0x36, 0xd2, 0x63, 0xba, 0x1b, 0x1f, 0x35, 0x77, 0x25,
    0xb0, 0xef, 0xc4, 0x3b, 0x7e, 0x2d, 0x02, 0xa5, 0x8b, 0x54, 0x1d, 0xda, 0x1e, 0x66, 0x9c, 0xd1,
];

#[test]
fn frozen_file_restores_historical_pin_and_owns_reusable_plans() -> Result {
    use octofhir_hl7v2::schema::package::{LoadLimits, OwnedPackage};
    let native = compile(manifest(), definitions())?;
    assert_eq!(native.encode(FILE.len())?, FILE);
    let loaded = {
        let bytes = FILE.to_vec();
        OwnedPackage::read_from(std::io::Cursor::new(bytes), PIN, LoadLimits::default())?
    };
    assert_eq!(loaded.digest(), PIN);
    assert_eq!(loaded.manifest().scope, manifest().scope);
    assert_eq!(loaded.manifest().sources[1].sha256, SOURCES[1].sha256);
    let mut exported = Vec::new();
    loaded.write_to(&mut exported, FILE.len())?;
    assert_eq!(exported, FILE);
    // A moved owned package remains usable after all loader buffers have been dropped.
    let loaded = std::thread::spawn(move || loaded)
        .join()
        .map_err(|_| "loader thread failed")?;
    for value in ["0001.2300", "\"\"", "invalid"] {
        let message = Message::parse(
            format!("MSH|^~\\&|||||||ZZZ^Z01^SYNTHETIC|||TEST\rZZZ|NM|{value}\rNTE|1||note\r")
                .as_bytes(),
        )?;
        let old = native.catalog().group(&message, MatchLimits::default())?;
        let restored = loaded.catalog().group(&message, MatchLimits::default())?;
        assert_eq!(old.occurrence_count(), restored.occurrence_count());
        let a = native.fields().validate(&old, ValidationLimits::default());
        let b = loaded
            .fields()
            .validate(&restored, ValidationLimits::default());
        assert_eq!(a.is_valid(), value != "invalid");
        assert_eq!(a.issues, b.issues);
        assert_eq!(
            (a.complete, a.omitted, a.steps, a.bytes),
            (b.complete, b.omitted, b.steps, b.bytes)
        );
        assert_eq!(
            restored.message().original().as_ptr(),
            message.original().as_ptr()
        );
        assert_eq!(
            restored.segments().nth(1).and_then(|s| s.profile()),
            Some("Value")
        );
    }
    Ok(())
}

#[test]
fn files_fail_closed_on_corruption_unknown_constructs_references_and_bounds() -> Result {
    use octofhir_hl7v2::schema::package::{FileError, LoadLimits, OwnedPackage};
    let limits = LoadLimits::default();
    for end in 0..FILE.len() {
        assert!(
            OwnedPackage::decode(&FILE[..end], PIN, limits).is_err(),
            "prefix {end}"
        );
    }
    for offset in 0..FILE.len() {
        let mut changed = FILE.to_vec();
        changed[offset] ^= 0x80;
        assert!(
            OwnedPackage::decode(&changed, PIN, limits).is_err(),
            "changed byte {offset}"
        );
    }
    let mut trailing = FILE.to_vec();
    trailing.push(0);
    assert!(matches!(
        OwnedPackage::decode(&trailing, PIN, limits),
        Err(FileError::Format { .. })
    ));
    for offset in [8, 16] {
        let mut unknown = FILE.to_vec();
        unknown[offset..offset + 8].copy_from_slice(&2_u64.to_be_bytes());
        assert!(matches!(
            OwnedPackage::decode(&unknown, PIN, limits),
            Err(FileError::UnsupportedVersion)
        ));
    }
    let mut pin = PIN;
    pin[0] ^= 1;
    assert!(matches!(
        OwnedPackage::decode(FILE, pin, limits),
        Err(FileError::Package(Error::DigestMismatch))
    ));
    // An unchanged embedded pin does not authorize changed metadata or schema contents.
    let mut changed = FILE.to_vec();
    changed[64] = b'S';
    assert!(matches!(
        OwnedPackage::decode(&changed, PIN, limits),
        Err(FileError::Package(Error::DigestMismatch))
    ));
    let replace_last =
        |from: &[u8], to: &[u8]| -> std::result::Result<Vec<u8>, Box<dyn std::error::Error>> {
            let mut changed = FILE.to_vec();
            let at = changed
                .windows(from.len())
                .rposition(|w| w == from)
                .ok_or("missing fixture token")?;
            changed[at..at + from.len()].copy_from_slice(to);
            Ok(changed)
        };
    for changed in [
        replace_last(b"NM", b"NO")?, // Unknown datatype in a dynamic allowlist.
        replace_last(b"SYNTHETIC", b"UNDEFINED")?, // Unknown binding target.
        // A grammar profile and its definition must still agree.
        {
            let mut b = FILE.to_vec();
            let at = b
                .windows(5)
                .position(|s| s == b"Value")
                .ok_or("profile token")?;
            b[at..at + 5].copy_from_slice(b"Other");
            b
        },
    ] {
        assert!(matches!(
            OwnedPackage::decode(&changed, PIN, limits),
            Err(FileError::Package(Error::Schema(_)))
        ));
    }
    for bounded in [
        LoadLimits {
            max_bytes: FILE.len() - 1,
            ..limits
        },
        LoadLimits {
            max_items: 1,
            ..limits
        },
        LoadLimits {
            max_text_bytes: 3,
            ..limits
        },
        LoadLimits {
            compile: CompileLimits {
                max_depth: 1,
                ..Default::default()
            },
            ..limits
        },
        LoadLimits {
            compile: CompileLimits {
                max_work: 1,
                ..Default::default()
            },
            ..limits
        },
    ] {
        assert!(OwnedPackage::decode(FILE, PIN, bounded).is_err());
    }
    let package = compile(manifest(), definitions())?;
    let mut output = Vec::new();
    assert!(matches!(
        package.write_to(&mut output, FILE.len() - 1),
        Err(FileError::Limit)
    ));
    assert!(output.is_empty());
    // The read boundary is enforced even for an endless input stream.
    struct Endless {
        read: usize,
    }
    impl std::io::Read for Endless {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            b.fill(0);
            self.read += b.len();
            Ok(b.len())
        }
    }
    let mut endless = Endless { read: 0 };
    assert!(matches!(
        OwnedPackage::read_from(&mut endless, PIN, limits),
        Err(FileError::Limit)
    ));
    assert_eq!(endless.read, limits.max_bytes + 1);
    Ok(())
}

#[test]
fn loaded_plans_preserve_scalar_kinds_length_units_and_nested_constraints() -> Result {
    use octofhir_hl7v2::schema::package::{LoadLimits, OwnedPackage};
    let types: Vec<_> = TYPES
        .iter()
        .map(|t| Datatype {
            name: t.name,
            kind: t.kind,
        })
        .chain([
            Datatype {
                name: "DT",
                kind: K::Primitive(P::Date),
            },
            Datatype {
                name: "DTM",
                kind: K::Primitive(P::DateTime),
            },
        ])
        .collect();
    let fields = [
        VALUE[0],
        E {
            nullable: true,
            length: VALUE[1].length,
            ..E::by_field(
                2,
                1,
                &["B", "ID", "ST", "NM", "DT", "DTM", "TS", "MSG"],
                Occurs::ONCE,
            )
        },
    ];
    for length in [Length::ScalarBytes(2), Length::Utf8Characters(2)] {
        let note = [
            E::optional(1, "ID"),
            E {
                length: Some(length),
                ..E::optional(3, "ST")
            },
        ];
        let segments = [
            S {
                fields: &fields,
                ..PROFILES[2]
            },
            S {
                name: "Header",
                id: *b"MSH",
                fields: PROFILES[0].fields,
            },
            S {
                name: "Note",
                id: *b"NTE",
                fields: &note,
            },
        ];
        let native = compile(
            manifest(),
            Definitions {
                datatypes: &types,
                segments: &segments,
                ..definitions()
            },
        )?;
        let loaded = OwnedPackage::decode(
            &native.encode(16_384)?,
            native.digest(),
            LoadLimits::default(),
        )?;
        for (datatype, value, valid) in [
            ("B", "raw", true),
            ("ID", "ascii", true),
            ("ST", "é", true),
            ("NM", "0001.2300", true),
            ("DT", "202402", true),
            ("DT", "20230229", false),
            ("DTM", "20240229235959.0100-0000", true),
            ("DTM", "2024023012", false),
            ("TS", "202402291234^L", true),
            ("TS", "2024^S", false),
            ("MSG", "ZZZ^Z01^SYNTHETIC", true),
            ("MSG", "ZZZ", false),
        ] {
            let message = Message::parse(
                format!(
                    "MSH|^~\\&|||||||ZZZ^Z01^SYNTHETIC|||TEST\rZZZ|{datatype}|{value}\rNTE|1||éé\r"
                )
                .as_bytes(),
            )?;
            let a = native.fields().validate(
                &native.catalog().group(&message, MatchLimits::default())?,
                ValidationLimits::default(),
            );
            let b = loaded.fields().validate(
                &loaded.catalog().group(&message, MatchLimits::default())?,
                ValidationLimits::default(),
            );
            assert_eq!(a.issues, b.issues);
            assert_eq!(
                b.is_valid(),
                valid && matches!(length, Length::Utf8Characters(_))
            );
        }
    }
    Ok(())
}
