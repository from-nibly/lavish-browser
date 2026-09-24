use lavish_browser_cli::{
    LavishSession, parse_lavish_toon, project_from_herdr_tabs, project_from_panes,
    upstream_arguments,
};
use lavish_browser_protocol::ProjectKey;

#[test]
fn parses_current_toon_and_rejects_malformed_or_remote_output() {
    let text = "session:\n  file: \"/tmp/review file.html\"\n  url: \"http://[::1]:4387/session/key?noGate=1\"\n  status: opened\nnext_step: ignored\n";
    assert_eq!(
        parse_lavish_toon(text).unwrap(),
        LavishSession {
            file: "/tmp/review file.html".into(),
            url: "http://[::1]:4387/session/key?noGate=1".into(),
            status: "opened".into(),
        }
    );
    assert!(parse_lavish_toon("session:\n  file: /tmp/x\n").is_err());
    assert!(
        parse_lavish_toon(
            "session:\n  file: /tmp/x\n  url: https://example.com/session/x\n  status: opened\n"
        )
        .is_err()
    );
}

#[test]
fn adds_no_open_exactly_once_without_reordering_flags() {
    assert_eq!(
        upstream_arguments(vec!["file.html".into(), "--no-gate".into()]),
        ["-y", "lavish-axi", "file.html", "--no-gate", "--no-open"].map(str::to_owned)
    );
    assert_eq!(
        upstream_arguments(vec![
            "file.html".into(),
            "--no-open".into(),
            "--reopen".into()
        ]),
        ["-y", "lavish-axi", "file.html", "--no-open", "--reopen"].map(str::to_owned)
    );
}

#[test]
fn maps_the_invoking_pane_instead_of_the_active_tab() {
    let panes = br#"[
      {"id": 4, "tab_id": 90, "tab_name": "active but wrong", "is_focused": true},
      {"id": 17, "tab_id": 6, "tab_name": "__super_tabs_id=\"x\" | directory=\"api\" | worktree=\"feature\"", "is_focused": false}
    ]"#;
    let project = project_from_panes("main".into(), 17, panes).unwrap();
    assert_eq!(
        project.key,
        ProjectKey::Zellij {
            session_name: "main".into(),
            stable_tab_id: 6
        }
    );
    assert_eq!(project.label, "api · feature");
}

#[test]
fn maps_the_invoking_herdr_tab_by_stable_string_identity() {
    let tabs = br#"{
      "id":"cli:tab:list",
      "result":{"tabs":[
        {"workspace_id":"w2","tab_id":"w2:t1","label":"wrong"},
        {"workspace_id":"w3","tab_id":"w3:tA","label":"home-manager"}
      ]}
    }"#;
    let project =
        project_from_herdr_tabs("main".into(), "w3".into(), "w3:tA".into(), tabs).unwrap();
    assert_eq!(
        project.key,
        ProjectKey::Herdr {
            session_name: "main".into(),
            workspace_id: "w3".into(),
            tab_id: "w3:tA".into(),
        }
    );
    assert_eq!(project.label, "home-manager");
}
