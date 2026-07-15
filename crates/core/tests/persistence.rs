use lavish_browser_core::persistence::{
    CoalescingStateStore, LoadIssue, MetadataStore, STATE_SCHEMA_VERSION,
};
use lavish_browser_core::{
    BrowserModel, BrowserStateStore, DocumentKey, DocumentLifecycle, ProjectKey,
};
use lavish_browser_protocol::ProjectMetadata;
use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "lavish-browser-persistence-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn project(key: ProjectKey, label: &str) -> ProjectMetadata {
    ProjectMetadata {
        key,
        label: label.into(),
        raw_tab_name: Some("untrusted raw tab name".into()),
    }
}

fn populated_model() -> BrowserModel {
    let mut model = BrowserModel::default();
    let zellij = ProjectKey::Zellij {
        session_name: "main".into(),
        stable_tab_id: 7,
    };
    model.open_url(
        project(zellij.clone(), "repo · worktree"),
        "/tmp/report.html",
        "http://127.0.0.1:4387/session/redacted",
        11,
    );
    model.set_document_lifecycle(
        &DocumentKey {
            project: zellij,
            canonical_source_file: "/tmp/report.html".into(),
        },
        DocumentLifecycle::Failed,
        Some("runtime-only failure".into()),
    );
    model.open_url(
        project(
            ProjectKey::Standalone {
                label: "Standalone".into(),
            },
            "Standalone",
        ),
        "/tmp/plan.html",
        "http://localhost:4387/session/redacted2",
        12,
    );
    model
}

#[test]
fn atomic_round_trip_uses_xdg_path_permissions_and_dormant_documents() {
    let temporary = TestDirectory::new();
    let store = MetadataStore::from_environment(|name| {
        (name == "XDG_STATE_HOME").then(|| temporary.0.join("state-root"))
    })
    .unwrap();
    let model = populated_model();

    store.save(&model).unwrap();
    let persisted = fs::read_to_string(store.state_file()).unwrap();
    assert!(persisted.contains(&format!("\"schema_version\": {STATE_SCHEMA_VERSION}")));
    assert!(persisted.contains("canonical_source_file"));
    assert!(!persisted.contains("lifecycle"));
    assert!(!persisted.contains("load_error"));
    assert!(!persisted.contains("runtime-only failure"));
    assert!(!persisted.contains("annotation"));
    assert!(!persisted.contains("whiteboard"));
    assert!(
        store
            .state_file()
            .parent()
            .unwrap()
            .read_dir()
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".tmp-"))
    );

    let loaded = store.load().unwrap();
    assert_eq!(loaded.projects.len(), 2);
    assert!(
        loaded
            .projects
            .iter()
            .flat_map(|project| &project.documents)
            .all(|document| document.lifecycle == DocumentLifecycle::Dormant
                && document.load_error.is_none())
    );
    assert_eq!(loaded.selected_project, model.selected_project);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(store.state_file())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(store.state_file().parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}

#[test]
fn a_second_save_atomically_replaces_the_complete_document() {
    let temporary = TestDirectory::new();
    let store = MetadataStore::new(&temporary.0);
    let first = populated_model();
    store.save(&first).unwrap();

    let mut second = BrowserModel::default();
    second.open_url(
        project(
            ProjectKey::Standalone {
                label: "replacement".into(),
            },
            "replacement",
        ),
        "/tmp/new.html",
        "http://localhost/session/new",
        99,
    );
    store.save(&second).unwrap();

    assert_eq!(store.load().unwrap(), second);
    assert!(
        !fs::read_to_string(store.state_file())
            .unwrap()
            .contains("report.html")
    );
}

#[test]
fn corrupt_and_unknown_schema_files_are_preserved_and_load_empty() {
    let temporary = TestDirectory::new();
    let store = MetadataStore::new(&temporary.0);
    fs::write(store.state_file(), b"{\"schema_version\":1,\"projects\":").unwrap();
    let corrupt_bytes = fs::read(store.state_file()).unwrap();

    let report = store.load_with_report().unwrap();
    assert!(report.model.projects.is_empty());
    assert!(matches!(report.issue, Some(LoadIssue::Corrupt(_))));
    assert_eq!(fs::read(store.state_file()).unwrap(), corrupt_bytes);

    let future = br#"{"schema_version":999,"projects":[],"selected_project":null}"#;
    fs::write(store.state_file(), future).unwrap();
    let report = store.load_with_report().unwrap();
    assert_eq!(report.issue, Some(LoadIssue::UnknownSchemaVersion(999)));
    assert!(report.model.projects.is_empty());
    assert_eq!(fs::read(store.state_file()).unwrap(), future);
}

