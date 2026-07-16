//! Optional one-way Zellij lifecycle integration for Lavish Browser.

use std::collections::BTreeSet;

/// A native helper invocation requested by the lifecycle reducer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelperCommand {
    Select { session_name: String, tab_id: u32 },
    Close { session_name: String, tab_id: u32 },
}

impl HelperCommand {
    pub fn argv(&self, helper_path: &str) -> Vec<String> {
        let (operation, session_name, tab_id) = match self {
            Self::Select {
                session_name,
                tab_id,
            } => ("select-project", session_name, tab_id),
            Self::Close {
                session_name,
                tab_id,
            } => ("close-project", session_name, tab_id),
        };
        vec![
            helper_path.to_owned(),
            operation.to_owned(),
            session_name.clone(),
            tab_id.to_string(),
        ]
    }
}

/// Host-independent lifecycle reducer.
///
/// The most recent tab snapshot is cached until both permissions and the
/// Zellij session name are available. Once ready, identical snapshots are
/// ignored so event bursts cannot create duplicate helper processes.
#[derive(Debug, Default)]
pub struct LifecycleReducer {
    permission_granted: bool,
    session_name: Option<String>,
    pending: Option<TabSnapshot>,
    previous_tabs: Option<BTreeSet<u32>>,
    previous_active: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TabSnapshot {
    tabs: BTreeSet<u32>,
    active: Option<u32>,
}

impl LifecycleReducer {
    pub fn set_permission(&mut self, granted: bool) -> Vec<HelperCommand> {
        self.permission_granted = granted;
        if granted { self.flush() } else { Vec::new() }
    }

    pub fn set_session_name(&mut self, session_name: Option<String>) -> Vec<HelperCommand> {
        self.session_name = session_name.filter(|name| !name.is_empty());
        self.flush()
    }

    pub fn observe_tabs<I>(&mut self, tabs: I, active: Option<u32>) -> Vec<HelperCommand>
    where
        I: IntoIterator<Item = u32>,
    {
        let tabs = tabs.into_iter().collect::<BTreeSet<_>>();
        self.pending = Some(TabSnapshot {
            active: active.filter(|tab_id| tabs.contains(tab_id)),
            tabs,
        });
        self.flush()
    }

    fn flush(&mut self) -> Vec<HelperCommand> {
        if !self.permission_granted || self.session_name.is_none() {
            return Vec::new();
        }
        let Some(snapshot) = self.pending.take() else {
            return Vec::new();
        };
        if self.previous_tabs.as_ref() == Some(&snapshot.tabs)
            && self.previous_active == snapshot.active
        {
            return Vec::new();
        }

        let session_name = self.session_name.clone().expect("checked above");
        let mut commands = Vec::new();
        if let Some(previous) = &self.previous_tabs {
            commands.extend(previous.difference(&snapshot.tabs).map(|tab_id| {
                HelperCommand::Close {
                    session_name: session_name.clone(),
                    tab_id: *tab_id,
                }
            }));
        }
        if snapshot.active != self.previous_active
            && let Some(tab_id) = snapshot.active
        {
            commands.push(HelperCommand::Select {
                session_name,
                tab_id,
            });
        }
        self.previous_tabs = Some(snapshot.tabs);
        self.previous_active = snapshot.active;
        commands
    }
}

// `cdylib` targets do not synthesize the WASI command entry point that
// Zellij invokes before calling the plugin exports.
#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn _start() {}

#[cfg(target_arch = "wasm32")]
mod plugin {
    use super::{HelperCommand, LifecycleReducer};
    use std::collections::BTreeMap;
    use zellij_tile::prelude::*;

    #[derive(Default)]
    pub struct Plugin {
        reducer: LifecycleReducer,
        helper_path: String,
        debug: bool,
    }

    register_plugin!(Plugin);

    impl ZellijPlugin for Plugin {
        fn load(&mut self, configuration: BTreeMap<String, String>) {
            self.helper_path = configuration
                .get("helper_path")
                .cloned()
                .unwrap_or_else(|| "lavish-browser-ctl".to_owned());
            self.debug = configuration
                .get("debug")
                .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "yes"));
            if let Ok(session_name) = std::env::var("ZELLIJ_SESSION_NAME") {
                self.reducer.set_session_name(Some(session_name));
            }
            request_permission(&[
                PermissionType::ReadApplicationState,
                PermissionType::RunCommands,
            ]);
            subscribe(&[
                EventType::PermissionRequestResult,
                EventType::ModeUpdate,
                EventType::TabUpdate,
            ]);
        }

