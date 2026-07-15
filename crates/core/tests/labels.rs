use lavish_browser_core::labels::{decode_super_tabs_name, project_label};

#[test]
fn decodes_escaped_metadata() {
    let raw =
        r#"__super_tabs_id="st-7-1" | directory="lavish \"browser\"" | worktree="feature\\one""#;
    let decoded = decode_super_tabs_name(raw).unwrap();
    assert_eq!(decoded["directory"], "lavish \"browser\"");
    assert_eq!(decoded["worktree"], "feature\\one");
}

#[test]
fn labels_directory_and_distinct_worktree() {
    let raw = r#"__super_tabs_id="st" | worktree="auth" | directory="api" | agent="BUSY""#;
    assert_eq!(project_label(raw, 7), "api · auth");
    let same = r#"__super_tabs_id="st" | directory="api" | worktree="api""#;
    assert_eq!(project_label(same, 7), "api");
}

#[test]
fn labels_single_identity_manual_and_status_only_tabs() {
    assert_eq!(
        project_label(r#"__super_tabs_id="st" | worktree="review""#, 3),
        "review"
    );
    assert_eq!(project_label("  My manual tab  ", 4), "My manual tab");
    assert_eq!(
        project_label(r#"__super_tabs_id="st" | cmd="cargo" | agent="IDLE""#, 9),
        "Zellij tab 9"
    );
}

#[test]
fn malformed_managed_names_are_treated_as_manual_text() {
    let raw = r#"__super_tabs_id="unterminated"#;
    assert!(decode_super_tabs_name(raw).is_none());
    assert_eq!(project_label(raw, 2), raw);
}
