use lavish_browser_core::{BrowserModel, DocumentKey};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WarningLevel {
    Low,
    Medium,
    Critical,
}

impl WarningLevel {
    pub(crate) fn suspension_limit(self) -> usize {
        match self {
            Self::Low => 1,
            Self::Medium => 2,
            Self::Critical => usize::MAX,
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "low" | "50" => Some(Self::Low),
            "medium" | "100" => Some(Self::Medium),
            "critical" | "255" => Some(Self::Critical),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MaterializationPlan {
    Placeholder,
    Create,
    Reuse,
}

pub(crate) fn materialization_plan(
    project_is_globally_selected: bool,
    document_is_locally_selected: bool,
    already_materialized: bool,
) -> MaterializationPlan {
    if already_materialized {
        MaterializationPlan::Reuse
    } else if project_is_globally_selected && document_is_locally_selected {
        MaterializationPlan::Create
    } else {
        MaterializationPlan::Placeholder
    }
}

pub(crate) fn suspension_candidates(
    model: &BrowserModel,
    warning: WarningLevel,
    mut is_materialized: impl FnMut(&DocumentKey) -> bool,
) -> Vec<DocumentKey> {
    model
        .inactive_documents_lru()
        .into_iter()
        .filter(|document| is_materialized(&document.key))
        .take(warning.suspension_limit())
        .map(|document| document.key.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{MaterializationPlan, WarningLevel, materialization_plan, suspension_candidates};
    use lavish_browser_core::{BrowserModel, DocumentKey};
    use lavish_browser_protocol::{ProjectKey, ProjectMetadata};
    use std::collections::HashSet;

    fn project(tab: u32) -> ProjectMetadata {
        ProjectMetadata {
            key: ProjectKey::Zellij {
                session_name: "main".into(),
                stable_tab_id: tab,
            },
            label: format!("tab-{tab}"),
            raw_tab_name: None,
        }
    }

    fn key(tab: u32, source: &str) -> DocumentKey {
        DocumentKey {
            project: ProjectKey::Zellij {
                session_name: "main".into(),
                stable_tab_id: tab,
            },
            canonical_source_file: source.into(),
        }
    }

    fn populated_model() -> BrowserModel {
        let mut model = BrowserModel::default();
        model.open_url(
            project(1),
            "/tmp/oldest.html",
            "http://localhost/session/a",
            10,
        );
        model.open_url(
            project(1),
            "/tmp/middle.html",
            "http://localhost/session/b",
            20,
        );
        model.open_url(
            project(2),
            "/tmp/active.html",
            "http://localhost/session/c",
            30,
        );
        model
    }

    #[test]
    fn low_warning_suspends_only_oldest_materialized_inactive_document() {
        let model = populated_model();
        let materialized = HashSet::from([
            key(1, "/tmp/oldest.html"),
            key(1, "/tmp/middle.html"),
            key(2, "/tmp/active.html"),
        ]);

        assert_eq!(
            suspension_candidates(&model, WarningLevel::Low, |key| materialized.contains(key)),
            vec![key(1, "/tmp/oldest.html")]
        );
    }

    #[test]
    fn severity_increases_release_without_ever_selecting_active_document() {
        let model = populated_model();
        let materialized = HashSet::from([
            key(1, "/tmp/oldest.html"),
            key(1, "/tmp/middle.html"),
            key(2, "/tmp/active.html"),
        ]);

        assert_eq!(
            suspension_candidates(&model, WarningLevel::Medium, |key| materialized
                .contains(key)),
            vec![key(1, "/tmp/oldest.html"), key(1, "/tmp/middle.html")]
        );
        assert_eq!(
            suspension_candidates(&model, WarningLevel::Critical, |key| materialized
                .contains(key)),
            vec![key(1, "/tmp/oldest.html"), key(1, "/tmp/middle.html")]
        );
    }

    #[test]
    fn dormant_documents_are_skipped_even_when_they_are_oldest() {
        let model = populated_model();
        let materialized = HashSet::from([key(1, "/tmp/middle.html")]);

        assert_eq!(
            suspension_candidates(&model, WarningLevel::Critical, |key| materialized
                .contains(key)),
            vec![key(1, "/tmp/middle.html")]
        );
    }

    fn three_project_model() -> BrowserModel {
        let mut model = BrowserModel::default();
        model.open_url(
            project(1),
            "/tmp/one.html",
            "http://localhost/session/one",
            10,
        );
        model.open_url(
            project(2),
            "/tmp/two.html",
            "http://localhost/session/two",
            20,
        );
        model.open_url(
            project(3),
            "/tmp/three.html",
            "http://localhost/session/three",
            30,
        );
        model
    }

    fn controller_render_plans(
        model: &BrowserModel,
        materialized: &HashSet<DocumentKey>,
    ) -> Vec<(DocumentKey, MaterializationPlan)> {
        model
            .projects
            .iter()
            .flat_map(|project| {
                project.documents.iter().map(|document| {
                    let plan = materialization_plan(
                        model.selected_project.as_ref() == Some(&project.key),
                        project.selected_document.as_deref()
                            == Some(&document.key.canonical_source_file),
                        materialized.contains(&document.key),
                    );
                    (document.key.clone(), plan)
                })
            })
            .collect()
    }

    #[test]
    fn inactive_project_suspended_views_stay_absent_across_controller_renders() {
        let model = three_project_model();
        let materialized = HashSet::from([key(3, "/tmp/three.html")]);
        let expected = vec![
            (key(1, "/tmp/one.html"), MaterializationPlan::Placeholder),
            (key(2, "/tmp/two.html"), MaterializationPlan::Placeholder),
            (key(3, "/tmp/three.html"), MaterializationPlan::Reuse),
        ];

        for _ in 0..3 {
            assert_eq!(controller_render_plans(&model, &materialized), expected);
        }
    }

    #[test]
    fn selecting_suspended_project_creates_exactly_one_view_and_then_reuses_it() {
        let mut model = three_project_model();
        let mut materialized = HashSet::from([key(3, "/tmp/three.html")]);
        assert!(model.select_project(&project(1).key, 40));

        let plans = controller_render_plans(&model, &materialized);
        assert_eq!(
            plans
                .iter()
                .filter(|(_, plan)| *plan == MaterializationPlan::Create)
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>(),
            vec![key(1, "/tmp/one.html")]
        );
        materialized.insert(key(1, "/tmp/one.html"));
        assert!(
            controller_render_plans(&model, &materialized)
                .iter()
                .all(|(_, plan)| *plan != MaterializationPlan::Create)
        );
    }

    #[test]
    fn debug_warning_levels_are_strict_and_deterministic() {
        assert_eq!(WarningLevel::parse("low"), Some(WarningLevel::Low));
        assert_eq!(WarningLevel::parse("100"), Some(WarningLevel::Medium));
        assert_eq!(
            WarningLevel::parse("critical"),
            Some(WarningLevel::Critical)
        );
        assert_eq!(WarningLevel::parse("high"), None);
    }
}
