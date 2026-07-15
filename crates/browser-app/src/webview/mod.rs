use gtk::gio;
use gtk::prelude::*;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::rc::Rc;
use url::Url;
use webkit6::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentEvent {
    Loading,
    Ready(Option<String>),
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefreshAction {
    Keep,
    Navigate,
    Reload,
}

pub(crate) fn refresh_action(
    current_url: &str,
    submitted_url: &str,
    failed: bool,
) -> RefreshAction {
    if current_url != submitted_url {
        RefreshAction::Navigate
    } else if failed {
        RefreshAction::Reload
    } else {
        RefreshAction::Keep
    }
}

#[derive(Debug, Default)]
struct LoadTracker {
    failed: bool,
}

impl LoadTracker {
    fn begin_navigation(&mut self) {
        self.failed = false;
    }

    fn started(&self) -> Option<DocumentEvent> {
        (!self.failed).then_some(DocumentEvent::Loading)
    }

    fn committed(&self) -> Option<DocumentEvent> {
        (!self.failed).then_some(DocumentEvent::Loading)
    }

    fn failed(&mut self, message: String) -> DocumentEvent {
        self.failed = true;
        DocumentEvent::Failed(message)
    }

    fn finished(
        &mut self,
        status: Option<u32>,
        title: Option<String>,
        uri: &str,
    ) -> Option<DocumentEvent> {
        if self.failed {
            return None;
        }
        if status.is_some_and(|status| (200..400).contains(&status)) {
            Some(DocumentEvent::Ready(title))
        } else {
            self.failed = true;
            Some(DocumentEvent::Failed(match status {
                Some(status) => format!("HTTP {status} loading {uri}"),
                None => format!("No HTTP response loading {uri}"),
            }))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DownloadStatus<'a> {
    AwaitingDestination,
    Started(&'a str),
    Progress(u8),
    Failed(&'a str),
    Cancelled,
    Finished,
}

fn download_status_message(status: DownloadStatus<'_>) -> String {
    match status {
        DownloadStatus::AwaitingDestination => "Choose a destination for the download".into(),
        DownloadStatus::Started(destination) => format!("Download started: {destination}"),
        DownloadStatus::Progress(percent) => format!("Download progress: {percent}%"),
        DownloadStatus::Failed(error) => format!("Download failed: {error}"),
        DownloadStatus::Cancelled => "Download cancelled".into(),
        DownloadStatus::Finished => "Download finished".into(),
    }
}

fn show_download_status(label: &gtk::Label, status: DownloadStatus<'_>) {
    label.set_text(&download_status_message(status));
    label.set_visible(true);
}

pub(crate) struct RetainedRegistry<K, V> {
    values: HashMap<K, V>,
}

impl<K: Eq + Hash, V> Default for RetainedRegistry<K, V> {
    fn default() -> Self {
        Self {
            values: HashMap::new(),
        }
    }
}

impl<K: Eq + Hash, V> RetainedRegistry<K, V> {
    pub(crate) fn get(&self, key: &K) -> Option<&V> {
        self.values.get(key)
    }

    pub(crate) fn contains_key(&self, key: &K) -> bool {
        self.values.contains_key(key)
    }

    pub(crate) fn insert(&mut self, key: K, value: V) {
        self.values.insert(key, value);
    }

    pub(crate) fn retain_keys(&mut self, live: &HashSet<K>) -> usize {
        let before = self.values.len();
        self.values.retain(|key, _| live.contains(key));
        before - self.values.len()
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.values.len()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NavigationOutcome {
    Allow,
    External,
    Reject,
}

pub struct DocumentView {
    root: gtk::Box,
    webview: webkit6::WebView,
    status: gtk::Label,
    loaded_url: Rc<RefCell<String>>,
    load_tracker: Rc<RefCell<LoadTracker>>,
}

impl DocumentView {
    pub fn new(
        url: &str,
        identity: &str,
        display_name: &str,
        _window: &gtk::ApplicationWindow,
        emit: Rc<dyn Fn(DocumentEvent)>,
    ) -> Rc<Self> {
        let webview = webkit6::WebView::new();
        let load_tracker = Rc::new(RefCell::new(LoadTracker::default()));
        webview.set_hexpand(true);
        webview.set_vexpand(true);
        webview.set_widget_name(&format!("document-webview-{identity}"));
        webview.update_property(&[gtk::accessible::Property::Label(&format!(
            "Lavish document {display_name}"
        ))]);

        let status = gtk::Label::new(Some("Connecting to upstream Lavish…"));
        status.set_xalign(0.0);
        status.add_css_class("dim-label");
        status.set_widget_name(&format!("document-status-{identity}"));
        status.update_property(&[gtk::accessible::Property::Label(&format!(
            "Load status for document {display_name}"
        ))]);

        let content = gtk::Stack::new();
        content.set_hexpand(true);
        content.set_vexpand(true);
        content.add_named(&webview, Some("web-content"));

        let failure = gtk::Box::new(gtk::Orientation::Vertical, 12);
        failure.set_halign(gtk::Align::Center);
        failure.set_valign(gtk::Align::Center);
        failure.set_widget_name(&format!("document-reconnect-placeholder-{identity}"));
        failure.update_property(&[gtk::accessible::Property::Label(&format!(
            "Reconnect document {display_name}"
        ))]);
        let failure_message = gtk::Label::new(Some("The Lavish session is unavailable."));
        failure_message.set_widget_name(&format!("document-failure-message-{identity}"));
        let retry = gtk::Button::with_label("Reconnect");
        retry.set_widget_name(&format!("document-reconnect-{identity}"));
        retry.update_property(&[gtk::accessible::Property::Label(&format!(
            "Reconnect document {display_name}"
        ))]);
        {
            let webview = webview.downgrade();
            let content = content.downgrade();
            let load_tracker = load_tracker.clone();
            retry.connect_clicked(move |_| {
                load_tracker.borrow_mut().begin_navigation();
                if let Some(content) = content.upgrade() {
                    content.set_visible_child_name("web-content");
                }
                if let Some(webview) = webview.upgrade() {
                    webview.reload();
                }
            });
        }
        failure.append(&failure_message);
        failure.append(&retry);
        content.add_named(&failure, Some("reconnect"));

        let reload = gtk::Button::with_label("Reload");
        reload.set_widget_name(&format!("document-reload-{identity}"));
        reload.update_property(&[gtk::accessible::Property::Label(&format!(
            "Reload document {display_name}"
        ))]);
        {
            let webview = webview.downgrade();
            let content = content.downgrade();
            let load_tracker = load_tracker.clone();
            reload.connect_clicked(move |_| {
                load_tracker.borrow_mut().begin_navigation();
                if let Some(content) = content.upgrade() {
                    content.set_visible_child_name("web-content");
                }
                if let Some(webview) = webview.upgrade() {
                    webview.reload();
                }
            });
        }

        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        toolbar.set_margin_top(6);
        toolbar.set_margin_bottom(6);
        toolbar.set_margin_start(8);
        toolbar.set_margin_end(8);
        toolbar.append(&reload);
        toolbar.append(&status);

        #[cfg(debug_assertions)]
        if std::env::var_os("LAVISH_BROWSER_INSPECTOR").is_some() {
            if let Some(settings) = webkit6::prelude::WebViewExt::settings(&webview) {
                settings.set_enable_developer_extras(true);
            }
            let inspector = gtk::Button::with_label("Inspector");
            inspector.set_widget_name(&format!("document-inspector-{identity}"));
            inspector.update_property(&[gtk::accessible::Property::Label(&format!(
                "Inspect document {display_name}"
            ))]);
            let inspect_view = webview.clone();
            inspector.connect_clicked(move |_| {
                if let Some(inspector) = inspect_view.inspector() {
                    inspector.show();
                }
            });
            toolbar.prepend(&inspector);
        }

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.set_widget_name(&format!("document-surface-{identity}"));
        root.append(&toolbar);
        root.append(&content);

        let controller = Rc::new(Self {
            root,
            webview,
            status,
            loaded_url: Rc::new(RefCell::new(url.to_owned())),
            load_tracker,
        });
        controller.connect_signals(&content, &failure_message, emit);
        controller.load_tracker.borrow_mut().begin_navigation();
        controller.webview.load_uri(url);
        controller
    }

    pub fn widget(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    pub fn navigate(&self, url: &str) {
        if self.loaded_url.borrow().as_str() != url {
            self.loaded_url.replace(url.to_owned());
            self.load_tracker.borrow_mut().begin_navigation();
            self.status
                .set_text("Connecting to refreshed Lavish session…");
            self.webview.load_uri(url);
        }
    }

    pub fn reload(&self) {
        self.load_tracker.borrow_mut().begin_navigation();
        self.status.set_text("Reconnecting to upstream Lavish…");
        eprintln!("reloading retained Lavish document");
        self.webview.reload();
    }

    pub fn loaded_url(&self) -> String {
        self.loaded_url.borrow().clone()
    }

    fn connect_signals(
        self: &Rc<Self>,
        content: &gtk::Stack,
        failure_message: &gtk::Label,
        emit: Rc<dyn Fn(DocumentEvent)>,
    ) {
        let load_tracker = self.load_tracker.clone();
        {
            let status = self.status.clone();
            let content = content.downgrade();
            let failure_message = failure_message.downgrade();
            let emit = emit.clone();
            let load_tracker = load_tracker.clone();
            self.webview
                .connect_load_changed(move |view, event| match event {
                    webkit6::LoadEvent::Started => {
                        if let Some(event) = load_tracker.borrow().started() {
                            if let Some(content) = content.upgrade() {
                                content.set_visible_child_name("web-content");
                            }
                            status.set_text("Loading upstream Lavish…");
                            emit(event);
                        }
                    }
                    webkit6::LoadEvent::Committed => {
                        if let Some(event) = load_tracker.borrow().committed() {
                            if let Some(content) = content.upgrade() {
                                content.set_visible_child_name("web-content");
                            }
                            status.set_text("Rendering Lavish session…");
                            emit(event);
                        }
                    }
                    webkit6::LoadEvent::Finished => {
                        let title = view.title().map(|value| value.to_string());
                        let response_status = view
                            .main_resource()
                            .and_then(|resource| resource.response())
                            .map(|response| response.status_code());
                        let uri = view
                            .uri()
                            .map(|value| value.to_string())
                            .unwrap_or_default();
                        if let Some(event) =
                            load_tracker
                                .borrow_mut()
                                .finished(response_status, title.clone(), &uri)
                        {
                            match &event {
                                DocumentEvent::Ready(_) => status.set_text(&format!(
                                    "Loaded: {}",
                                    title.as_deref().unwrap_or("untitled Lavish session")
                                )),
                                DocumentEvent::Failed(message) => {
                                    status.set_text(
                                        "Reconnect required: no successful HTTP response",
                                    );
                                    if let Some(failure_message) = failure_message.upgrade() {
                                        failure_message.set_text(message);
                                    }
                                    if let Some(content) = content.upgrade() {
                                        content.set_visible_child_name("reconnect");
                                    }
                                }
                                DocumentEvent::Loading => {}
                            }
                            emit(event);
                        }
                    }
                    _ => {}
                });
        }
        {
            let status = self.status.clone();
            let content = content.downgrade();
            let failure_message = failure_message.downgrade();
            let emit = emit.clone();
            let load_tracker = load_tracker.clone();
            self.webview.connect_load_failed(move |_, _, uri, error| {
                let message = format!("Could not load {uri}: {error}");
                eprintln!("WebKit load failed: {message}");
                status.set_text(&format!("Reconnect required: {error}"));
                if let Some(failure_message) = failure_message.upgrade() {
                    failure_message.set_text(&message);
                }
                if let Some(content) = content.upgrade() {
                    content.set_visible_child_name("reconnect");
                }
                emit(load_tracker.borrow_mut().failed(message));
                true
            });
        }
        {
            let status = self.status.clone();
            let content = content.downgrade();
            let failure_message = failure_message.downgrade();
            let emit = emit.clone();
            let load_tracker = load_tracker.clone();
            self.webview
                .connect_load_failed_with_tls_errors(move |_, uri, _, errors| {
                    let message = format!("TLS failure loading {uri}: {errors:?}");
                    status.set_text("TLS failure. Reconnect required.");
                    if let Some(failure_message) = failure_message.upgrade() {
                        failure_message.set_text(&message);
                    }
                    if let Some(content) = content.upgrade() {
                        content.set_visible_child_name("reconnect");
                    }
                    emit(load_tracker.borrow_mut().failed(message));
                    true
                });
        }
        {
            let status = self.status.clone();
            let content = content.downgrade();
            let failure_message = failure_message.downgrade();
            let emit = emit.clone();
            let load_tracker = load_tracker.clone();
            self.webview
                .connect_web_process_terminated(move |_, reason| {
                    let message = format!("Web process terminated: {reason:?}");
                    status.set_text("Web content stopped. Reload to reconnect.");
                    if let Some(failure_message) = failure_message.upgrade() {
                        failure_message.set_text(&message);
                    }
                    if let Some(content) = content.upgrade() {
                        content.set_visible_child_name("reconnect");
                    }
                    emit(load_tracker.borrow_mut().failed(message));
                });
        }

        let allowed_url = self.loaded_url.clone();
        self.webview
            .connect_decide_policy(move |_, decision, decision_type| {
                if !matches!(
                    decision_type,
                    webkit6::PolicyDecisionType::NavigationAction
                        | webkit6::PolicyDecisionType::NewWindowAction
                ) {
                    return false;
                }
                let Some(navigation) = decision.downcast_ref::<webkit6::NavigationPolicyDecision>()
                else {
                    decision.ignore();
                    return true;
                };
                let Some(action) = navigation.navigation_action() else {
                    decision.ignore();
                    return true;
                };
                let Some(uri) = action.request().and_then(|request| request.uri()) else {
                    decision.ignore();
                    return true;
                };
                let popup = decision_type == webkit6::PolicyDecisionType::NewWindowAction;
                match navigation_outcome(
                    &allowed_url.borrow(),
                    &uri,
                    action.is_user_gesture(),
                    popup,
                ) {
                    NavigationOutcome::Allow => {
                        decision.use_();
                        true
                    }
                    NavigationOutcome::External => {
                        if let Err(error) = gio::AppInfo::launch_default_for_uri(
                            &uri,
                            None::<&gio::AppLaunchContext>,
                        ) {
                            eprintln!("could not open external link {uri}: {error}");
                        }
                        decision.ignore();
                        true
                    }
                    NavigationOutcome::Reject => {
                        eprintln!("blocked top-level navigation to {uri}");
                        decision.ignore();
                        true
                    }
                }
            });

        // A popup is accepted only through decide-policy, where safe user-initiated
        // HTTP(S) destinations are handed to the desktop browser.
        self.webview.connect_create(|_, _| None);
    }
}

pub fn configure_downloads(window: &gtk::ApplicationWindow, status: &gtk::Label) {
    let Some(session) = webkit6::NetworkSession::default() else {
        return;
    };
    let parent = window.clone();
    let status = status.clone();
    session.connect_download_started(move |_, download| {
        show_download_status(&status, DownloadStatus::AwaitingDestination);
        let parent = parent.clone();
        let chooser_status = status.clone();
        download.connect_decide_destination(move |download, suggested| {
            let dialog = gtk::FileDialog::builder()
                .title("Save Lavish download")
                .initial_name(suggested)
                .build();
            let download = download.clone();
            let chooser_status = chooser_status.clone();
            dialog.save(
                Some(&parent),
                None::<&gio::Cancellable>,
                move |result| match result {
                    Ok(file) => download.set_destination(&file.uri()),
                    Err(error) => {
                        download.cancel();
                        if error.matches(gio::IOErrorEnum::Cancelled) {
                            show_download_status(&chooser_status, DownloadStatus::Cancelled);
                        } else {
                            show_download_status(
                                &chooser_status,
                                DownloadStatus::Failed(&error.to_string()),
                            );
                        }
                    }
                },
            );
            true
        });
        let started_status = status.clone();
        download.connect_created_destination(move |_, destination| {
            show_download_status(&started_status, DownloadStatus::Started(destination));
        });
        let progress_status = status.clone();
        download.connect_estimated_progress_notify(move |download| {
            let percent = (download.estimated_progress() * 100.0)
                .round()
                .clamp(0.0, 100.0) as u8;
            show_download_status(&progress_status, DownloadStatus::Progress(percent));
        });
        let failed_status = status.clone();
        download.connect_failed(move |_, error| {
            show_download_status(&failed_status, DownloadStatus::Failed(&error.to_string()));
        });
        let finished_status = status.clone();
        download.connect_finished(move |_| {
            show_download_status(&finished_status, DownloadStatus::Finished);
        });
    });
}

pub fn configure_automation_from_environment() {
    let enabled = std::env::var("LAVISH_BROWSER_AUTOMATION")
        .is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "yes"));
    if enabled && let Some(context) = webkit6::WebContext::default() {
        context.set_automation_allowed(true);
        eprintln!("WebKit automation enabled for this Lavish Browser process");
    }
}

fn navigation_outcome(
    session_url: &str,
    requested_url: &str,
    user_gesture: bool,
    popup: bool,
) -> NavigationOutcome {
    if requested_url == "about:blank" {
        return NavigationOutcome::Allow;
    }
    let Ok(requested) = Url::parse(requested_url) else {
        return NavigationOutcome::Reject;
    };
    if !matches!(requested.scheme(), "http" | "https") {
        return NavigationOutcome::Reject;
    }
    if popup {
        return if user_gesture {
            NavigationOutcome::External
        } else {
            NavigationOutcome::Reject
        };
    }
    let Ok(session) = Url::parse(session_url) else {
        return NavigationOutcome::Reject;
    };
    let same_origin = requested.scheme() == session.scheme()
        && requested.host_str() == session.host_str()
        && requested.port_or_known_default() == session.port_or_known_default();
    if same_origin {
        NavigationOutcome::Allow
    } else if user_gesture {
        NavigationOutcome::External
    } else {
        NavigationOutcome::Reject
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DocumentEvent, DownloadStatus, LoadTracker, NavigationOutcome, RefreshAction,
        RetainedRegistry, download_status_message, navigation_outcome, refresh_action,
    };
    use std::collections::HashSet;

    const SESSION: &str = "http://127.0.0.1:4387/session/abc";

    #[test]
    fn failure_wins_over_finished_until_a_later_explicit_navigation() {
        let mut tracker = LoadTracker::default();
        tracker.begin_navigation();
        assert_eq!(tracker.started(), Some(DocumentEvent::Loading));
        assert_eq!(
            tracker.failed("connection refused".into()),
            DocumentEvent::Failed("connection refused".into())
        );
        assert_eq!(tracker.committed(), None);
        assert_eq!(tracker.finished(Some(200), None, SESSION), None);

        tracker.begin_navigation();
        assert_eq!(tracker.started(), Some(DocumentEvent::Loading));
        assert_eq!(tracker.committed(), Some(DocumentEvent::Loading));
        assert_eq!(
            tracker.finished(Some(200), Some("Recovered · Lavish".into()), SESSION),
            Some(DocumentEvent::Ready(Some("Recovered · Lavish".into())))
        );
    }

    #[test]
    fn process_termination_uses_the_same_sticky_failure_transition() {
        let mut tracker = LoadTracker::default();
        tracker.begin_navigation();
        assert_eq!(tracker.started(), Some(DocumentEvent::Loading));
        assert_eq!(
            tracker.failed("Web process terminated: Crashed".into()),
            DocumentEvent::Failed("Web process terminated: Crashed".into())
        );
        assert_eq!(tracker.committed(), None);
        assert_eq!(
            tracker.finished(Some(200), Some("stale title".into()), SESSION),
            None
        );
    }

    #[test]
    fn finished_without_a_successful_http_response_is_a_sticky_failure() {
        let mut tracker = LoadTracker::default();
        tracker.begin_navigation();
        assert_eq!(
            tracker.finished(None, None, "http://127.0.0.1:9/session/unreachable"),
            Some(DocumentEvent::Failed(
                "No HTTP response loading http://127.0.0.1:9/session/unreachable".into()
            ))
        );
        assert_eq!(tracker.committed(), None);
        assert_eq!(
            tracker.finished(Some(200), Some("error page".into()), SESSION),
            None
        );
    }

    #[test]
    fn refresh_distinguishes_changed_healthy_and_unchanged_failed_urls() {
        assert_eq!(refresh_action(SESSION, SESSION, false), RefreshAction::Keep);
        assert_eq!(
            refresh_action(SESSION, SESSION, true),
            RefreshAction::Reload
        );
        assert_eq!(
            refresh_action(SESSION, "http://127.0.0.1:9000/session/abc", true),
            RefreshAction::Navigate
        );
    }

    #[test]
    fn download_statuses_are_observable_without_choosing_a_desktop_destination() {
        assert_eq!(
            download_status_message(DownloadStatus::AwaitingDestination),
            "Choose a destination for the download"
        );
        assert_eq!(
            download_status_message(DownloadStatus::Started("file:///tmp/report.pdf")),
            "Download started: file:///tmp/report.pdf"
        );
        assert_eq!(
            download_status_message(DownloadStatus::Progress(42)),
            "Download progress: 42%"
        );
        assert_eq!(
            download_status_message(DownloadStatus::Failed("disk full")),
            "Download failed: disk full"
        );
        assert_eq!(
            download_status_message(DownloadStatus::Cancelled),
            "Download cancelled"
        );
        assert_eq!(
            download_status_message(DownloadStatus::Finished),
            "Download finished"
        );
    }

    #[test]
    fn retained_registry_keeps_project_scoped_identity_and_cleans_closed_keys() {
        let first = ("project-a", "/tmp/report.html");
        let second = ("project-b", "/tmp/report.html");
        let mut registry = RetainedRegistry::default();
        registry.insert(first, 11_u64);
        registry.insert(second, 22_u64);
        assert_eq!(registry.get(&first), Some(&11));
        assert_eq!(registry.get(&second), Some(&22));
        assert_eq!(registry.len(), 2);

        assert_eq!(registry.retain_keys(&HashSet::from([second])), 1);
        assert_eq!(registry.get(&first), None);
        assert_eq!(registry.get(&second), Some(&22));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn navigation_policy_preserves_session_and_externalizes_user_links() {
        assert_eq!(
            navigation_outcome(SESSION, "http://127.0.0.1:4387/session/other", false, false),
            NavigationOutcome::Allow
        );
        assert_eq!(
            navigation_outcome(SESSION, "https://example.com/help", true, false),
            NavigationOutcome::External
        );
        assert_eq!(
            navigation_outcome(SESSION, "https://example.com/help", false, false),
            NavigationOutcome::Reject
        );
    }

    #[test]
    fn navigation_policy_rejects_unsafe_schemes_and_non_user_popups() {
        assert_eq!(
            navigation_outcome(SESSION, "about:blank", false, false),
            NavigationOutcome::Allow
        );
        assert_eq!(
            navigation_outcome(SESSION, "javascript:alert(1)", true, false),
            NavigationOutcome::Reject
        );
        assert_eq!(
            navigation_outcome(SESSION, "https://example.com/popup", false, true),
            NavigationOutcome::Reject
        );
        assert_eq!(
            navigation_outcome(SESSION, "https://example.com/popup", true, true),
            NavigationOutcome::External
        );
    }
}
