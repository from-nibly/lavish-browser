use lavish_browser_core::BrowserModel;
use lavish_browser_protocol::{ProjectKey, ProjectMetadata};

fn key(workspace: &str) -> ProjectKey {
    ProjectKey::Herdr {
        session_name: "main".into(),
        workspace_id: workspace.into(),
        tab_id: "same-tab-id".into(),
    }
}

#[test]
fn rename_preserves_identity_documents_selection_and_timestamps() {
    let mut model = BrowserModel::default();
    for workspace in ["one", "two"] {
        model.open_url(
            ProjectMetadata {
                key: key(workspace),
                label: "Old".into(),
                raw_tab_name: None,
            },
            format!("/tmp/{workspace}.html"),
            "http://localhost/session/test",
            42,
        );
    }
    let before = model.clone();
    assert!(model.rename_project(
        &key("one"),
        "Review <日本語>".into(),
        "Review <日本語>".into()
    ));
    let mut expected = before;
    expected.projects[0].label = "Review <日本語>".into();
    expected.projects[0].raw_tab_name = Some("Review <日本語>".into());
    assert_eq!(model, expected);
    assert!(!model.rename_project(
        &key("one"),
        "Review <日本語>".into(),
        "Review <日本語>".into()
    ));
    assert!(!model.rename_project(&key("absent"), "Unknown".into(), "Unknown".into()));
    let wrong_session = ProjectKey::Herdr {
        session_name: "other".into(),
        workspace_id: "one".into(),
        tab_id: "same-tab-id".into(),
    };
    assert!(!model.rename_project(&wrong_session, "Wrong".into(), "Wrong".into()));
    assert_eq!(model, expected);
}
