use lavish_browser_control::{BrowserControl, ControlError, SocketClient, default_socket_path};
use lavish_browser_protocol::Command;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::env;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

const MAX_HERDR_FRAME_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct HerdrTab {
    tab_id: String,
    workspace_id: String,
    #[serde(default)]
    label: String,
}

#[derive(Debug, Deserialize)]
struct HerdrSnapshot {
    focused_workspace_id: Option<String>,
    focused_tab_id: Option<String>,
    #[serde(default)]
    tabs: Vec<HerdrTab>,
}

#[derive(Debug, Deserialize)]
struct SnapshotResult {
    snapshot: HerdrSnapshot,
}

#[derive(Debug, Deserialize)]
struct HerdrErrorBody {
    message: String,
}

#[derive(Debug, Deserialize)]
struct HerdrResponse<T> {
    id: String,
    result: Option<T>,
    error: Option<HerdrErrorBody>,
}

#[derive(Debug, Deserialize)]
struct HerdrEvent {
    event: String,
    #[serde(default)]
    data: Value,
}

pub fn run_herdr_sync(arguments: Vec<String>) -> Result<(), String> {
    let options = SyncOptions::parse(arguments)?;
    if options.once {
        synchronize_once(&options, &browser_client())?;
        return Ok(());
    }

    loop {
        if let Err(error) = synchronize(&options, &browser_client()) {
            eprintln!(
                "lavish-browser-herdr-sync: waiting for HerdR session {:?}: {error}",
                options.session_name
            );
            thread::sleep(Duration::from_secs(1));
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SyncOptions {
    session_name: String,
    socket_path: PathBuf,
    once: bool,
}

impl SyncOptions {
    fn parse(arguments: Vec<String>) -> Result<Self, String> {
        let mut session_name = "main".to_owned();
        let mut socket_path = None;
        let mut once = false;
        let mut arguments = arguments.into_iter();
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--session" => {
                    session_name = arguments
                        .next()
                        .ok_or_else(|| "--session requires a value".to_owned())?;
                }
                "--socket" => {
                    socket_path = Some(PathBuf::from(
                        arguments
                            .next()
                            .ok_or_else(|| "--socket requires a value".to_owned())?,
                    ));
                }
                "--once" => once = true,
                "-h" | "--help" => {
                    return Err(
                        "usage: lavish-browser-herdr-sync [--session NAME] [--socket PATH] [--once]"
                            .to_owned(),
                    );
                }
                _ => return Err(format!("unknown option {argument:?}")),
            }
        }
        if session_name.is_empty() || session_name.contains('/') {
            return Err("session name must be non-empty and cannot contain '/'".to_owned());
        }
        let socket_path = socket_path
            .or_else(|| env::var_os("HERDR_SOCKET_PATH").map(PathBuf::from))
            .unwrap_or_else(|| {
                let home = env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
                home.join(".config/herdr/sessions")
                    .join(&session_name)
                    .join("herdr.sock")
            });
        Ok(Self {
            session_name,
            socket_path,
            once,
        })
    }
}

fn browser_client() -> SocketClient {
    SocketClient::new(default_socket_path())
}

fn synchronize(options: &SyncOptions, browser: &SocketClient) -> Result<(), String> {
    let snapshot = request_snapshot(&options.socket_path)?;
    let mut tabs = tab_map(&snapshot);
    select_focused(browser, &options.session_name, &snapshot)?;

    let mut reader = subscribe(&options.socket_path)?;
    loop {
        let event: HerdrEvent = read_json_line(&mut reader)?;
        match event.event.as_str() {
            "workspace_focused" | "tab_focused" => {
                let snapshot = request_snapshot(&options.socket_path)?;
                tabs = tab_map(&snapshot);
                select_focused(browser, &options.session_name, &snapshot)?;
            }
            "tab_created" => {
                let snapshot = request_snapshot(&options.socket_path)?;
                tabs = tab_map(&snapshot);
            }
            "tab_closed" => {
                if let (Some(workspace_id), Some(tab_id)) = (
                    event.data.get("workspace_id").and_then(Value::as_str),
                    event.data.get("tab_id").and_then(Value::as_str),
                ) {
                    dispatch_browser(
                        browser,
                        Command::CloseHerdrProject {
                            session_name: options.session_name.clone(),
                            workspace_id: workspace_id.to_owned(),
                            tab_id: tab_id.to_owned(),
                        },
                    )?;
                    tabs.remove(tab_id);
                }
            }
            "workspace_closed" => {
                if let Some(workspace_id) = event.data.get("workspace_id").and_then(Value::as_str) {
                    let closed = tabs
                        .values()
                        .filter(|tab| tab.workspace_id == workspace_id)
                        .cloned()
                        .collect::<Vec<_>>();
                    for tab in closed {
                        dispatch_browser(
                            browser,
                            Command::CloseHerdrProject {
                                session_name: options.session_name.clone(),
                                workspace_id: tab.workspace_id.clone(),
                                tab_id: tab.tab_id.clone(),
                            },
                        )?;
                        tabs.remove(&tab.tab_id);
                    }
                }
            }
            "tab_renamed" => {}
            _ => {}
        }
    }
}

fn synchronize_once(options: &SyncOptions, browser: &SocketClient) -> Result<(), String> {
    let snapshot = request_snapshot(&options.socket_path)?;
    select_focused(browser, &options.session_name, &snapshot)
}

