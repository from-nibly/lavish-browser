use gtk::gio;
use gtk::prelude::*;
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
enum NavigationOutcome {
    Allow,
    External,
    Reject,
}

pub struct DocumentView {
    root: gtk::Box,
    webview: webkit6::WebView,
    status: gtk::Label,
    loaded_url: Rc<std::cell::RefCell<String>>,
}

impl DocumentView {
    pub fn new(
        url: &str,
        accessible_name: &str,
        _window: &gtk::ApplicationWindow,
        emit: Rc<dyn Fn(DocumentEvent)>,
    ) -> Rc<Self> {
        let webview = webkit6::WebView::new();
        webview.set_hexpand(true);
        webview.set_vexpand(true);
        webview.set_widget_name(accessible_name);
        webview.update_property(&[gtk::accessible::Property::Label(&format!(
            "Lavish document {accessible_name}"
        ))]);

        let status = gtk::Label::new(Some("Connecting to upstream Lavish…"));
        status.set_xalign(0.0);
        status.add_css_class("dim-label");
        status.set_widget_name(&format!("{accessible_name}-status"));
        status.update_property(&[gtk::accessible::Property::Label(&format!(
            "Load status for {accessible_name}"
        ))]);

        let content = gtk::Stack::new();
        content.set_hexpand(true);
        content.set_vexpand(true);
        content.add_named(&webview, Some("web-content"));

        let failure = gtk::Box::new(gtk::Orientation::Vertical, 12);
        failure.set_halign(gtk::Align::Center);
        failure.set_valign(gtk::Align::Center);
        failure.set_widget_name(&format!("{accessible_name}-reconnect"));
        failure.update_property(&[gtk::accessible::Property::Label(&format!(
            "Reconnect {accessible_name}"
        ))]);
        let failure_message = gtk::Label::new(Some("The Lavish session is unavailable."));
        let retry = gtk::Button::with_label("Reconnect");
        {
            let webview = webview.downgrade();
            let content = content.downgrade();
            retry.connect_clicked(move |_| {
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
        reload.set_widget_name(&format!("{accessible_name}-reload"));
        reload.update_property(&[gtk::accessible::Property::Label(&format!(
            "Reload {accessible_name}"
        ))]);
        {
            let webview = webview.downgrade();
            let content = content.downgrade();
            reload.connect_clicked(move |_| {
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
            let inspect_view = webview.clone();
            inspector.connect_clicked(move |_| {
                if let Some(inspector) = inspect_view.inspector() {
                    inspector.show();
                }
            });
            toolbar.prepend(&inspector);
        }

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&toolbar);
        root.append(&content);

        let controller = Rc::new(Self {
            root,
            webview,
            status,
            loaded_url: Rc::new(std::cell::RefCell::new(url.to_owned())),
        });
        controller.connect_signals(&content, &failure_message, emit);
        controller.webview.load_uri(url);
        controller
    }

    pub fn widget(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    pub fn navigate(&self, url: &str) {
        if self.loaded_url.borrow().as_str() != url {
            self.loaded_url.replace(url.to_owned());
            self.status
                .set_text("Connecting to refreshed Lavish session…");
            self.webview.load_uri(url);
        }
    }

    pub fn reload(&self) {
        self.status.set_text("Reconnecting to upstream Lavish…");
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
        {
            let status = self.status.clone();
            let content = content.downgrade();
            let emit = emit.clone();
            self.webview
                .connect_load_changed(move |view, event| match event {
                    webkit6::LoadEvent::Started => {
                        if let Some(content) = content.upgrade() {
                            content.set_visible_child_name("web-content");
                        }
                        status.set_text("Loading upstream Lavish…");
                        emit(DocumentEvent::Loading);
                    }
                    webkit6::LoadEvent::Committed => status.set_text("Rendering Lavish session…"),
                    webkit6::LoadEvent::Finished => {
                        let title = view.title().map(|value| value.to_string());
                        status.set_text(&format!(
                            "Loaded: {}",
                            title.as_deref().unwrap_or("untitled Lavish session")
                        ));
                        emit(DocumentEvent::Ready(title));
                    }
                    _ => {}
                });
        }
        {
            let status = self.status.clone();
            let content = content.downgrade();
            let failure_message = failure_message.downgrade();
            let emit = emit.clone();
            self.webview.connect_load_failed(move |_, _, uri, error| {
                let message = format!("Could not load {uri}: {error}");
                status.set_text(&format!("Reconnect required: {error}"));
                if let Some(failure_message) = failure_message.upgrade() {
                    failure_message.set_text(&message);
                }
                if let Some(content) = content.upgrade() {
                    content.set_visible_child_name("reconnect");
                }
                emit(DocumentEvent::Failed(message));
                true
            });
        }
        {
            let status = self.status.clone();
            let content = content.downgrade();
            let failure_message = failure_message.downgrade();
            let emit = emit.clone();
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
                    emit(DocumentEvent::Failed(message));
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

pub fn configure_downloads(window: &gtk::ApplicationWindow) {
    let Some(session) = webkit6::NetworkSession::default() else {
        return;
    };
    let parent = window.clone();
    session.connect_download_started(move |_, download| {
        let parent = parent.clone();
        download.connect_decide_destination(move |download, suggested| {
            let dialog = gtk::FileDialog::builder()
                .title("Save Lavish download")
                .initial_name(suggested)
                .build();
            let download = download.clone();
            dialog.save(
                Some(&parent),
                None::<&gio::Cancellable>,
                move |result| match result {
                    Ok(file) => download.set_destination(&file.uri()),
                    Err(error) => {
                        download.cancel();
                        if !error.matches(gio::IOErrorEnum::Cancelled) {
                            eprintln!("download destination failed: {error}");
                        }
                    }
                },
            );
            true
        });
        download.connect_created_destination(|_, destination| {
            eprintln!("download started: {destination}");
        });
        download.connect_failed(|_, error| eprintln!("download failed: {error}"));
        download.connect_finished(|_| eprintln!("download finished"));
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
    use super::{NavigationOutcome, navigation_outcome};

    const SESSION: &str = "http://127.0.0.1:4387/session/abc";

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
