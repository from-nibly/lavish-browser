use lavish_browser_core::persistence::{
    CoalescingStateStore, LoadIssue, MetadataStore, STATE_SCHEMA_VERSION,
};
use lavish_browser_core::{
    BrowserModel, BrowserStateStore, DocumentKey, DocumentLifecycle, ProjectKey,
};
use lavish_browser_protocol::ProjectMetadata;
use std::cell::Cell;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

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
    saves: Cell<usize>,
}

impl BrowserStateStore for CountingStore {
    type Error = ();

    fn save(&self, _model: &BrowserModel) -> Result<(), Self::Error> {
        self.saves.set(self.saves.get() + 1);
        Ok(())
    }

    fn load(&self) -> Result<BrowserModel, Self::Error> {
        Ok(BrowserModel::default())
    }
}

#[test]
fn duplicate_pending_state_is_coalesced() {
    let store = CoalescingStateStore::new(CountingStore {
        saves: Cell::new(0),
    });
    let first = populated_model();
    store.save(&first).unwrap();
    store.save(&first).unwrap();
    assert_eq!(store.into_inner().saves.get(), 1);
}