fn request_snapshot(path: &Path) -> Result<HerdrSnapshot, String> {
    let response: HerdrResponse<SnapshotResult> = request(
        path,
        "lavish:session:snapshot",
        "session.snapshot",
        serde_json::json!({}),
    )?;
    response
        .result
        .map(|result| result.snapshot)
        .ok_or_else(|| "HerdR snapshot response did not include a result".to_owned())
}

fn tab_map(snapshot: &HerdrSnapshot) -> HashMap<String, HerdrTab> {
    snapshot
        .tabs
        .iter()
        .cloned()
        .map(|tab| (tab.tab_id.clone(), tab))
        .collect()
}

fn select_focused(
    browser: &SocketClient,
    session_name: &str,
    snapshot: &HerdrSnapshot,
) -> Result<(), String> {
    let (Some(workspace_id), Some(tab_id)) = (
        snapshot.focused_workspace_id.as_ref(),
        snapshot.focused_tab_id.as_ref(),
    ) else {
        return Ok(());
    };
    dispatch_browser(
        browser,
        Command::SelectHerdrProject {
            session_name: session_name.to_owned(),
            workspace_id: workspace_id.clone(),
            tab_id: tab_id.clone(),
        },
    )
}

fn dispatch_browser(browser: &SocketClient, command: Command) -> Result<(), String> {
    match browser.request(&crate::request(command)) {
        Ok(_) | Err(ControlError::Absent(_)) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn request<T: for<'de> Deserialize<'de>>(
    path: &Path,
    id: &str,
    method: &str,
    params: Value,
) -> Result<HerdrResponse<T>, String> {
    let mut stream = UnixStream::connect(path)
        .map_err(|error| format!("could not connect to {}: {error}", path.display()))?;
    let request = serde_json::json!({"id": id, "method": method, "params": params});
    serde_json::to_writer(&mut stream, &request).map_err(|error| error.to_string())?;
    stream.write_all(b"\n").map_err(|error| error.to_string())?;
    let mut reader = BufReader::new(stream);
    let response: HerdrResponse<T> = read_json_line(&mut reader)?;
    if response.id != id {
        return Err(format!(
            "HerdR response ID {:?} did not match {id:?}",
            response.id
        ));
    }
    if let Some(error) = response.error {
        return Err(error.message);
    }
    Ok(response)
}

fn subscribe(path: &Path) -> Result<BufReader<UnixStream>, String> {
    let mut stream = UnixStream::connect(path)
        .map_err(|error| format!("could not connect to {}: {error}", path.display()))?;
    let request = serde_json::json!({
        "id": "lavish:events",
        "method": "events.subscribe",
        "params": {
            "subscriptions": [
                {"type": "workspace.focused"},
                {"type": "workspace.closed"},
                {"type": "tab.created"},
                {"type": "tab.closed"},
                {"type": "tab.focused"},
                {"type": "tab.renamed"}
            ]
        }
    });
    serde_json::to_writer(&mut stream, &request).map_err(|error| error.to_string())?;
    stream.write_all(b"\n").map_err(|error| error.to_string())?;
    let mut reader = BufReader::new(stream);
    let acknowledgement: HerdrResponse<Value> = read_json_line(&mut reader)?;
    if acknowledgement.id != "lavish:events" {
        return Err("HerdR subscription returned a mismatched request ID".to_owned());
    }
    if let Some(error) = acknowledgement.error {
        return Err(error.message);
    }
    Ok(reader)
}

fn read_json_line<T: for<'de> Deserialize<'de>, R: BufRead>(reader: &mut R) -> Result<T, String> {
    let mut bytes = Vec::new();
    let count = reader
        .read_until(b'\n', &mut bytes)
        .map_err(|error| error.to_string())?;
    if count == 0 {
        return Err("HerdR socket closed".to_owned());
    }
    if bytes.len() > MAX_HERDR_FRAME_BYTES {
        return Err(format!(
            "HerdR frame exceeded {MAX_HERDR_FRAME_BYTES} bytes"
        ));
    }
    serde_json::from_slice(&bytes).map_err(|error| format!("invalid HerdR JSON: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_defaults_and_explicit_options() {
        let defaults = SyncOptions::parse(vec!["--once".into()]).unwrap();
        assert_eq!(defaults.session_name, "main");
        assert!(defaults.once);

        let explicit = SyncOptions::parse(vec![
            "--session".into(),
            "work".into(),
            "--socket".into(),
            "/tmp/herdr.sock".into(),
        ])
        .unwrap();
        assert_eq!(explicit.session_name, "work");
        assert_eq!(explicit.socket_path, PathBuf::from("/tmp/herdr.sock"));
    }

    #[test]
    fn indexes_snapshot_tabs_by_stable_id() {
        let snapshot = HerdrSnapshot {
            focused_workspace_id: Some("w1".into()),
            focused_tab_id: Some("w1:t2".into()),
            tabs: vec![HerdrTab {
                tab_id: "w1:t2".into(),
                workspace_id: "w1".into(),
                label: "review".into(),
            }],
        };
        assert_eq!(tab_map(&snapshot)["w1:t2"].label, "review");
    }

    #[test]
    fn rejects_invalid_session_names() {
        assert!(SyncOptions::parse(vec!["--session".into(), "bad/name".into()]).is_err());
    }
}
