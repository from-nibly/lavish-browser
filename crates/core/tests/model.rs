use lavish_browser_core::{BrowserModel, DocumentKey, DocumentLifecycle, ProjectKey};
use lavish_browser_protocol::ProjectMetadata;

fn zellij(tab: u32) -> ProjectMetadata {
    ProjectMetadata {
        key: ProjectKey::Zellij {
            session_name: "main".into(),
            stable_tab_id: tab,
        },
        label: format!("tab-{tab}"),
        raw_tab_name: None,
    }
}

fn key(tab: u32) -> ProjectKey {
    zellij(tab).key
}

fn document(tab: u32, source: &str) -> DocumentKey {
    DocumentKey {
        project: key(tab),
        canonical_source_file: source.into(),
    }
}

#[test]
fn projects_are_lazy_and_same_project_open_deduplicates() {
    let mut model = BrowserModel::default();
    assert!(model.projects.is_empty());
    let first = model.open_url(zellij(1), "/tmp/a.html", "http://localhost/session/a", 1);
    let second = model.open_url(zellij(1), "/tmp/a.html", "http://localhost/session/a", 2);
    assert!(first.project_created && first.document_created);
    assert!(!second.project_created && !second.document_created);
    assert_eq!(model.projects.len(), 1);
    assert_eq!(model.projects[0].documents.len(), 1);
    assert_eq!(model.projects[0].documents[0].last_activated_at, 2);
}

#[test]
fn same_source_can_exist_across_projects_and_url_refresh_is_global() {
    let mut model = BrowserModel::default();
    model.open_url(zellij(1), "/tmp/a.html", "http://localhost:1/session/a", 1);
    model.open_url(zellij(2), "/tmp/a.html", "http://localhost:1/session/a", 2);
    model.set_document_lifecycle(&document(1, "/tmp/a.html"), DocumentLifecycle::Ready, None);
    let outcome = model.open_url(zellij(2), "/tmp/a.html", "http://localhost:2/session/a", 3);
    assert_eq!(model.projects.len(), 2);
    assert_eq!(outcome.refreshed_documents, 2);
    for item in model.projects.iter().flat_map(|project| &project.documents) {
        assert_eq!(item.lavish_url, "http://localhost:2/session/a");
        assert_eq!(item.lifecycle, DocumentLifecycle::Dormant);
    }
}

#[test]
fn unknown_lifecycle_commands_are_no_ops() {
    let mut model = BrowserModel::default();
    model.open_url(zellij(1), "/tmp/a", "http://localhost/session/a", 1);
    let before = model.clone();
    assert!(!model.select_project(&key(99), 2));
    assert!(!model.close_project(&key(99)));
    assert!(!model.close_document(&document(1, "/tmp/missing")));
    assert!(!model.set_document_lifecycle(
        &document(1, "/tmp/missing"),
        DocumentLifecycle::Failed,
        Some("nope".into())
    ));
    assert_eq!(model, before);
}

#[test]
fn close_uses_deterministic_document_and_project_fallbacks() {
    let mut model = BrowserModel::default();
    model.open_url(zellij(1), "/tmp/a", "http://localhost/session/a", 1);
    model.open_url(zellij(1), "/tmp/b", "http://localhost/session/b", 2);
    model.open_url(zellij(1), "/tmp/c", "http://localhost/session/c", 3);
    assert!(model.close_document(&document(1, "/tmp/b")));
    assert_eq!(
        model.projects[0].selected_document.as_deref(),
        Some("/tmp/c")
    );

    model.open_url(zellij(2), "/tmp/d", "http://localhost/session/d", 4);
    model.open_url(zellij(3), "/tmp/e", "http://localhost/session/e", 5);
    assert!(model.select_project(&key(2), 6));
    assert!(model.close_project(&key(2)));
    assert_eq!(model.selected_project, Some(key(3)));
    assert_eq!(
        model
            .projects
            .iter()
            .map(|p| p.created_at)
            .collect::<Vec<_>>(),
        vec![1, 5]
    );
}

#[test]
fn inactive_documents_are_ordered_least_recently_used_and_exclude_active() {
    let mut model = BrowserModel::default();
    model.open_url(zellij(1), "/tmp/a", "http://localhost/session/a", 10);
    model.open_url(zellij(1), "/tmp/b", "http://localhost/session/b", 20);
    model.open_url(zellij(2), "/tmp/c", "http://localhost/session/c", 30);
    model.select_document(&document(1, "/tmp/a"), 40);
    let lru = model
        .inactive_documents_lru()
        .into_iter()
        .map(|item| item.key.canonical_source_file.as_str())
        .collect::<Vec<_>>();
    assert_eq!(lru, vec!["/tmp/b", "/tmp/c"]);
}

#[test]
fn lifecycle_states_and_errors_are_model_only() {
    let mut model = BrowserModel::default();
    model.open_url(zellij(1), "/tmp/a", "http://localhost/session/a", 1);
    for state in [
        DocumentLifecycle::Loading,
        DocumentLifecycle::Ready,
        DocumentLifecycle::Failed,
        DocumentLifecycle::Suspended,
        DocumentLifecycle::Dormant,
    ] {
        let error = (state == DocumentLifecycle::Failed).then(|| "load failed".into());
        assert!(model.set_document_lifecycle(&document(1, "/tmp/a"), state.clone(), error));
        assert_eq!(model.projects[0].documents[0].lifecycle, state);
    }
}
