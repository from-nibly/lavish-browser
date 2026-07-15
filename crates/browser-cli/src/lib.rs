//! Shared production launcher and lifecycle-helper behavior.

use lavish_browser_control::{BrowserControl, ControlError, SocketClient, default_socket_path};
use lavish_browser_core::labels::project_label;
use lavish_browser_protocol::{
    Command, ProjectKey, ProjectMetadata, RequestEnvelope, ResponseEnvelope, ResponseStatus,
    validate_session_url,
};
use serde::Deserialize;
use std::ffi::OsString;
use std::io::{self, Write};
use std::process::{Command as ProcessCommand, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

pub const LAVISH_OPEN_BINARY: &str = "lavish-open";
pub const CONTROL_BINARY: &str = "lavish-browser-ctl";
static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
pub struct ProcessOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub trait ProcessRunner {
    fn output(&self, program: &str, args: &[String]) -> Result<ProcessOutput, String>;
}

pub struct SystemRunner;
impl ProcessRunner for SystemRunner {
    fn output(&self, program: &str, args: &[String]) -> Result<ProcessOutput, String> {
        let output = ProcessCommand::new(program)
            .args(args)
            .output()
            .map_err(|error| format!("failed to run {program}: {error}"))?;
        Ok(ProcessOutput {
            status: output.status,
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LavishSession {
    pub file: String,
    pub url: String,
    pub status: String,
}

#[derive(Debug, Deserialize)]
struct PaneInfo {
    #[serde(alias = "pane_id")]
    id: u32,
    #[serde(default)]
    is_plugin: bool,
    tab_id: u32,
    #[serde(default)]
    tab_name: String,
}

pub fn run_lavish_open(args: Vec<String>) -> Result<(), String> {
    if args.is_empty() {
        return Err("usage: lavish-open <html-file> [lavish-axi open options]".to_owned());
    }
    let runner = SystemRunner;
    let upstream_args = upstream_arguments(args);
    let output = runner
        .output("npx", &upstream_args)
        .map_err(|error| format!("failed to invoke upstream lavish-axi: {error}"))?;

    io::stdout()
        .write_all(&output.stdout)
        .map_err(|error| format!("failed to forward upstream stdout: {error}"))?;
    io::stderr()
        .write_all(&output.stderr)
        .map_err(|error| format!("failed to forward upstream stderr: {error}"))?;
    if !output.status.success() {
        return Err(format!("upstream lavish-axi exited with {}", output.status));
    }

    let text = std::str::from_utf8(&output.stdout)
        .map_err(|_| "upstream lavish-axi stdout was not UTF-8 TOON".to_owned())?;
    let session = parse_lavish_toon(text)?;
    if session.status == "user-ended" {
        return Ok(());
    }
    let project = resolve_project(&runner)?;
    let client = SocketClient::new(default_socket_path());
    ensure_browser(&client)?;
    client
        .request(&request(Command::OpenUrl {
            project,
            source_file: session.file,
            url: session.url,
        }))
        .map_err(|error| format!("could not route Lavish session to browser: {error}"))?;
    Ok(())
}

pub fn upstream_arguments(args: Vec<String>) -> Vec<String> {
    let mut upstream_args = vec!["-y".to_owned(), "lavish-axi".to_owned()];
    upstream_args.extend(args);
    if !upstream_args.iter().any(|argument| argument == "--no-open") {
        upstream_args.push("--no-open".to_owned());
    }
    upstream_args
}

pub fn parse_lavish_toon(output: &str) -> Result<LavishSession, String> {
    let mut in_session = false;
    let mut file = None;
    let mut url = None;
    let mut status = None;
    for line in output.lines() {
        if line.trim() == "session:" {
            in_session = true;
            continue;
        }
        if in_session
            && !line.chars().next().is_some_and(char::is_whitespace)
            && !line.trim().is_empty()
        {
            break;
        }
        if !in_session {
            continue;
        }
        let Some((key, value)) = line.trim().split_once(':') else {
            continue;
        };
        let value = decode_toon_scalar(value.trim())?;
        match key {
            "file" => file = Some(value),
            "url" => url = Some(value),
            "status" => status = Some(value),
            _ => {}
        }
    }
    let session = LavishSession {
        file: file.ok_or_else(|| "upstream TOON did not include session.file".to_owned())?,
        url: url.ok_or_else(|| "upstream TOON did not include session.url".to_owned())?,
        status: status.ok_or_else(|| "upstream TOON did not include session.status".to_owned())?,
    };
    validate_session_url(&session.url)
        .map_err(|error| format!("refusing upstream session URL {:?}: {error}", session.url))?;
    Ok(session)
}

fn decode_toon_scalar(value: &str) -> Result<String, String> {
    if value.starts_with('"') {
        serde_json::from_str(value).map_err(|error| format!("invalid quoted TOON value: {error}"))
    } else if value.is_empty() {
        Err("empty TOON value".to_owned())
    } else {
        Ok(value.to_owned())
    }
}

pub fn resolve_project(runner: &impl ProcessRunner) -> Result<ProjectMetadata, String> {
    let Some(session_name) = std::env::var_os("ZELLIJ_SESSION_NAME") else {
        return Ok(ProjectMetadata {
            key: ProjectKey::Standalone {
                label: "Standalone".to_owned(),
            },
            label: "Standalone".to_owned(),
            raw_tab_name: None,
        });
    };
    let pane_id = std::env::var("ZELLIJ_PANE_ID")
        .map_err(|_| "ZELLIJ_SESSION_NAME is set but ZELLIJ_PANE_ID is missing".to_owned())?
        .parse::<u32>()
        .map_err(|_| "ZELLIJ_PANE_ID must be an unsigned integer".to_owned())?;
    let output = runner.output(
        "zellij",
        &["action", "list-panes", "--all", "--json"].map(str::to_owned),
    )?;
    if !output.status.success() {
        return Err(format!(
            "zellij action list-panes exited with {}",
            output.status
        ));
    }
    project_from_panes(
        session_name.to_string_lossy().into_owned(),
        pane_id,
        &output.stdout,
    )
}

pub fn project_from_panes(
    session_name: String,
    pane_id: u32,
    output: &[u8],
) -> Result<ProjectMetadata, String> {
    let panes: Vec<PaneInfo> = serde_json::from_slice(output)
        .map_err(|error| format!("invalid Zellij list-panes JSON: {error}"))?;
    let pane = panes
        .into_iter()
        .find(|pane| !pane.is_plugin && pane.id == pane_id)
        .ok_or_else(|| format!("invoking Zellij pane {pane_id} was not found"))?;
    Ok(ProjectMetadata {
        key: ProjectKey::Zellij {
            session_name,
            stable_tab_id: pane.tab_id,
        },
        label: project_label(&pane.tab_name, pane.tab_id),
        raw_tab_name: Some(pane.tab_name),
    })
}

pub fn run_control(args: Vec<String>) -> Result<(), String> {
    let client = SocketClient::new(default_socket_path());
    let command = match args.as_slice() {
        [name, session, tab] if name == "select-project" || name == "close-project" => {
            let stable_tab_id = tab
                .parse::<u32>()
                .map_err(|_| "stable-tab-id must be an unsigned integer".to_owned())?;
            let command = if name == "select-project" {
                Command::SelectProject {
                    session_name: session.clone(),
                    stable_tab_id,
                }
            } else {
                Command::CloseProject {
                    session_name: session.clone(),
                    stable_tab_id,
                }
            };
            match client.request(&request(command)) {
                Ok(_) | Err(ControlError::Absent(_)) => return Ok(()),
                Err(error) => return Err(error.to_string()),
            }
        }
        [name] if name == "status" || name == "ping" => {
            if name == "status" { Command::InspectState } else { Command::Ping }
        }
        [name, json] if name == "status" && json == "--json" => Command::InspectState,
        _ => return Err("usage: lavish-browser-ctl <select-project|close-project> <zellij-session> <stable-tab-id>\n       lavish-browser-ctl status [--json]\n       lavish-browser-ctl ping".to_owned()),
    };
    match client.request(&request(command)) {
        Ok(response) => {
            println!(
                "{}",
                serde_json::to_string(&response).map_err(|e| e.to_string())?
            );
            Ok(())
        }
        Err(ControlError::Absent(_)) => {
            println!("{{\"status\":\"absent\"}}");
            Ok(())
        }
        Err(error) => Err(error.to_string()),
    }
}

fn ensure_browser(client: &SocketClient) -> Result<ResponseEnvelope, String> {
    match client.request(&request(Command::Ping)) {
        Ok(response) => return Ok(response),
        Err(ControlError::Absent(_)) => {}
        Err(error) => return Err(format!("browser readiness check failed: {error}")),
    }
    let executable: OsString = std::env::var_os("LAVISH_BROWSER_EXECUTABLE")
        .unwrap_or_else(|| OsString::from("lavish-browser"));
    ProcessCommand::new(&executable)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("failed to start {:?}: {error}", executable))?;
    let delays = [20, 40, 80, 160, 300, 500, 750, 1000];
    for delay in delays {
        thread::sleep(Duration::from_millis(delay));
        match client.request(&request(Command::Ping)) {
            Ok(response) => return Ok(response),
            Err(ControlError::Absent(_)) => continue,
            Err(error) => return Err(format!("browser readiness check failed: {error}")),
        }
    }
    Err("timed out waiting for lavish-browser control socket".to_owned())
}

pub fn request(command: Command) -> RequestEnvelope {
    let sequence = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    RequestEnvelope::new(format!("{}-{sequence}", std::process::id()), command)
}

pub fn ignored(request_id: String, message: impl Into<String>) -> ResponseEnvelope {
    ResponseEnvelope {
        protocol_version: lavish_browser_protocol::PROTOCOL_VERSION,
        request_id,
        status: ResponseStatus::Ignored,
        error_code: None,
        message: Some(message.into()),
        state: None,
    }
}
