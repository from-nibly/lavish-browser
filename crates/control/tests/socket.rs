use lavish_browser_control::{BrowserControl, ControlError, SocketClient, SocketServer};
use lavish_browser_protocol::{
    Command, MAX_MESSAGE_BYTES, PROTOCOL_VERSION, RequestEnvelope, ResponseEnvelope, ResponseStatus,
};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir()
        .join(format!(
            "lavish-control-{}-{}-{name}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
        .join("control.sock")
}

fn ok(request: RequestEnvelope) -> ResponseEnvelope {
    ResponseEnvelope {
        protocol_version: PROTOCOL_VERSION,
        request_id: request.request_id,
        status: ResponseStatus::Ok,
        error_code: None,
        message: None,
        state: None,
    }
}

#[test]
fn round_trips_acknowledgement_and_sets_permissions() {
    let path = temp_path("roundtrip");
    let server = SocketServer::bind(&path, ok).unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let worker = thread::spawn(move || server.accept().unwrap().join().unwrap());
    let response = SocketClient::new(&path)
        .request(&RequestEnvelope::new("request-7", Command::Ping))
        .unwrap();
    assert_eq!(response.request_id, "request-7");
    worker.join().unwrap();
}

#[test]
fn serves_concurrent_clients() {
    let path = temp_path("concurrent");
    let server = Arc::new(SocketServer::bind(&path, ok).unwrap());
    let accepting = {
        let server = Arc::clone(&server);
        thread::spawn(move || {
            let workers = (0..8).map(|_| server.accept().unwrap()).collect::<Vec<_>>();
            for worker in workers {
                worker.join().unwrap();
            }
        })
    };
    let clients = (0..8)
        .map(|id| {
            let path = path.clone();
            thread::spawn(move || {
                let request_id = format!("r{id}");
                SocketClient::new(path)
                    .request(&RequestEnvelope::new(&request_id, Command::Ping))
                    .unwrap()
            })
        })
        .collect::<Vec<_>>();
    for (id, client) in clients.into_iter().enumerate() {
        assert_eq!(client.join().unwrap().request_id, format!("r{id}"));
    }
    accepting.join().unwrap();
}

#[test]
fn rejects_malformed_truncated_oversized_and_wrong_version_requests() {
    let path = temp_path("malformed");
    let server = SocketServer::bind(&path, ok).unwrap();
    for payload in [
        b"not-json\n".to_vec(),
        br#"{"protocol_version":1"#.to_vec(),
        vec![b'x'; MAX_MESSAGE_BYTES + 1],
        br#"{"protocol_version":99,"request_id":"old","command":{"type":"ping"}}
"#
        .to_vec(),
    ] {
        // Connect before accepting by performing each exchange on a worker.
        let path = path.clone();
        let exchange = thread::spawn(move || {
            let mut stream = UnixStream::connect(path).unwrap();
            stream.write_all(&payload).unwrap();
            if !payload.ends_with(b"\n") {
                stream.shutdown(std::net::Shutdown::Write).unwrap();
            }
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            response
        });
        server.accept().unwrap().join().unwrap();
        assert!(exchange.join().unwrap().contains("\"status\":\"error\""));
    }
}

#[test]
fn bounds_diagnostic_responses_and_preserves_acknowledgement_id() {
    let path = temp_path("bounded-response");
    let server = SocketServer::bind(&path, |_request| ResponseEnvelope {
        protocol_version: 999,
        request_id: "wrong".into(),
        status: ResponseStatus::Ok,
        error_code: None,
        message: Some("x".repeat(MAX_MESSAGE_BYTES)),
        state: None,
    })
    .unwrap();
    let worker = thread::spawn(move || server.accept().unwrap().join().unwrap());
    let error = SocketClient::new(&path)
        .request(&RequestEnvelope::new("inspect-1", Command::InspectState))
        .unwrap_err();
    assert!(matches!(error, ControlError::Rejected(message) if message.contains("byte limit")));
    worker.join().unwrap();
}

#[test]
fn recovers_stale_socket_but_not_live_or_regular_files() {
    let path = temp_path("stale");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let stale = std::os::unix::net::UnixListener::bind(&path).unwrap();
    drop(stale);
    let server = SocketServer::bind(&path, ok).unwrap();
    assert!(matches!(
        SocketServer::bind(&path, ok),
        Err(ControlError::AlreadyRunning(_))
    ));
    drop(server);
    fs::write(&path, b"do not remove").unwrap();
    assert!(matches!(
        SocketServer::bind(&path, ok),
        Err(ControlError::UnsafePath { .. })
    ));
    assert_eq!(fs::read(&path).unwrap(), b"do not remove");
}
