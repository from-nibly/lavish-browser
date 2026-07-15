#[path = "webview/mod.rs"]
mod webview;

use gtk::glib;
use gtk::prelude::*;
use lavish_browser_control::{SocketServer, default_socket_path};
use lavish_browser_core::persistence::MetadataStore;
use lavish_browser_core::reconcile::{ZellijCommandState, reconcile_from_source};
use lavish_browser_core::{
    BrowserModel, BrowserStateStore, DocumentKey, DocumentLifecycle, ProjectKey,
};
use lavish_browser_protocol::{
    BrowserStateSnapshot, Command, DocumentLifecycleSnapshot, DocumentSnapshot, PROTOCOL_VERSION,
    ProjectSnapshot, RequestEnvelope, ResponseEnvelope, ResponseStatus,
};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const APPLICATION_ID: &str = "works.from-nibly.LavishBrowser";

pub fn run() -> glib::ExitCode {
    webview::configure_automation_from_environment();
    let app = gtk::Application::builder()
        .application_id(APPLICATION_ID)
        .build();
    let controller: Rc<RefCell<Option<Rc<AppController>>>> = Rc::new(RefCell::new(None));

    app.connect_activate({
        let controller = controller.clone();
        move |app| {
            if let Some(controller) = controller.borrow().as_ref() {
                controller
                    .presentation_count
                    .set(controller.presentation_count.get().saturating_add(1));
                controller.window.present();
                return;
            }
            match AppController::new(app) {
                Ok(created) => {
                    created.render();
                    created.window.present();
                    created.presentation_count.set(1);
                    *controller.borrow_mut() = Some(created);
                }
                Err(error) => {
                    eprintln!("failed to start Lavish Browser: {error}");
                    app.quit();
                }
            }
        }
    });

    app.run_with_args::<&str>(&[])
}

struct PendingRequest {
    request: RequestEnvelope,
    reply: mpsc::SyncSender<ResponseEnvelope>,
}

struct AppController {
    window: gtk::ApplicationWindow,
    notebook: gtk::Notebook,
    model: RefCell<BrowserModel>,
    store: MetadataStore,
    rendering: Cell<bool>,
    presentation_count: Cell<u64>,
    document_views: RefCell<webview::RetainedRegistry<DocumentKey, Rc<webview::DocumentView>>>,
}

impl AppController {
    fn new(app: &gtk::Application) -> Result<Rc<Self>, Box<dyn std::error::Error>> {
        let store = MetadataStore::from_xdg_environment()?;
        let mut model = store.load()?;
        let persisted_model = model.clone();
        match reconcile_from_source(&mut model, &ZellijCommandState::system()) {
            Ok(()) if model != persisted_model => store.save(&model)?,
            Ok(()) => {}
            Err(error) => eprintln!("could not reconcile persisted Zellij projects: {error}"),
        }

        let notebook = gtk::Notebook::new();
        notebook.set_hexpand(true);
        notebook.set_vexpand(true);
        notebook.set_scrollable(true);
        notebook.set_show_border(false);

        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("Lavish Browser")
            .default_width(1280)
            .default_height(800)
            .child(&notebook)
            .build();
        window.set_widget_name("lavish-browser-window");
        webview::configure_downloads(&window);

        let controller = Rc::new(Self {
            window,
            notebook,
            model: RefCell::new(model),
            store,
            rendering: Cell::new(false),
            presentation_count: Cell::new(0),
            document_views: RefCell::new(webview::RetainedRegistry::default()),
        });
        controller.connect_notebook_selection();
        controller.start_control_server()?;
        Ok(controller)
    }

    fn connect_notebook_selection(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.notebook.connect_switch_page(move |_, _, index| {
            let Some(controller) = weak.upgrade() else {
                return;
            };
            if controller.rendering.get() {
                return;
            }
            let key = controller
                .model
                .borrow()
                .projects
                .get(index as usize)
                .map(|project| project.key.clone());
            if let Some(key) = key {
                controller.mutate(|model| model.select_project(&key, timestamp()));
            }
        });
    }

