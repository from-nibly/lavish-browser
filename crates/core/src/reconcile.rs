use crate::labels::{project_label, super_tabs_id};
use crate::{BrowserModel, DocumentLifecycle, ProjectKey};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io;
use std::process::Command;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveZellijState {
    tabs_by_session: HashMap<String, HashMap<u32, String>>,
    unresolved_sessions: HashSet<String>,
}

impl LiveZellijState {
    pub fn insert_session(
        &mut self,
        session_name: impl Into<String>,
        stable_tab_ids: impl IntoIterator<Item = u32>,
    ) {
        self.tabs_by_session.insert(
            session_name.into(),
            stable_tab_ids
                .into_iter()
                .map(|stable_tab_id| (stable_tab_id, String::new()))
                .collect(),
        );
    }

    pub fn insert_tabs(
        &mut self,
        session_name: impl Into<String>,
        tabs: impl IntoIterator<Item = (u32, String)>,
    ) {
        self.tabs_by_session
            .insert(session_name.into(), tabs.into_iter().collect());
    }

    pub fn contains(&self, session_name: &str, stable_tab_id: u32) -> bool {
        self.tabs_by_session
            .get(session_name)
            .is_some_and(|tabs| tabs.contains_key(&stable_tab_id))
    }

    fn restored_tab(&self, session_name: &str, raw_tab_name: Option<&str>) -> Option<(u32, &str)> {
        let durable_id = raw_tab_name.and_then(super_tabs_id)?;
        self.tabs_by_session
            .get(session_name)?
            .iter()
            .find_map(|(stable_tab_id, live_name)| {
                (super_tabs_id(live_name).as_deref() == Some(durable_id.as_str()))
                    .then_some((*stable_tab_id, live_name.as_str()))
            })
    }

    fn tab_name(&self, session_name: &str, stable_tab_id: u32) -> Option<&str> {
        self.tabs_by_session
            .get(session_name)?
            .get(&stable_tab_id)
            .map(String::as_str)
    }

    pub fn mark_unresolved(&mut self, session_name: impl Into<String>) {
        self.unresolved_sessions.insert(session_name.into());
    }

    pub fn is_unresolved(&self, session_name: &str) -> bool {
        self.unresolved_sessions.contains(session_name)
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

    fn query_tabs(&self, arguments: Vec<String>) -> Result<TabQueryResult, ReconcileError>
    where
        R: CommandRunner,
    {
        let output = self
            .runner
            .run("zellij", &arguments)
            .map_err(ReconcileError::Command)?;
        if !output.success {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Ok(if is_authoritative_missing_session(&stderr) {
                TabQueryResult::Missing
            } else {
                TabQueryResult::Unresolved
            });
        }
        String::from_utf8(output.stdout)
            .map(TabQueryResult::Live)
            .map_err(|_| ReconcileError::InvalidUtf8 { arguments })
    }
}

fn is_authoritative_missing_session(stderr: &str) -> bool {
    let Some(first_line) = stderr.lines().next() else {
        return false;
    };
    let first_line = first_line.trim().to_ascii_lowercase();
    let Some(after_prefix) = first_line.strip_prefix("session '") else {
        return false;
    };
    let Some((_, after_name)) = after_prefix.split_once('\'') else {
        return false;
    };
    after_name.trim_start().starts_with("not found")
}

enum TabQueryResult {
    Live(String),
    Missing,
    Unresolved,
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
            let tab_output = match self.query_tabs(vec![
                "--session".into(),
                session_name.clone(),
                "action".into(),
                "list-tabs".into(),
                "--json".into(),
            ])? {
                TabQueryResult::Live(output) => output,
                // Sessions can disappear between list and tab queries. Some Zellij versions
                // also include resumable exited sessions in the short listing.
                TabQueryResult::Missing => continue,
                // Permissions, configuration, and other transient failures are not evidence
                // that an identity is stale, so preserve it for a later reconciliation.
                TabQueryResult::Unresolved => {
                    state.mark_unresolved(session_name.clone());
                    continue;
                }
            };
            let tabs: Vec<TabRecord> = serde_json::from_str(&tab_output).map_err(|error| {
                ReconcileError::InvalidTabJson {
                    session_name: session_name.clone(),
                    message: error.to_string(),
                }
            })?;
            state.insert_tabs(
                session_name.clone(),
                tabs.into_iter().map(|tab| (tab.tab_id, tab.name)),
            );
        }
        Ok(state)
    }
}

