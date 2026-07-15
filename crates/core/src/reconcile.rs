use crate::{BrowserModel, DocumentLifecycle, ProjectKey};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io;
use std::process::Command;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveZellijState {
    tabs_by_session: HashMap<String, HashSet<u32>>,
}

impl LiveZellijState {
    pub fn insert_session(
        &mut self,
        session_name: impl Into<String>,
        stable_tab_ids: impl IntoIterator<Item = u32>,
    ) {
        self.tabs_by_session
            .insert(session_name.into(), stable_tab_ids.into_iter().collect());
    }

    pub fn contains(&self, session_name: &str, stable_tab_id: u32) -> bool {
        self.tabs_by_session
            .get(session_name)
            .is_some_and(|tabs| tabs.contains(&stable_tab_id))
    }
}

pub trait ZellijStateSource {
    type Error;

    fn live_state(
        &self,
        relevant_sessions: &HashSet<String>,
    ) -> Result<LiveZellijState, Self::Error>;
}

#[derive(Debug)]
pub enum ReconcileError {
    Command(io::Error),
    CommandFailed {
        arguments: Vec<String>,
        stderr: String,
    },
    InvalidUtf8 {
        arguments: Vec<String>,
    },
    InvalidTabJson {
        session_name: String,
        message: String,
    },
}

impl fmt::Display for ReconcileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Command(error) => write!(formatter, "failed to execute zellij: {error}"),
            Self::CommandFailed { arguments, stderr } => write!(
                formatter,
                "zellij {} failed: {}",
                arguments.join(" "),
                stderr.trim()
            ),
            Self::InvalidUtf8 { arguments } => {
                write!(
                    formatter,
                    "zellij {} returned non-UTF-8 output",
                    arguments.join(" ")
                )
            }
            Self::InvalidTabJson {
                session_name,
                message,
            } => write!(
                formatter,
                "invalid tab JSON for Zellij session {session_name:?}: {message}"
            ),
        }
    }
}

impl std::error::Error for ReconcileError {}

pub struct CommandOutput {
    pub success: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub trait CommandRunner {
    fn run(&self, program: &str, arguments: &[String]) -> io::Result<CommandOutput>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct StdCommandRunner;

impl CommandRunner for StdCommandRunner {
    fn run(&self, program: &str, arguments: &[String]) -> io::Result<CommandOutput> {
        let output = Command::new(program).args(arguments).output()?;
        Ok(CommandOutput {
            success: output.status.success(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

pub struct ZellijCommandState<R = StdCommandRunner> {
    runner: R,
}

impl ZellijCommandState<StdCommandRunner> {
    pub fn system() -> Self {
        Self {
            runner: StdCommandRunner,
        }
    }
}

impl<R> ZellijCommandState<R> {
    pub fn new(runner: R) -> Self {
        Self { runner }
    }

    pub fn into_inner(self) -> R {
        self.runner
    }

    fn run(&self, arguments: Vec<String>) -> Result<String, ReconcileError>
    where
        R: CommandRunner,
    {
        let output = self
            .runner
            .run("zellij", &arguments)
            .map_err(ReconcileError::Command)?;
        if !output.success {
            return Err(ReconcileError::CommandFailed {
                arguments,
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        String::from_utf8(output.stdout).map_err(|_| ReconcileError::InvalidUtf8 { arguments })
    }

    fn run_if_live(&self, arguments: Vec<String>) -> Result<Option<String>, ReconcileError>
    where
        R: CommandRunner,
    {
        let output = self
            .runner
            .run("zellij", &arguments)
            .map_err(ReconcileError::Command)?;
        if !output.success {
            return Ok(None);
        }
        String::from_utf8(output.stdout)
            .map(Some)
            .map_err(|_| ReconcileError::InvalidUtf8 { arguments })
    }
}

impl<R: CommandRunner> ZellijStateSource for ZellijCommandState<R> {
    type Error = ReconcileError;

    fn live_state(
        &self,
        relevant_sessions: &HashSet<String>,
    ) -> Result<LiveZellijState, Self::Error> {
        let session_output = self.run(vec![
            "list-sessions".into(),
            "--short".into(),
            "--no-formatting".into(),
        ])?;
        let active_sessions = session_output
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<HashSet<_>>();

        let mut state = LiveZellijState::default();
        for session_name in relevant_sessions {
            if !active_sessions.contains(session_name.as_str()) {
                continue;
            }
            let Some(tab_output) = self.run_if_live(vec![
                "--session".into(),
                session_name.clone(),
                "action".into(),
                "list-tabs".into(),
                "--json".into(),
            ])?
            else {
                // Sessions can disappear between the list and tab queries. Some Zellij
                // versions also include resumable exited sessions in the short list.
                continue;
            };
            let tabs: Vec<TabRecord> = serde_json::from_str(&tab_output).map_err(|error| {
                ReconcileError::InvalidTabJson {
                    session_name: session_name.clone(),
                    message: error.to_string(),
                }
            })?;
            state.insert_session(session_name.clone(), tabs.into_iter().map(|tab| tab.tab_id));
        }
        Ok(state)
    }
}

#[derive(Debug, Deserialize)]
struct TabRecord {
    tab_id: u32,
}

pub fn reconcile_model(model: &mut BrowserModel, live: &LiveZellijState) {
    model.projects.retain(|project| match &project.key {
        ProjectKey::Standalone { .. } => true,
        ProjectKey::Zellij {
            session_name,
            stable_tab_id,
        } => live.contains(session_name, *stable_tab_id),
    });

    for project in &mut model.projects {
        for document in &mut project.documents {
            document.lifecycle = DocumentLifecycle::Dormant;
            document.load_error = None;
        }
    }

    if model.selected_project.as_ref().is_some_and(|selected| {
        !model
            .projects
            .iter()
            .any(|project| &project.key == selected)
    }) {
        model.selected_project = None;
    }
}

pub fn reconcile_from_source<S: ZellijStateSource>(
    model: &mut BrowserModel,
    source: &S,
) -> Result<(), S::Error> {
    let relevant_sessions = model
        .projects
        .iter()
        .filter_map(|project| match &project.key {
            ProjectKey::Zellij { session_name, .. } => Some(session_name.clone()),
            ProjectKey::Standalone { .. } => None,
        })
        .collect();
    let live = source.live_state(&relevant_sessions)?;
    reconcile_model(model, &live);
    Ok(())
}