    fn start_control_server(self: &Rc<Self>) -> Result<(), Box<dyn std::error::Error>> {
        let (sender, receiver) = mpsc::channel::<PendingRequest>();
        let server = SocketServer::bind(default_socket_path(), move |request| {
            let request_id = request.request_id.clone();
            let (reply_sender, reply_receiver) = mpsc::sync_channel(1);
            if sender
                .send(PendingRequest {
                    request,
                    reply: reply_sender,
                })
                .is_err()
            {
                return response(
                    request_id,
                    ResponseStatus::Error,
                    "application stopped",
                    None,
                );
            }
            reply_receiver
                .recv_timeout(Duration::from_secs(10))
                .unwrap_or_else(|_| {
                    response(
                        request_id,
                        ResponseStatus::Error,
                        "main context did not respond",
                        None,
                    )
                })
        })?;
        std::thread::spawn(move || {
            if let Err(error) = server.run() {
                eprintln!("browser control server stopped: {error}");
            }
        });

        let weak = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(10), move || {
            let Some(controller) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            while let Ok(pending) = receiver.try_recv() {
                let result = controller.handle_command(pending.request);
                let _ = pending.reply.send(result);
            }
            glib::ControlFlow::Continue
        });
        Ok(())
    }

    fn handle_command(self: &Rc<Self>, request: RequestEnvelope) -> ResponseEnvelope {
        let request_id = request.request_id;
        match request.command {
            Command::OpenUrl {
                project,
                source_file,
                url,
            } => {
                let view_actions: Vec<_> = self
                    .model
                    .borrow()
                    .projects
                    .iter()
                    .flat_map(|project| &project.documents)
                    .filter(|document| document.key.canonical_source_file == source_file)
                    .map(|document| {
                        (
                            document.key.clone(),
                            webview::refresh_action(
                                &document.lavish_url,
                                &url,
                                document.lifecycle == DocumentLifecycle::Failed,
                            ),
                        )
                    })
                    .collect();
                let submitted_url = url.clone();
                self.mutate(|model| {
                    model.open_url(project, source_file, url, timestamp());
                    true
                });
                for (key, action) in view_actions {
                    let views = self.document_views.borrow();
                    let Some(view) = views.get(&key) else {
                        continue;
                    };
                    match action {
                        webview::RefreshAction::Keep => {}
                        webview::RefreshAction::Navigate => view.navigate(&submitted_url),
                        webview::RefreshAction::Reload => view.reload(),
                    }
                }
                self.presentation_count
                    .set(self.presentation_count.get().saturating_add(1));
                self.window.present();
                response(request_id, ResponseStatus::Ok, "document opened", None)
            }
            Command::SelectProject {
                session_name,
                stable_tab_id,
            } => {
                let key = ProjectKey::Zellij {
                    session_name,
                    stable_tab_id,
                };
                let selected = self.mutate(|model| model.select_project(&key, timestamp()));
                response(
                    request_id,
                    if selected {
                        ResponseStatus::Ok
                    } else {
                        ResponseStatus::Ignored
                    },
                    if selected {
                        "project selected"
                    } else {
                        "project not found"
                    },
                    None,
                )
            }
            Command::CloseProject {
                session_name,
                stable_tab_id,
            } => {
                let key = ProjectKey::Zellij {
                    session_name,
                    stable_tab_id,
                };
                let closed = self.mutate(|model| model.close_project(&key));
                response(
                    request_id,
                    if closed {
                        ResponseStatus::Ok
                    } else {
                        ResponseStatus::Ignored
                    },
                    if closed {
                        "project closed"
                    } else {
                        "project not found"
                    },
                    None,
                )
            }
            Command::InspectState => response(
                request_id,
                ResponseStatus::Ok,
                "state inspected",
                Some(self.snapshot()),
            ),
            Command::Ping => response(request_id, ResponseStatus::Ok, "ready", None),
        }
    }

    fn mutate(self: &Rc<Self>, action: impl FnOnce(&mut BrowserModel) -> bool) -> bool {
        let changed = {
            let mut model = self.model.borrow_mut();
            action(&mut model)
        };
        if changed {
            if let Err(error) = self.store.save(&self.model.borrow()) {
                eprintln!("could not persist browser metadata: {error}");
            }
            self.render();
        }
        changed
    }

    fn render(self: &Rc<Self>) {
        self.rendering.set(true);
        while self.notebook.n_pages() > 0 {
            self.notebook.remove_page(Some(0));
        }

        let model = self.model.borrow().clone();
        let live_keys: HashSet<_> = model
            .projects
            .iter()
            .flat_map(|project| {
                project
                    .documents
                    .iter()
                    .map(|document| document.key.clone())
            })
            .collect();
        let released_views = self.document_views.borrow_mut().retain_keys(&live_keys);
        if released_views > 0 {
            eprintln!("released {released_views} retained document view(s)");
        }
        for project in &model.projects {
            let page = self.project_page(project);
            let tab = self.project_tab(project);
            self.notebook.append_page(&page, Some(&tab));
        }
        if let Some(selected) = &model.selected_project
            && let Some(index) = model
                .projects
                .iter()
                .position(|project| &project.key == selected)
        {
            self.notebook.set_current_page(Some(index as u32));
        }
        self.rendering.set(false);
    }

    fn project_tab(self: &Rc<Self>, project: &lavish_browser_core::Project) -> gtk::Box {
        let tab = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let label = gtk::Label::new(Some(&project.label));
        label.set_widget_name(&format!("project-tab-{}", safe_name(&project.label)));
        label.set_tooltip_text(project.raw_tab_name.as_deref());
        let close = gtk::Button::from_icon_name("window-close-symbolic");
        close.set_tooltip_text(Some(&format!("Close project {}", project.label)));
        close.update_property(&[gtk::accessible::Property::Label(&format!(
            "Close project {}",
            project.label
        ))]);
        close.add_css_class("flat");
        let key = project.key.clone();
        let weak = Rc::downgrade(self);
        close.connect_clicked(move |_| {
            if let Some(controller) = weak.upgrade() {
                controller.mutate(|model| model.close_project(&key));
            }
        });
        tab.append(&label);
        tab.append(&close);
        tab
    }

    fn project_page(self: &Rc<Self>, project: &lavish_browser_core::Project) -> gtk::Paned {
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Single);
        list.set_widget_name(&format!("documents-{}", safe_name(&project.label)));
        list.set_width_request(280);

        for document in &project.documents {
            let row = document_row(document, self);
            list.append(&row);
        }
        if let Some(selected) = &project.selected_document
            && let Some(index) = project
                .documents
                .iter()
                .position(|document| &document.key.canonical_source_file == selected)
            && let Some(row) = list.row_at_index(index as i32)
        {
            list.select_row(Some(&row));
        }

        let project_key = project.key.clone();
        let weak = Rc::downgrade(self);
        list.connect_row_selected(move |_, row| {
            let Some(row) = row else { return };
            let Some(controller) = weak.upgrade() else {
                return;
            };
            if controller.rendering.get() {
                return;
            }
            let source = controller
                .model
                .borrow()
                .projects
                .iter()
                .find(|project| project.key == project_key)
                .and_then(|project| project.documents.get(row.index() as usize))
                .map(|document| document.key.canonical_source_file.clone());
            if let Some(source) = source {
                let key = DocumentKey {
                    project: project_key.clone(),
                    canonical_source_file: source,
                };
                controller.mutate(|model| model.select_document(&key, timestamp()));
            }
        });

        let sidebar = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&list)
            .build();
        let stack = gtk::Stack::new();
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        if project.documents.is_empty() {
            let empty = gtk::Label::new(Some("No documents in this project"));
            empty.set_widget_name("empty-project-placeholder");
            stack.add_named(&empty, Some("empty"));
        } else {
            for (index, document) in project.documents.iter().enumerate() {
                stack.add_named(
                    &self.document_surface(document, project.selected_document.as_deref()),
                    Some(&format!("document-{index}")),
                );
            }
            if let Some(selected) = &project.selected_document
                && let Some(index) = project
                    .documents
                    .iter()
                    .position(|document| &document.key.canonical_source_file == selected)
            {
                stack.set_visible_child_name(&format!("document-{index}"));
            }
        }
        let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
        paned.set_start_child(Some(&sidebar));
        paned.set_end_child(Some(&stack));
        paned.set_position(280);
        paned.set_resize_start_child(false);
        paned.set_shrink_start_child(false);
        paned
    }

    fn document_surface(
        self: &Rc<Self>,
        document: &lavish_browser_core::Document,
        selected_source: Option<&str>,
    ) -> gtk::Widget {
        let selected = selected_source == Some(&document.key.canonical_source_file);
        if selected || self.document_views.borrow().contains_key(&document.key) {
            return self.document_view(document).widget();
        }
        document_placeholder(document, self)
    }

    fn document_view(
        self: &Rc<Self>,
        document: &lavish_browser_core::Document,
    ) -> Rc<webview::DocumentView> {
        if let Some(view) = self.document_views.borrow().get(&document.key).cloned() {
            if view.loaded_url() != document.lavish_url {
                view.navigate(&document.lavish_url);
            }
            return view;
        }
        let key = document.key.clone();
        let weak = Rc::downgrade(self);
        let event_key = key.clone();
        let emit = Rc::new(move |event| {
            if let Some(controller) = weak.upgrade() {
                controller.apply_document_event(&event_key, event);
            }
        });
        let accessible_name = format!(
            "lavish-view-{}",
            safe_name(&document.key.canonical_source_file)
        );
        let view =
            webview::DocumentView::new(&document.lavish_url, &accessible_name, &self.window, emit);
        self.document_views.borrow_mut().insert(key, view.clone());
        view
    }

    fn apply_document_event(self: &Rc<Self>, key: &DocumentKey, event: webview::DocumentEvent) {
        let changed = {
            let mut model = self.model.borrow_mut();
            let Some(document) = model
                .projects
                .iter_mut()
                .flat_map(|project| &mut project.documents)
                .find(|document| document.key == *key)
            else {
                return;
            };
            match event {
                webview::DocumentEvent::Loading => {
                    document.lifecycle = DocumentLifecycle::Loading;
                    document.load_error = None;
                }
                webview::DocumentEvent::Ready(title) => {
                    document.lifecycle = DocumentLifecycle::Ready;
                    document.load_error = None;
                    document.title = title;
                }
                webview::DocumentEvent::Failed(error) => {
                    document.lifecycle = DocumentLifecycle::Failed;
                    document.load_error = Some(error);
                }
            }
            true
        };
        if changed {
            if let Err(error) = self.store.save(&self.model.borrow()) {
                eprintln!("could not persist WebView state: {error}");
            }
            self.render();
        }
    }

    fn snapshot(&self) -> BrowserStateSnapshot {
        snapshot(&self.model.borrow(), self.presentation_count.get())
    }
}

