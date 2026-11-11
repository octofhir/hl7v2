use octofhir_hl7v2::{
    ack::{self, Code, Condition, Issue, Mode, Policy, Reply, Severity},
    wire::{Edit, Message, ParseOptions},
};
type Result = std::result::Result<(), Box<dyn std::error::Error>>;

fn request(
    accept: &str,
    application: &str,
) -> std::result::Result<Message, octofhir_hl7v2::wire::Error> {
    Message::parse(format!(
        "MSH|^~\\&|SOURCE|S|TARGET|T|20261110120000||ADT^A01^ADT_A01|m\\F\\1|T|2.5.1|||{accept}|{application}\rPID|1\r"
    ))
}
fn reply(code: Code) -> Reply<'static> {
    Reply {
        code,
        control_id: b"ack-1",
        timestamp: b"20261110120000",
        issue: None,
    }
}

#[test]
fn original_and_enhanced_ack_envelopes_correlate_and_retain_errors() -> Result {
    for (mode, accept, application, codes) in [
        (Mode::Original, "", "", [Code::AA, Code::AE, Code::AR]),
        (
            Mode::EnhancedAcceptOnly,
            "AL",
            "NE",
            [Code::CA, Code::CE, Code::CR],
        ),
    ] {
        let request = request(accept, application)?;
        for code in codes {
            let response = Message::parse(ack::build(
                &request,
                Reply {
                    issue: Some(Issue {
                        code: 207,
                        severity: Severity::Error,
                        diagnostic: b"issue|^~\\&\r\n",
                    }),
                    ..reply(code)
                },
                4096,
            )?)?;
            assert_eq!(response.value(&[1, 3]).raw(), request.value(&[1, 5]).raw());
            assert_eq!(response.value(&[1, 5]).raw(), request.value(&[1, 3]).raw());
            assert_eq!(response.value(&[1, 10]).decode()?.as_ref(), b"ack-1");
            assert_eq!(response.value(&[2, 2]).raw(), request.value(&[1, 10]).raw());
            assert_eq!(response.value(&[3, 3, 1, 3]).decode()?.as_ref(), b"HL70357");
            assert_eq!(
                response.value(&[3, 7]).decode()?.as_ref(),
                b"issue|^~\\&\r\n"
            );
            assert_eq!(ack::correlate(&request, response, mode)?.code, code);
        }
    }
    Ok(())
}

#[test]
fn acknowledgement_conditions_are_independent_and_never_switch_modes() -> Result {
    let conditions = [
        ("AL", 0b111111),
        ("NE", 0),
        ("SU", 0b001001),
        ("ER", 0b110110),
    ];
    let codes = [Code::AA, Code::AE, Code::AR, Code::CA, Code::CE, Code::CR];
    for (accept, accept_mask) in conditions {
        for (application, application_mask) in conditions {
            let request = request(accept, application)?;
            let policy = Policy::from_message(&request)?;
            for (index, code) in codes.iter().copied().enumerate() {
                let mask = if index < 3 {
                    application_mask
                } else {
                    accept_mask
                };
                assert_eq!(policy.requests(code), mask & (1 << index) != 0);
            }
        }
    }
    for request in [request("AL", "")?, request("", "AL")?, request("\"\"", "")?] {
        assert!(Policy::from_message(&request).is_err());
    }
    assert_eq!(Policy::from_message(&request("", "")?)?, Policy::Original);
    assert_eq!(
        Policy::from_message(&request("AL", "NE")?)?,
        Policy::Enhanced {
            accept: Condition::Always,
            application: Condition::Never
        }
    );
    assert!(ack::validate_request(&request("ER", "NE")?, Mode::EnhancedAcceptOnly).is_err());
    assert!(ack::build(&request("AL", "NE")?, reply(Code::AA), 4096).is_err());
    Ok(())
}