        fn update(&mut self, event: Event) -> bool {
            if self.debug {
                self.trace_event(&event);
            }
            let commands = match event {
                Event::PermissionRequestResult(status) => {
                    if status == PermissionStatus::Granted {
                        set_selectable(false);
                    }
                    self.reducer
                        .set_permission(status == PermissionStatus::Granted)
                }
                Event::ModeUpdate(mode) => self.reducer.set_session_name(mode.session_name),
                Event::TabUpdate(tabs) => {
                    let active = tabs
                        .iter()
                        .find(|tab| tab.active)
                        .and_then(|tab| u32::try_from(tab.tab_id).ok());
                    self.reducer.observe_tabs(
                        tabs.into_iter()
                            .filter_map(|tab| u32::try_from(tab.tab_id).ok()),
                        active,
                    )
                }
                _ => Vec::new(),
            };
            for command in commands {
                self.run_helper(command);
            }
            false
        }
    }

    impl Plugin {
        fn trace_event(&self, event: &Event) {
            let detail = match event {
                Event::PermissionRequestResult(status) => format!("permission={status:?}"),
                Event::ModeUpdate(mode) => format!("mode session={:?}", mode.session_name),
                Event::TabUpdate(tabs) => format!(
                    "tabs={:?}",
                    tabs.iter()
                        .map(|tab| (tab.tab_id, tab.active, tab.name.as_str()))
                        .collect::<Vec<_>>()
                ),
                _ => return,
            };
            run_command(
                &[&self.helper_path, "trace-plugin-event", &detail],
                BTreeMap::new(),
            );
        }

        fn run_helper(&self, command: HelperCommand) {
            let argv = command.argv(&self.helper_path);
            if self.debug {
                eprintln!("lavish-browser-zellij: {}", argv[1..].join(" "));
            }
            run_command(&[&argv[0], &argv[1], &argv[2], &argv[3]], BTreeMap::new());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready(reducer: &mut LifecycleReducer) {
        assert!(reducer.set_session_name(Some("dev".into())).is_empty());
        assert!(reducer.set_permission(true).is_empty());
    }

    #[test]
    fn caches_latest_snapshot_until_ready() {
        let mut reducer = LifecycleReducer::default();
        assert!(reducer.observe_tabs([1, 2], Some(1)).is_empty());
        assert!(reducer.observe_tabs([1, 2], Some(2)).is_empty());
        assert!(reducer.set_permission(true).is_empty());
        assert_eq!(
            reducer.set_session_name(Some("dev".into())),
            vec![HelperCommand::Select {
                session_name: "dev".into(),
                tab_id: 2,
            }]
        );
    }

    #[test]
    fn unchanged_events_are_coalesced() {
        let mut reducer = LifecycleReducer::default();
        ready(&mut reducer);
        assert_eq!(reducer.observe_tabs([1, 2], Some(1)).len(), 1);
        assert!(reducer.observe_tabs([1, 2], Some(1)).is_empty());
    }

    #[test]
    fn readiness_updates_do_not_reset_active_event_coalescing() {
        let mut reducer = LifecycleReducer::default();
        reducer.observe_tabs([7, 8], Some(7));
        reducer.set_permission(true);
        assert_eq!(
            reducer.set_session_name(Some("dev".into())),
            vec![HelperCommand::Select {
                session_name: "dev".into(),
                tab_id: 7,
            }]
        );
        assert!(reducer.set_session_name(Some("dev".into())).is_empty());
        assert!(reducer.observe_tabs([7, 8], Some(7)).is_empty());
    }

    #[test]
    fn active_switch_emits_one_select() {
        let mut reducer = LifecycleReducer::default();
        ready(&mut reducer);
        reducer.observe_tabs([1, 2], Some(1));
        assert_eq!(
            reducer.observe_tabs([1, 2], Some(2)),
            vec![HelperCommand::Select {
                session_name: "dev".into(),
                tab_id: 2,
            }]
        );
    }

    #[test]
    fn removed_tabs_emit_exactly_one_close_each() {
        let mut reducer = LifecycleReducer::default();
        ready(&mut reducer);
        reducer.observe_tabs([1, 2, 3], Some(1));
        assert_eq!(
            reducer.observe_tabs([1], Some(1)),
            vec![
                HelperCommand::Close {
                    session_name: "dev".into(),
                    tab_id: 2,
                },
                HelperCommand::Close {
                    session_name: "dev".into(),
                    tab_id: 3,
                },
            ]
        );
        assert!(reducer.observe_tabs([1], Some(1)).is_empty());
    }

    #[test]
    fn permission_denial_is_quiet_and_recovery_uses_latest_state() {
        let mut reducer = LifecycleReducer::default();
        reducer.set_session_name(Some("dev".into()));
        reducer.set_permission(false);
        assert!(reducer.observe_tabs([7], Some(7)).is_empty());
        assert_eq!(reducer.set_permission(true).len(), 1);
    }

    #[test]
    fn invalid_active_id_is_not_dispatched() {
        let mut reducer = LifecycleReducer::default();
        ready(&mut reducer);
        assert!(reducer.observe_tabs([1], Some(9)).is_empty());
    }

    #[test]
    fn helper_arguments_are_distinct_and_match_control_cli() {
        assert_eq!(
            HelperCommand::Select {
                session_name: "name with spaces".into(),
                tab_id: 42,
            }
            .argv("/opt/lavish browser/ctl"),
            [
                "/opt/lavish browser/ctl",
                "select-project",
                "name with spaces",
                "42",
            ]
        );
    }
}
