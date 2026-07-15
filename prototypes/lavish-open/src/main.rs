use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

#[derive(Debug, Clone, PartialEq, Eq)]
struct LavishOpenOutput {
    file: String,
    url: String,
    status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ProjectContext {
    Zellij {
        session_name: String,
        stable_tab_id: u32,
        raw_tab_name: String,
        label: String,
    },
    Standalone {
        label: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct OpenUrl {
    protocol_version: u32,
    command: String,
    project: ProjectContext,
    source_file: String,
    url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct LifecycleMessage {
    protocol_version: u32,
    command: String,
    zellij_session_name: String,
    stable_tab_id: u32,
}

#[derive(Debug, Deserialize)]
struct PaneInfo {
    id: u32,
    #[serde(default)]
    is_plugin: bool,
    tab_id: u32,
    #[serde(default)]
    tab_name: String,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("lavish-open-prototype: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.first().map(String::as_str) == Some("listen") {
        let socket = args
            .get(1)
            .map(PathBuf::from)
            .unwrap_or_else(default_socket_path);
        return listen_once(&socket);
    }
    if matches!(
        args.first().map(String::as_str),
        Some("select-project" | "close-project")
    ) {
        return send_lifecycle_command(&args);
    }
    if args.is_empty() {
        return Err("usage: lavish-open-prototype <html-file> [lavish-open options]\n       lavish-open-prototype listen [socket-path]\n       lavish-open-prototype <select-project|close-project> <zellij-session> <stable-tab-id>".into());
    }

    let mut lavish_args = vec!["-y".to_owned(), "lavish-axi".to_owned()];
    lavish_args.extend(args);
    if !lavish_args.iter().any(|arg| arg == "--no-open") {
        lavish_args.push("--no-open".to_owned());
    }

    let output = Command::new("npx")
        .args(&lavish_args)
        .output()
        .map_err(|error| format!("failed to run upstream lavish-axi: {error}"))?;

    std::io::stdout()
        .write_all(&output.stdout)
        .map_err(|error| format!("failed to forward lavish stdout: {error}"))?;
    std::io::stderr()
        .write_all(&output.stderr)
        .map_err(|error| format!("failed to forward lavish stderr: {error}"))?;

    if !output.status.success() {
        return Err(format!("upstream lavish-axi exited with {}", output.status));
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|_| "upstream lavish-axi output was not UTF-8".to_owned())?;
    let session = parse_lavish_open_output(&stdout)?;
    if session.status == "user-ended" {
        return Ok(());
    }

    let message = OpenUrl {
        protocol_version: 1,
        command: "open_url".to_owned(),
        project: resolve_project_context()?,
        source_file: session.file,
        url: session.url,
    };
    send_message(&default_socket_path(), &message)
}

fn parse_lavish_open_output(output: &str) -> Result<LavishOpenOutput, String> {
    let file = toon_field(output, "file").ok_or("Lavish output did not include session.file")?;
    let url = toon_field(output, "url").ok_or("Lavish output did not include session.url")?;
    let status =
        toon_field(output, "status").ok_or("Lavish output did not include session.status")?;
    if !is_loopback_lavish_session_url(&url) {
        return Err(format!("refusing unexpected Lavish session URL: {url}"));
    }
    Ok(LavishOpenOutput { file, url, status })
}

fn toon_field(output: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    output.lines().find_map(|line| {
        let value = line.trim().strip_prefix(&prefix)?.trim();
        Some(unquote_toon(value))
    })
}

fn unquote_toon(value: &str) -> String {
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        serde_json::from_str::<String>(value)
            .unwrap_or_else(|_| value[1..value.len() - 1].to_owned())
    } else {
        value.to_owned()
    }
}

fn is_loopback_lavish_session_url(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    (value.starts_with("http://127.0.0.1:")
        || value.starts_with("http://localhost:")
        || value.starts_with("http://[::1]:")
        || value.starts_with("https://127.0.0.1:")
        || value.starts_with("https://localhost:")
        || value.starts_with("https://[::1]:"))
        && value.contains("/session/")
}

fn resolve_project_context() -> Result<ProjectContext, String> {
    let Ok(session_name) = std::env::var("ZELLIJ_SESSION_NAME") else {
        return Ok(ProjectContext::Standalone {
            label: "Standalone".to_owned(),
        });
    };
    let pane_id = std::env::var("ZELLIJ_PANE_ID")
        .map_err(|_| "ZELLIJ_SESSION_NAME is set but ZELLIJ_PANE_ID is missing".to_owned())?
        .parse::<u32>()
        .map_err(|_| "ZELLIJ_PANE_ID is not an integer".to_owned())?;
    let output = Command::new("zellij")
        .args(["action", "list-panes", "--all", "--json"])
        .output()
        .map_err(|error| format!("failed to query Zellij panes: {error}"))?;
    if !output.status.success() {
        return Err("zellij action list-panes failed".to_owned());
    }
    let panes: Vec<PaneInfo> = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("invalid Zellij pane JSON: {error}"))?;
    let pane = panes
        .into_iter()
        .find(|pane| !pane.is_plugin && pane.id == pane_id)
        .ok_or_else(|| format!("Zellij pane {pane_id} was not present in list-panes output"))?;
    Ok(ProjectContext::Zellij {
        session_name,
        stable_tab_id: pane.tab_id,
        label: project_label(&pane.tab_name, pane.tab_id),
        raw_tab_name: pane.tab_name,
    })
}

fn project_label(tab_name: &str, tab_id: u32) -> String {
    let metadata = decode_super_tabs_name(tab_name).unwrap_or_default();
    let directory = metadata.get("directory").filter(|value| !value.is_empty());
    let worktree = metadata.get("worktree").filter(|value| !value.is_empty());
    match (directory, worktree) {
        (Some(directory), Some(worktree)) if directory != worktree => {
            format!("{directory} · {worktree}")
        }
        (Some(directory), _) => directory.clone(),
        (_, Some(worktree)) => worktree.clone(),
        _ if !tab_name.trim().is_empty() && !metadata.contains_key("__super_tabs_id") => {
            tab_name.trim().to_owned()
        }
        _ => format!("Zellij tab {tab_id}"),
    }
}

fn decode_super_tabs_name(input: &str) -> Option<BTreeMap<String, String>> {
    let mut result = BTreeMap::new();
    let mut rest = input.trim();
    while !rest.is_empty() {
        let (key, after_equals) = rest.split_once('=')?;
        let key = key.trim();
        let (value, remaining) = parse_quoted(after_equals.trim_start())?;
        result.insert(key.to_owned(), value);
        rest = remaining.trim_start();
        if rest.is_empty() {
            break;
        }
        rest = rest.strip_prefix('|')?.trim_start();
    }
    Some(result)
}

fn parse_quoted(input: &str) -> Option<(String, &str)> {
    let mut chars = input.char_indices();
    if chars.next()?.1 != '"' {
        return None;
    }
    let mut value = String::new();
    let mut escaped = false;
    for (index, ch) in chars {
        if escaped {
            match ch {
                '\\' | '"' => value.push(ch),
                _ => return None,
            }
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return Some((value, &input[index + ch.len_utf8()..]));
        } else {
            value.push(ch);
        }
    }
    None
}

fn default_socket_path() -> PathBuf {
    if let Ok(path) = std::env::var("LAVISH_BROWSER_SOCKET") {
        return PathBuf::from(path);
    }
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("lavish-browser.sock")
}

fn send_lifecycle_command(args: &[String]) -> Result<(), String> {
    if args.len() != 3 {
        return Err(format!(
            "{} requires <zellij-session> <stable-tab-id>",
            args[0]
        ));
    }
    let message = LifecycleMessage {
        protocol_version: 1,
        command: args[0].replace('-', "_"),
        zellij_session_name: args[1].clone(),
        stable_tab_id: args[2]
            .parse()
            .map_err(|_| "stable-tab-id must be an unsigned integer".to_owned())?,
    };
    send_message(&default_socket_path(), &message)
}

fn send_message<T: Serialize>(socket: &Path, message: &T) -> Result<(), String> {
    let mut stream = UnixStream::connect(socket).map_err(|error| {
        format!(
            "cannot connect to browser socket {}: {error}",
            socket.display()
        )
    })?;
    serde_json::to_writer(&mut stream, message)
        .map_err(|error| format!("cannot encode browser message: {error}"))?;
    stream
        .write_all(b"\n")
        .map_err(|error| format!("cannot finish browser message: {error}"))
}

fn listen_once(socket: &Path) -> Result<(), String> {
    if socket.exists() {
        std::fs::remove_file(socket)
            .map_err(|error| format!("cannot remove stale socket {}: {error}", socket.display()))?;
    }
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "cannot create socket directory {}: {error}",
                parent.display()
            )
        })?;
    }
    let listener = UnixListener::bind(socket)
        .map_err(|error| format!("cannot bind {}: {error}", socket.display()))?;
    println!("listening on {}", socket.display());
    let (stream, _) = listener
        .accept()
        .map_err(|error| format!("accept failed: {error}"))?;
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|error| format!("read failed: {error}"))?;
    let message: serde_json::Value =
        serde_json::from_str(&line).map_err(|error| format!("invalid browser message: {error}"))?;
    println!("{}", serde_json::to_string_pretty(&message).unwrap());
    std::fs::remove_file(socket).ok();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_current_lavish_toon_shape() {
        let output = r#"session:
  file: /tmp/review.html
  url: "http://127.0.0.1:4387/session/0123456789abcdef"
  status: opened
next_step: "poll next"
"#;
        assert_eq!(
            parse_lavish_open_output(output).unwrap(),
            LavishOpenOutput {
                file: "/tmp/review.html".to_owned(),
                url: "http://127.0.0.1:4387/session/0123456789abcdef".to_owned(),
                status: "opened".to_owned(),
            }
        );
    }

    #[test]
    fn rejects_non_loopback_urls() {
        let output =
            "file: /tmp/x.html\nurl: \"https://example.com/session/key\"\nstatus: opened\n";
        assert!(parse_lavish_open_output(output).is_err());
    }

    #[test]
    fn derives_directory_and_worktree_label() {
        let raw = "__super_tabs_id=\"st-7-1\" | worktree=\"feature-auth\" | directory=\"api\"";
        assert_eq!(project_label(raw, 7), "api · feature-auth");
    }

    #[test]
    fn falls_back_without_identity_cells() {
        assert_eq!(project_label("My manual tab", 4), "My manual tab");
        assert_eq!(
            project_label("__super_tabs_id=\"st-4-1\" | agent=\"IDLE\"", 4),
            "Zellij tab 4"
        );
    }

    #[test]
    fn lifecycle_message_uses_socket_protocol_commands() {
        let message = LifecycleMessage {
            protocol_version: 1,
            command: "select_project".to_owned(),
            zellij_session_name: "main".to_owned(),
            stable_tab_id: 13,
        };
        assert_eq!(
            serde_json::to_value(message).unwrap(),
            serde_json::json!({
                "protocol_version": 1,
                "command": "select_project",
                "zellij_session_name": "main",
                "stable_tab_id": 13
            })
        );
    }
}