fn document_row(
    document: &lavish_browser_core::Document,
    controller: &Rc<AppController>,
) -> gtk::ListBoxRow {
    let source = Path::new(&document.key.canonical_source_file);
    let basename = source
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&document.key.canonical_source_file);
    let parent = source
        .parent()
        .and_then(Path::to_str)
        .filter(|parent| !parent.is_empty())
        .unwrap_or(" ");
    let labels = gtk::Box::new(gtk::Orientation::Vertical, 1);
    let name = gtk::Label::new(Some(basename));
    name.set_xalign(0.0);
    let detail = gtk::Label::new(Some(&format!(
        "{parent} · {}",
        lifecycle_text(&document.lifecycle)
    )));
    detail.set_xalign(0.0);
    detail.add_css_class("dim-label");
    detail.add_css_class("caption");
    labels.append(&name);
    labels.append(&detail);

    let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    labels.set_hexpand(true);
    content.append(&labels);
    let close = gtk::Button::from_icon_name("window-close-symbolic");
    close.set_tooltip_text(Some(&format!("Close document {basename}")));
    close.update_property(&[gtk::accessible::Property::Label(&format!(
        "Close document {basename}"
    ))]);
    close.add_css_class("flat");
    let key = document.key.clone();
    let weak = Rc::downgrade(controller);
    close.connect_clicked(move |_| {
        if let Some(controller) = weak.upgrade() {
            controller.mutate(|model| model.close_document(&key));
        }
    });
    content.append(&close);

    let row = gtk::ListBoxRow::new();
    row.set_widget_name(&format!("document-row-{}", safe_name(basename)));
    row.update_property(&[gtk::accessible::Property::Label(&format!(
        "Document {basename}, {}",
        lifecycle_text(&document.lifecycle)
    ))]);
    row.set_child(Some(&content));
    row
}

