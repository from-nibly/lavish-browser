use lavish_browser_control::SocketServer;
use lavish_browser_protocol::{
    Command, PROTOCOL_VERSION, ProjectKey, RequestEnvelope, ResponseEnvelope, ResponseStatus,
};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command as ProcessCommand;
use std::sync::mpsc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "lavish-cli-{}-{}-{name}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

fn executable(path: &std::path::Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn ack(request: &RequestEnvelope) -> ResponseEnvelope {
    ResponseEnvelope {
        protocol_version: PROTOCOL_VERSION,
        request_id: request.request_id.clone(),
        status: ResponseStatus::Ok,
        error_code: None,
        message: None,
        state: None,
    }
}

#[test]
fn launcher_forwards_bytes_preserves_args_and_routes_standalone() {
    let dir = temp_dir("opened");
    let socket = dir.join("control.sock");
    let args_log = dir.join("args");
    executable(
        &dir.join("npx"),
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$ARGS_LOG\"\nprintf 'session:\\n  file: /tmp/review.html\\n  url: \"http://127.0.0.1:4387/session/abc\"\\n  status: opened\\nnext_step: \"poll\"\\n'\nprintf 'upstream warning\\n' >&2\n",
    );
    let (tx, rx) = mpsc::channel();
    let server = SocketServer::bind(&socket, move |request| {
        tx.send(request.clone()).unwrap();
        ack(&request)
    })
    .unwrap();
    let serving = thread::spawn(move || {
        for _ in 0..2 {
            server.accept().unwrap().join().unwrap();
        }
    });

    let output = ProcessCommand::new(env!("CARGO_BIN_EXE_lavish-open"))
        .args(["/tmp/review.html", "--no-gate"])
        .env(
            "PATH",
            format!(
                "{}:{}",
                dir.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("ARGS_LOG", &args_log)
        .env("LAVISH_BROWSER_SOCKET", &socket)
        .env_remove("ZELLIJ_SESSION_NAME")
        .env_remove("ZELLIJ_PANE_ID")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"session:\n  file: /tmp/review.html\n  url: \"http://127.0.0.1:4387/session/abc\"\n  status: opened\nnext_step: \"poll\"\n");
    assert_eq!(output.stderr, b"upstream warning\n");
    assert_eq!(
        fs::read_to_string(args_log).unwrap(),
        "-y\nlavish-axi\n/tmp/review.html\n--no-gate\n--no-open\n"
    );
    assert!(matches!(rx.recv().unwrap().command, Command::Ping));
    match rx.recv().unwrap().command {
        Command::OpenUrl {
            project,
            source_file,
            ..
        } => {
            assert_eq!(
                project.key,
                ProjectKey::Standalone {
                    label: "Standalone".into()
                }
            );
            assert_eq!(source_file, "/tmp/review.html");
        }
        command => panic!("unexpected command: {command:?}"),
    }
    serving.join().unwrap();
}

#[test]
fn user_ended_and_upstream_failure_never_contact_or_start_browser() {
    for (name, status, exit, expected_success) in [
        ("ended", "user-ended", 0, true),
        ("failed", "opened", 17, false),
    ] {
        let dir = temp_dir(name);
        let marker = dir.join("started");
        executable(
            &dir.join("lavish-browser"),
            &format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
        );
        executable(
            &dir.join("npx"),
            &format!(
                "#!/bin/sh\nprintf 'raw-prefix\\n' >&2\nprintf 'session:\\n  file: /tmp/x.html\\n  url: \"http://localhost:4387/session/x\"\\n  status: {status}\\n'\nexit {exit}\n"
            ),
        );
        let output = ProcessCommand::new(env!("CARGO_BIN_EXE_lavish-open"))
            .arg("/tmp/x.html")
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    dir.display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env("LAVISH_BROWSER_EXECUTABLE", dir.join("lavish-browser"))
            .env("LAVISH_BROWSER_SOCKET", dir.join("absent.sock"))
            .env_remove("ZELLIJ_SESSION_NAME")
            .output()
            .unwrap();
        assert_eq!(output.status.success(), expected_success);
        assert!(output.stderr.starts_with(b"raw-prefix\n"));
        assert!(!marker.exists(), "browser was started for {name}");
    }
}

#[test]
fn launcher_starts_browser_and_waits_for_ping_before_opening() {
    let dir = temp_dir("startup");
    let socket = dir.join("control.sock");
    let marker = dir.join("started");
    executable(
        &dir.join("npx"),
        "#!/bin/sh\nprintf 'session:\\n  file: /tmp/start.html\\n  url: http://localhost:4387/session/start\\n  status: opened\\n'\n",
    );
    let python = r#"import json,os,socket
p=os.environ['LAVISH_BROWSER_SOCKET']
try: os.unlink(p)
except FileNotFoundError: pass
s=socket.socket(socket.AF_UNIX); s.bind(p); os.chmod(p,0o600); s.listen()
for _ in range(2):
 c,_=s.accept(); data=b''
 while not data.endswith(b'\n'): data += c.recv(4096)
 r=json.loads(data); c.sendall((json.dumps({'protocol_version':1,'request_id':r['request_id'],'status':'ok'})+'\n').encode()); c.close()
"#;
    executable(
        &dir.join("fake-browser"),
        &format!(
            "#!/bin/sh\ntouch '{}'\nexec python3 -c '{}'\n",
            marker.display(),
            python.replace('\'', "'\\''")
        ),
    );
    let output = ProcessCommand::new(env!("CARGO_BIN_EXE_lavish-open"))
        .arg("/tmp/start.html")
        .env(
            "PATH",
            format!(
                "{}:{}",
                dir.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("LAVISH_BROWSER_EXECUTABLE", dir.join("fake-browser"))
        .env("LAVISH_BROWSER_SOCKET", &socket)
        .env_remove("ZELLIJ_SESSION_NAME")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(marker.exists());
}

#[test]
fn lifecycle_helper_is_quiet_when_absent_and_never_starts_browser() {
    let dir = temp_dir("helper");
    let marker = dir.join("started");
    executable(
        &dir.join("browser"),
        &format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
    );
    let output = ProcessCommand::new(env!("CARGO_BIN_EXE_lavish-browser-ctl"))
        .args(["select-project", "main", "7"])
        .env("LAVISH_BROWSER_SOCKET", dir.join("absent.sock"))
        .env("LAVISH_BROWSER_EXECUTABLE", dir.join("browser"))
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    assert!(!marker.exists());
}
