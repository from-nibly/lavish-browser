use lavish_browser_control::SocketServer;
use lavish_browser_protocol::{Command, PROTOCOL_VERSION, ResponseEnvelope, ResponseStatus};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::Command as ProcessCommand;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn receive(stream: &mut UnixStream) -> Value {
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

fn send(stream: &mut UnixStream, value: Value) {
    writeln!(stream, "{value}").unwrap();
}

#[test]
fn startup_and_rename_refresh_labels_without_selecting_the_renamed_tab() {
    let dir = std::env::temp_dir().join(format!(
        "lavish-rename-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let herdr_path = dir.join("herdr.sock");
    let browser_path = dir.join("browser.sock");
    let herdr = UnixListener::bind(&herdr_path).unwrap();
    let (tx, rx) = mpsc::channel();
    let server = SocketServer::bind(&browser_path, move |request| {
        tx.send(request.command).unwrap();
        ResponseEnvelope {
            protocol_version: PROTOCOL_VERSION,
            request_id: request.request_id,
            status: ResponseStatus::Ok,
            error_code: None,
            message: None,
            state: None,
        }
    })
    .unwrap();
    let browser_thread = thread::spawn(move || {
        for _ in 0..5 {
            server.accept().unwrap().join().unwrap();
        }
    });
    let mut child = ProcessCommand::new(env!("CARGO_BIN_EXE_lavish-browser-herdr-sync"))
        .args(["--session", "main", "--socket"])
        .arg(&herdr_path)
        .env("LAVISH_BROWSER_SOCKET", &browser_path)
        .spawn()
        .unwrap();
    let (mut events, _) = herdr.accept().unwrap();
    let request = receive(&mut events);
    assert_eq!(request["method"], "events.subscribe");
    send(&mut events, json!({"id":request["id"],"result":{}}));
    let snapshot = |label: &str| {
        json!({"focused_workspace_id":"w1","focused_tab_id":"t1","tabs":[
            {"workspace_id":"w1","tab_id":"t1","label":"Focused"},
            {"workspace_id":"w2","tab_id":"t2","label":label}
        ]})
    };
    for (index, label) in ["Old", "Review <日本語>"].iter().enumerate() {
        if index == 1 {
            send(
                &mut events,
                json!({"event":"tab_renamed","data":{"tab_id":"t2","workspace_id":"w2"}}),
            );
        }
        let (mut stream, _) = herdr.accept().unwrap();
        let request = receive(&mut stream);
        assert_eq!(request["method"], "session.snapshot");
        send(
            &mut stream,
            json!({"id":request["id"],"result":{"snapshot":snapshot(label)}}),
        );
        for (workspace, tab, name) in [("w1", "t1", "Focused"), ("w2", "t2", *label)] {
            assert_eq!(
                rx.recv_timeout(Duration::from_secs(10)).unwrap(),
                Command::RenameHerdrProject {
                    session_name: "main".into(),
                    workspace_id: workspace.into(),
                    tab_id: tab.into(),
                    name: name.into()
                }
            );
        }
        if index == 0 {
            assert_eq!(
                rx.recv_timeout(Duration::from_secs(10)).unwrap(),
                Command::SelectHerdrProject {
                    session_name: "main".into(),
                    workspace_id: "w1".into(),
                    tab_id: "t1".into()
                }
            );
        }
    }
    child.kill().unwrap();
    child.wait().unwrap();
    browser_thread.join().unwrap();
    assert!(rx.try_recv().is_err());
    std::fs::remove_dir_all(dir).unwrap();
}