fn document_placeholder(
    document: &lavish_browser_core::Document,
    controller: &Rc<AppController>,
) -> gtk::Widget {
    let container = gtk::Box::new(gtk::Orientation::Vertical, 12);
    container.set_halign(gtk::Align::Center);
    container.set_valign(gtk::Align::Center);
    let source = &document.key.canonical_source_file;
    let basename = Path::new(source)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(source);
    let title = gtk::Label::new(Some(basename));
    title.add_css_class("title-2");
    let status = gtk::Label::new(Some(&format!(
        "{} — Web content will be created when selected",
        lifecycle_text(&document.lifecycle)
    )));
    status.set_widget_name("document-placeholder");
    status.update_property(&[gtk::accessible::Property::Label(&format!(
        "Document placeholder for {basename}, {}",
        lifecycle_text(&document.lifecycle)
    ))]);
    container.append(&title);
    container.append(&status);
    if matches!(document.lifecycle, DocumentLifecycle::Failed) {
        let reload = gtk::Button::with_label("Retry loading");
        let key = document.key.clone();
        let weak = Rc::downgrade(controller);
        reload.connect_clicked(move |_| {
            if let Some(controller) = weak.upgrade() {
                controller.mutate(|model| {
                    model.set_document_lifecycle(&key, DocumentLifecycle::Dormant, None)
                });
            }
        });
        container.append(&reload);
    }
    container.upcast()
}