#[derive(Debug, Deserialize)]
struct TabRecord {
    tab_id: u32,
    #[serde(default)]
    name: String,
}

pub fn reconcile_model(model: &mut BrowserModel, live: &LiveZellijState) {
    reconcile_model_inner(model, live, true);
}

pub fn reconcile_model_preserving_runtime(model: &mut BrowserModel, live: &LiveZellijState) {
    reconcile_model_inner(model, live, false);
}

fn reconcile_model_inner(
    model: &mut BrowserModel,
    live: &LiveZellijState,
    reset_runtime_state: bool,
) {
    let selected_before = model.selected_project.clone();
    let mut remapped_keys = HashMap::new();
    let mut claimed_keys = HashSet::new();
    let mut reconciled_projects = Vec::with_capacity(model.projects.len());

    for mut project in std::mem::take(&mut model.projects) {
        let old_key = project.key.clone();
        let target = match &old_key {
            ProjectKey::Standalone { .. } => Some((old_key.clone(), None)),
            ProjectKey::Zellij { session_name, .. } if live.is_unresolved(session_name) => {
                Some((old_key.clone(), None))
            }
            ProjectKey::Zellij {
                session_name,
                stable_tab_id,
            } => {
                if let Some((restored_tab_id, live_name)) =
                    live.restored_tab(session_name, project.raw_tab_name.as_deref())
                {
                    Some((
                        ProjectKey::Zellij {
                            session_name: session_name.clone(),
                            stable_tab_id: restored_tab_id,
                        },
                        Some(live_name.to_owned()),
                    ))
                } else if live.contains(session_name, *stable_tab_id) {
                    let live_name = live
                        .tab_name(session_name, *stable_tab_id)
                        .unwrap_or_default();
                    let persisted_id = project.raw_tab_name.as_deref().and_then(super_tabs_id);
                    let live_id = super_tabs_id(live_name);
                    (persisted_id.is_none() || persisted_id == live_id)
                        .then(|| (old_key.clone(), Some(live_name.to_owned())))
                } else {
                    None
                }
            }
        };
        let Some((target_key, live_name)) = target else {
            continue;
        };
        if !claimed_keys.insert(target_key.clone()) {
            continue;
        }

        if let Some(live_name) = live_name.filter(|name| !name.is_empty()) {
            let stable_tab_id = match &target_key {
                ProjectKey::Zellij { stable_tab_id, .. } => *stable_tab_id,
                ProjectKey::Standalone { .. } => unreachable!(),
            };
            project.label = project_label(&live_name, stable_tab_id);
            project.raw_tab_name = Some(live_name);
        }
        project.key.clone_from(&target_key);
        for document in &mut project.documents {
            document.key.project.clone_from(&target_key);
            if reset_runtime_state {
                document.lifecycle = DocumentLifecycle::Dormant;
                document.load_error = None;
            }
        }
        remapped_keys.insert(old_key, target_key);
        reconciled_projects.push(project);
    }

    model.projects = reconciled_projects;
    model.selected_project =
        selected_before.and_then(|selected| remapped_keys.get(&selected).cloned());
}

pub fn reconcile_from_source<S: ZellijStateSource>(
    model: &mut BrowserModel,
    source: &S,
) -> Result<(), S::Error> {
    reconcile_from_source_inner(model, source, true)
}

pub fn reconcile_from_source_preserving_runtime<S: ZellijStateSource>(
    model: &mut BrowserModel,
    source: &S,
) -> Result<(), S::Error> {
    reconcile_from_source_inner(model, source, false)
}

fn reconcile_from_source_inner<S: ZellijStateSource>(
    model: &mut BrowserModel,
    source: &S,
    reset_runtime_state: bool,
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
    reconcile_model_inner(model, &live, reset_runtime_state);
    Ok(())
}
