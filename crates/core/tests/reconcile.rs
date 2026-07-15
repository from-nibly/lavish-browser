use lavish_browser_core::reconcile::{
    CommandOutput, CommandRunner, LiveZellijState, ZellijCommandState, ZellijStateSource,
    reconcile_from_source, reconcile_model,
};
use lavish_browser_core::{BrowserModel, DocumentKey, DocumentLifecycle, ProjectKey};
use lavish_browser_protocol::ProjectMetadata;
use std::collections::{HashSet, VecDeque};
use std::io;
use std::sync::Mutex;

fn metadata(key: ProjectKey, label: &str) -> ProjectMetadata {
    ProjectMetadata {
        key,
        label: label.into(),
        raw_tab_name: Some(format!("raw {label}")),
    }
}

fn zellij(session_name: &str, stable_tab_id: u32) -> ProjectKey {
    ProjectKey::Zellij {
        session_name: session_name.into(),
        stable_tab_id,
    }
}

fn add(model: &mut BrowserModel, key: ProjectKey, source: &str, timestamp: u64) {
    model.open_url(
        metadata(key, source),
        source,
        format!("http://localhost/session/{timestamp}"),
        timestamp,
    );
}

#[test]
fn reconciliation_uses_exact_session_and_stable_id_and_keeps_standalone() {
    let mut model = BrowserModel::default();
    add(&mut model, zellij("main", 4), "/live-main", 1);
    add(&mut model, zellij("other", 4), "/stale-other", 2);
    add(&mut model, zellij("main", 9), "/stale-tab", 3);
    add(
        &mut model,
        ProjectKey::Standalone {
            label: "Standalone".into(),
        },
        "/standalone",
        4,
    );
    model.set_document_lifecycle(
        &DocumentKey {
            project: zellij("main", 4),
            canonical_source_file: "/live-main".into(),
        },
        DocumentLifecycle::Ready,
        Some("runtime state".into()),
    );

    let mut live = LiveZellijState::default();
    live.insert_session("main", [4]);
    live.insert_session("other", [8]);
    reconcile_model(&mut model, &live);

    assert_eq!(
        model
            .projects
            .iter()
            .map(|project| project.key.clone())
            .collect::<Vec<_>>(),
        vec![
            zellij("main", 4),
            ProjectKey::Standalone {
                label: "Standalone".into()
            }
        ]
    );
    assert_eq!(
        model.selected_project,
        Some(ProjectKey::Standalone {
            label: "Standalone".into()
        })
    );
    assert!(
        model
            .projects
            .iter()
            .flat_map(|project| &project.documents)
            .all(|document| document.lifecycle == DocumentLifecycle::Dormant
                && document.load_error.is_none())
    );
}

#[test]
fn a_removed_selected_project_is_not_guessed_from_label_or_title() {
    let mut model = BrowserModel::default();
    add(&mut model, zellij("gone", 1), "/gone", 1);
    add(&mut model, zellij("live", 2), "/live", 2);
    assert!(model.select_project(&zellij("gone", 1), 3));
    let mut live = LiveZellijState::default();
    live.insert_session("live", [2]);

    reconcile_model(&mut model, &live);

    assert_eq!(model.projects.len(), 1);
    assert_eq!(model.projects[0].key, zellij("live", 2));
    assert_eq!(model.selected_project, None);
}

struct StaticSource {
    state: LiveZellijState,
    requested: Mutex<Option<HashSet<String>>>,
}

impl ZellijStateSource for StaticSource {
    type Error = ();

    fn live_state(
        &self,
        relevant_sessions: &HashSet<String>,
    ) -> Result<LiveZellijState, Self::Error> {
        *self.requested.lock().unwrap() = Some(relevant_sessions.clone());
        Ok(self.state.clone())
    }
}

#[test]
fn source_adapter_receives_only_relevant_zellij_session_names() {
    let mut model = BrowserModel::default();
    add(&mut model, zellij("alpha", 1), "/a", 1);
    add(&mut model, zellij("beta", 2), "/b", 2);
    add(
        &mut model,
        ProjectKey::Standalone {
            label: "Standalone".into(),
        },
        "/s",
        3,
    );
    let mut state = LiveZellijState::default();
    state.insert_session("alpha", [1]);
    state.insert_session("beta", [2]);
    let source = StaticSource {
        state,
        requested: Mutex::new(None),
    };

    reconcile_from_source(&mut model, &source).unwrap();

    assert_eq!(
        source.requested.into_inner().unwrap().unwrap(),
        HashSet::from(["alpha".into(), "beta".into()])
    );
}

struct FakeRunner {
    outputs: Mutex<VecDeque<CommandOutput>>,
    calls: Mutex<Vec<Vec<String>>>,
}

impl CommandRunner for FakeRunner {
    fn run(&self, program: &str, arguments: &[String]) -> io::Result<CommandOutput> {
        assert_eq!(program, "zellij");
        self.calls.lock().unwrap().push(arguments.to_vec());
        Ok(self.outputs.lock().unwrap().pop_front().unwrap())
    }
}

fn success(stdout: &str) -> CommandOutput {
    CommandOutput {
        success: true,
        stdout: stdout.as_bytes().to_vec(),
        stderr: Vec::new(),
    }
}

fn failure(stderr: &str) -> CommandOutput {
    CommandOutput {
        success: false,
        stdout: Vec::new(),
        stderr: stderr.as_bytes().to_vec(),
    }
}

#[test]
fn command_adapter_uses_required_commands_and_parses_fixture_json() {
    let runner = FakeRunner {
        outputs: Mutex::new(VecDeque::from([
            success("main\nunrelated\n"),
            success(
                r#"[
                    {"tab_id":7,"name":"mutable label","position":0},
                    {"tab_id":11,"name":"another","position":1}
                ]"#,
            ),
        ])),
        calls: Mutex::new(Vec::new()),
    };
    let adapter = ZellijCommandState::new(runner);
    let state = adapter
        .live_state(&HashSet::from(["main".into(), "offline".into()]))
        .unwrap();

    assert!(state.contains("main", 7));
    assert!(state.contains("main", 11));
    assert!(!state.contains("offline", 7));
    let runner = adapter.into_inner();
    assert_eq!(
        runner.calls.into_inner().unwrap(),
        vec![
            vec!["list-sessions", "--short", "--no-formatting"],
            vec!["--session", "main", "action", "list-tabs", "--json"],
        ]
    );
}

#[test]
fn listed_but_exited_or_raced_session_is_treated_as_not_live() {
    let runner = FakeRunner {
        outputs: Mutex::new(VecDeque::from([
            success("exited\n"),
            failure("Session 'exited' not found"),
        ])),
        calls: Mutex::new(Vec::new()),
    };
    let adapter = ZellijCommandState::new(runner);

    let state = adapter
        .live_state(&HashSet::from(["exited".into()]))
        .unwrap();

    assert!(!state.contains("exited", 1));
}

#[test]
fn malformed_tab_json_is_reported_without_mutating_a_model() {
    let runner = FakeRunner {
        outputs: Mutex::new(VecDeque::from([success("main\n"), success("not json")])),
        calls: Mutex::new(Vec::new()),
    };
    let source = ZellijCommandState::new(runner);
    let mut model = BrowserModel::default();
    add(&mut model, zellij("main", 1), "/a", 1);
    let before = model.clone();

    assert!(reconcile_from_source(&mut model, &source).is_err());
    assert_eq!(model, before);
}