fn response(
    request_id: String,
    status: ResponseStatus,
    message: &str,
    state: Option<BrowserStateSnapshot>,
) -> ResponseEnvelope {
    ResponseEnvelope {
        protocol_version: PROTOCOL_VERSION,
        request_id,
        status,
        error_code: None,
        message: Some(message.to_owned()),
        state,
    }
}

fn snapshot(model: &BrowserModel, presentation_count: u64) -> BrowserStateSnapshot {
    BrowserStateSnapshot {
        projects: model
            .projects
            .iter()
            .map(|project| ProjectSnapshot {
                key: project.key.clone(),
                label: project.label.clone(),
                selected_source_file: project.selected_document.clone(),
                documents: project
                    .documents
                    .iter()
                    .map(|document| DocumentSnapshot {
                        source_file: document.key.canonical_source_file.clone(),
                        url: document.lavish_url.clone(),
                        title: document.title.clone(),
                        lifecycle: match document.lifecycle {
                            DocumentLifecycle::Dormant => DocumentLifecycleSnapshot::Dormant,
                            DocumentLifecycle::Loading => DocumentLifecycleSnapshot::Loading,
                            DocumentLifecycle::Ready => DocumentLifecycleSnapshot::Ready,
                            DocumentLifecycle::Failed => DocumentLifecycleSnapshot::Failed,
                            DocumentLifecycle::Suspended => DocumentLifecycleSnapshot::Suspended,
                        },
                    })
                    .collect(),
            })
            .collect(),
        selected_project: model.selected_project.clone(),
        presentation_count,
        process_id: std::process::id(),
        runtime_id: Some(format!("{APPLICATION_ID}:{}", std::process::id())),
    }
}

fn lifecycle_text(lifecycle: &DocumentLifecycle) -> &'static str {
    match lifecycle {
        DocumentLifecycle::Dormant => "Dormant",
        DocumentLifecycle::Loading => "Loading",
        DocumentLifecycle::Ready => "Ready",
        DocumentLifecycle::Failed => "Failed",
        DocumentLifecycle::Suspended => "Suspended",
    }
}

fn safe_name(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::snapshot;
    use lavish_browser_core::BrowserModel;
    use lavish_browser_protocol::{ProjectKey, ProjectMetadata};

    #[test]
    fn inspect_snapshot_correlates_selection_and_presentations() {
        let mut model = BrowserModel::default();
        let key = ProjectKey::Standalone {
            label: "Standalone".into(),
        };
        model.open_url(
            ProjectMetadata {
                key: key.clone(),
                label: "Standalone".into(),
                raw_tab_name: None,
            },
            "/tmp/report.html",
            "http://127.0.0.1:4387/session/report",
            1,
        );
        let state = snapshot(&model, 3);
        assert_eq!(state.selected_project, Some(key));
        assert_eq!(
            state.projects[0].selected_source_file.as_deref(),
            Some("/tmp/report.html")
        );
        assert_eq!(state.projects[0].documents.len(), 1);
        assert_eq!(state.presentation_count, 3);
    }
}