#[test]
fn wrong_identity_phase_and_envelope_are_not_valid_acknowledgements() -> Result {
    let request = request("AL", "NE")?;
    let response = Message::parse(ack::build(&request, reply(Code::CA), 4096)?)?;
    for (path, value) in [
        (&[2, 2][..], b"unrelated".as_slice()),
        (&[2, 1], b"AA"),
        (&[1, 9, 1, 2], b"A03"),
        (&[1, 12], b"2.3"),
        (&[1, 9, 1, 3], b"OTHER"),
    ] {
        let mut edit = response.edit();
        edit.set(path, Edit::Value(value.to_vec()))?;
        assert!(ack::correlate(&request, edit.finish()?, Mode::EnhancedAcceptOnly).is_err());
    }
    let duplicate = Message::parse([response.original(), b"MSA|CA|m\\F\\1\r"].concat())?;
    assert!(ack::correlate(&request, duplicate, Mode::EnhancedAcceptOnly).is_err());
    assert!(ack::build(&response, reply(Code::AA), 4096).is_err());
    assert!(ack::build(&request, reply(Code::CA), 8).is_err());
    for identity in [
        b"TEST\\".as_slice(),
        b"TEST\\X1\\",
        b"TEST\\Zlocal\\",
        b"TEST\\XFF\\",
        b"TEST\\X0D\\",
    ] {
        let bytes = [
            b"MSH|^~\\&|S|S|T|".as_slice(),
            identity,
            b"|20261111||ADT^A01^ADT_A01|m1|T|2.5.1\r",
        ]
        .concat();
        let malformed = Message::parse_with_options(
            bytes,
            ParseOptions {
                max_diagnostics: 0,
                ..Default::default()
            },
        )?;
        assert!(malformed.diagnostics().is_empty());
        assert!(ack::validate_request(&malformed, Mode::Original).is_err());
        assert!(ack::build(&malformed, reply(Code::AA), 4096).is_err());
    }
    for timestamp in [
        b"20240229235959.1234-0000".as_slice(),
        b"20261110120000+0300",
    ] {
        let response = Message::parse(ack::build(
            &request,
            Reply {
                timestamp,
                ..reply(Code::CA)
            },
            4096,
        )?)?;
        assert_eq!(response.value(&[1, 7]).decode()?.as_ref(), timestamp);
    }
    for timestamp in [
        b"20260229120000".as_slice(),
        b"20261110240000",
        b"20261110120000.",
        b"20261110120000.12345",
        b"20261110120000+2460",
        b"20261110",
    ] {
        assert!(
            ack::build(
                &request,
                Reply {
                    timestamp,
                    ..reply(Code::CA)
                },
                4096
            )
            .is_err()
        );
    }
    Ok(())
}

#[test]
fn custom_delimiters_and_declared_utf8_use_the_existing_scalar_encoder() -> Result {
    let request =
        Message::parse(b"MSH*$%!?*SOURCE*S*TARGET*T*20261110**ADT$A01$ADT_A01*m!F!1*T*2.5.1\r")?;
    let response = Message::parse(ack::build(&request, reply(Code::AA), 4096)?)?;
    assert_eq!(response.delimiters(), request.delimiters());
    assert_eq!(response.value(&[2, 2]).raw(), Some(b"m!F!1".as_slice()));
    assert_eq!(
        ack::correlate(&request, response, Mode::Original)?.code,
        Code::AA
    );
    let request = Message::parse(
        b"MSH|^~\\&|\\XCEBB\\^id^ISO|S|T|T|20261110||ADT^A01^ADT_A01|m1|T|2.5.1||||||UNICODE UTF-8\r",
    )?;
    let response = Message::parse(ack::build(
        &request,
        Reply {
            issue: Some(Issue {
                code: 101,
                severity: Severity::Error,
                diagnostic: "Required: имя".as_bytes(),
            }),
            ..reply(Code::AE)
        },
        4096,
    )?)?;
    assert_eq!(
        response.value(&[3, 7]).decode()?.as_ref(),
        "Required: имя".as_bytes()
    );
    assert_eq!(response.value(&[1, 5]).raw(), request.value(&[1, 3]).raw());
    ack::correlate(&request, response, Mode::Original)?;
    Ok(())
}
