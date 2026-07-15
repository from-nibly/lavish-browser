use lavish_browser_protocol::*;

fn open_request() -> RequestEnvelope {
    RequestEnvelope::new(
        "req-7",
        Command::OpenUrl {
            project: ProjectMetadata {
                key: ProjectKey::Zellij {
                    session_name: "main".into(),
                    stable_tab_id: 4,
                },
                label: "browser".into(),
                raw_tab_name: None,
            },
            source_file: "/tmp/review.html".into(),
            url: "http://127.0.0.1:4387/session/key?noGate=1".into(),
        },
    )
}

#[test]
fn request_round_trips_with_version_and_id() {
    let request = open_request();
    let frame = encode_frame(&request).unwrap();
    assert_eq!(decode_frame::<RequestEnvelope>(&frame).unwrap(), request);
}

#[test]
fn protocol_version_is_explicit_and_rejected_when_unknown() {
    assert_eq!(validate_protocol_version(PROTOCOL_VERSION), Ok(()));
    assert_eq!(
        validate_protocol_version(99),
        Err(ProtocolError::UnsupportedVersion {
            actual: 99,
            expected: PROTOCOL_VERSION,
        })
    );
}

#[test]
fn response_and_read_only_inspection_round_trip() {
    let response = ResponseEnvelope {
        protocol_version: PROTOCOL_VERSION,
        request_id: "inspect-1".into(),
        status: ResponseStatus::Ok,
        error_code: None,
        message: None,
        state: Some(BrowserStateSnapshot {
            projects: vec![],
            selected_project: None,
            presentation_count: 0,
            process_id: 42,
            runtime_id: Some("test".into()),
        }),
    };
    let frame = encode_frame(&response).unwrap();
    assert_eq!(decode_frame::<ResponseEnvelope>(&frame).unwrap(), response);
}

#[test]
fn framing_rejects_oversized_truncated_malformed_and_multiple_messages() {
    assert_eq!(
        decode_frame::<RequestEnvelope>(&vec![b'x'; MAX_MESSAGE_BYTES + 1]),
        Err(ProtocolError::TooLarge)
    );
    assert_eq!(
        decode_frame::<RequestEnvelope>(br#"{"protocol_version":1}"#),
        Err(ProtocolError::MissingNewline)
    );
    assert!(matches!(
        decode_frame::<RequestEnvelope>(b"not-json\n"),
        Err(ProtocolError::InvalidJson(_))
    ));
    assert_eq!(
        decode_frame::<RequestEnvelope>(b"{}\n{}\n"),
        Err(ProtocolError::TrailingData)
    );
}

#[test]
fn accepts_exact_loopback_session_urls_and_queries() {
    for url in [
        "http://127.0.0.1:4387/session/key",
        "https://localhost/session/key?noGate=1",
        "http://[::1]:9000/session/key?x=session/other",
    ] {
        assert!(validate_session_url(url).is_ok(), "{url}");
    }
}

#[test]
fn rejects_credentials_deceptive_hosts_and_non_session_paths() {
    for url in [
        "http://user@localhost:4387/session/key",
        "http://localhost:4387@evil.example/session/key",
        "http://localhost.evil.example/session/key",
        "http://127.0.0.2/session/key",
        "http://[::2]/session/key",
        "http://example.com/?next=http://localhost/session/key",
        "http://localhost/not-session/key",
        "http://localhost/session/",
        "file://localhost/session/key",
        "not a url",
        "http://localhost:bad/session/key",
    ] {
        assert!(validate_session_url(url).is_err(), "accepted {url}");
    }
}