#[test]
fn untrusted_persisted_session_urls_are_rejected_and_reported() {
    let temporary = TestDirectory::new();
    let store = MetadataStore::new(&temporary.0);
    store.save(&populated_model()).unwrap();
    let mut state: serde_json::Value =
        serde_json::from_slice(&fs::read(store.state_file()).unwrap()).unwrap();
    let project = &mut state["projects"][0];
    let documents = project["documents"].as_array_mut().unwrap();
    for (source, url) in [
        ("/tmp/remote.html", "https://example.com/session/x"),
        (
            "/tmp/credentials.html",
            "http://user:secret@localhost/session/x",
        ),
        (
            "/tmp/deceptive.html",
            "http://localhost.evil.example/session/x",
        ),
        ("/tmp/malformed.html", "not a url"),
        ("/tmp/path.html", "http://127.0.0.1/not-session/x"),
    ] {
        documents.push(serde_json::json!({
            "canonical_source_file": source,
            "lavish_url": url,
            "title": null,
            "last_activated_at": 50
        }));
    }
    project["selected_document"] = serde_json::json!("/tmp/credentials.html");
    fs::write(
        store.state_file(),
        serde_json::to_vec_pretty(&state).unwrap(),
    )
    .unwrap();

    let report = store.load_with_report().unwrap();

    assert_eq!(
        report.issue,
        Some(LoadIssue::InvalidSessionUrls { rejected: 5 })
    );
    let restored = &report.model.projects[0];
    assert_eq!(restored.documents.len(), 1);
    assert_eq!(
        restored.documents[0].key.canonical_source_file,
        "/tmp/report.html"
    );
    assert_eq!(restored.selected_document, None);
}

#[test]
fn xdg_falls_back_to_home_and_missing_environment_is_reported() {
    let store = MetadataStore::from_environment(|name| {
        (name == "HOME").then(|| PathBuf::from("/home/example"))
    })
    .unwrap();
    assert_eq!(
        store.state_file(),
        PathBuf::from("/home/example/.local/state/lavish-browser/state.json")
    );
    assert!(MetadataStore::from_environment(|_| None).is_err());
}

struct CountingStore {
    saves: RefCell<Vec<BrowserModel>>,
}

impl BrowserStateStore for CountingStore {
    type Error = ();

    fn save(&self, model: &BrowserModel) -> Result<(), Self::Error> {
        self.saves.borrow_mut().push(model.clone());
        Ok(())
    }

    fn load(&self) -> Result<BrowserModel, Self::Error> {
        Ok(BrowserModel::default())
    }
}

#[test]
fn distinct_burst_mutations_write_only_the_final_state_after_the_window() {
    let store = CoalescingStateStore::new(
        CountingStore {
            saves: RefCell::new(Vec::new()),
        },
        Duration::from_millis(10),
    );
    let mut first = populated_model();
    let mut second = first.clone();
    second.projects[0].label = "second mutation".into();
    let mut final_state = second.clone();
    final_state.projects[0].label = "final mutation".into();

    store.queue_save(&first, Duration::ZERO);
    first.projects[0].label = "changed after queue".into();
    store.queue_save(&second, Duration::from_millis(3));
    store.queue_save(&final_state, Duration::from_millis(5));

    assert!(!store.flush_due(Duration::from_millis(14)).unwrap());
    assert!(store.has_pending_save());
    assert!(store.flush_due(Duration::from_millis(15)).unwrap());
    assert!(!store.has_pending_save());
    assert!(!store.flush_due(Duration::from_millis(100)).unwrap());

    let inner = store.into_inner();
    assert_eq!(inner.saves.into_inner(), vec![final_state]);
}
