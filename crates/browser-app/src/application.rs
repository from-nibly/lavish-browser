#[path = "memory.rs"]
mod memory;
#[path = "webview/mod.rs"]
mod webview;

use gtk::gio;
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
    let controller: Rc<RefCell<Option<Rc<AppController>>>> = Rc::new(RefCell::new(None));
    webview::configure_automation_session({
        let controller = controller.clone();
        move || {
            controller
                .borrow()
                .as_ref()
                .and_then(|controller| controller.automation_webview())
        }
    });
    let app = gtk::Application::builder()
        .application_id(APPLICATION_ID)
        .build();

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
    memory_monitor: gio::MemoryMonitor,
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
        notebook.set_widget_name("project-notebook");
        notebook.update_property(&[gtk::accessible::Property::Label("Lavish projects")]);
        notebook.set_hexpand(true);
        notebook.set_vexpand(true);
        notebook.set_scrollable(true);
        notebook.set_show_border(false);

        let download_status = gtk::Label::new(Some("No active downloads"));
        download_status.set_xalign(0.0);
        download_status.set_margin_top(6);
        download_status.set_margin_bottom(6);
        download_status.set_margin_start(8);
        download_status.set_margin_end(8);
        download_status.set_widget_name("download-status");
        download_status.update_property(&[gtk::accessible::Property::Label("Download status")]);
        download_status.set_visible(false);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.set_widget_name("lavish-browser-content");
        root.append(&notebook);
        root.append(&download_status);

        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("Lavish Browser")
            .default_width(1280)
            .default_height(800)
            .child(&root)
            .build();
        window.set_widget_name("lavish-browser-window");
        webview::configure_downloads(&window, &download_status);

        let controller = Rc::new(Self {
            window,
            notebook,
            model: RefCell::new(model),
            store,
            rendering: Cell::new(false),
            presentation_count: Cell::new(0),
            document_views: RefCell::new(webview::RetainedRegistry::default()),
            memory_monitor: gio::MemoryMonitor::dup_default(),
        });
        controller.connect_notebook_selection();
        controller.connect_memory_monitor();
        controller.start_control_server()?;
        Ok(controller)
    }

    fn connect_memory_monitor(self: &Rc<Self>) {
        use gio::prelude::MemoryMonitorExt;

        let weak = Rc::downgrade(self);
        self.memory_monitor
            .connect_low_memory_warning(move |_, level| {
                let Some(controller) = weak.upgrade() else {
                    return;
                };
                let level = match level {
                    gio::MemoryMonitorWarningLevel::Low => memory::WarningLevel::Low,
                    gio::MemoryMonitorWarningLevel::Medium => memory::WarningLevel::Medium,
                    gio::MemoryMonitorWarningLevel::Critical => memory::WarningLevel::Critical,
                    _ => memory::WarningLevel::Critical,
                };
                controller.handle_memory_warning(level, "GIO MemoryMonitor");
            });

        let Some(path) = std::env::var_os("LAVISH_BROWSER_MEMORY_WARNING_FILE") else {
            return;
        };
        let mut previous = String::new();
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(100), move || {
            let Some(controller) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let Ok(contents) = std::fs::read_to_string(&path) else {
                return glib::ControlFlow::Continue;
            };
            if contents == previous {
                return glib::ControlFlow::Continue;
            }
            previous.clone_from(&contents);
            let warning = contents
                .split_whitespace()
                .next_back()
                .and_then(memory::WarningLevel::parse);
            if let Some(warning) = warning {
                controller.handle_memory_warning(warning, "deterministic test hook");
            } else if !contents.trim().is_empty() {
                eprintln!(
                    "ignored invalid memory warning hook value; expected low, medium, or critical"
                );
            }
            glib::ControlFlow::Continue
        });
    }

    fn handle_memory_warning(self: &Rc<Self>, level: memory::WarningLevel, source: &str) {
        let (candidates, materialized_lru) = {
            let model = self.model.borrow();
            let views = self.document_views.borrow();
            let materialized_lru = model
                .inactive_documents_lru()
                .into_iter()
                .filter(|document| views.contains_key(&document.key))
                .map(|document| {
                    format!(
                        "{}@{}",
                        document.key.canonical_source_file, document.last_activated_at
                    )
                })
                .collect::<Vec<_>>();
            let candidates =
                memory::suspension_candidates(&model, level, |key| views.contains_key(key));
            (candidates, materialized_lru)
        };
        eprintln!(
            "{source} warning {level:?}: materialized inactive LRU [{}]",
            materialized_lru.join(", ")
        );
        if candidates.is_empty() {
            eprintln!("{source} warning {level:?}: no inactive materialized document to suspend");
            return;
        }
        let candidate_set: HashSet<_> = candidates.iter().cloned().collect();
        eprintln!(
            "{source} warning {level:?}: suspending {} inactive document(s)",
            candidates.len()
        );
        self.mutate(|model| {
            let mut changed = false;
            for document in model
                .projects
                .iter_mut()
                .flat_map(|project| &mut project.documents)
            {
                if candidate_set.contains(&document.key) {
                    document.lifecycle = DocumentLifecycle::Suspended;
                    document.load_error = None;
                    changed = true;
                }
            }
            changed
        });
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
        if self.rendering.replace(true) {
            return;
        }
        let retained_widgets: Vec<_> = self
            .document_views
            .borrow()
            .values()
            .map(|view| view.widget())
            .collect();
        for widget in retained_widgets {
            if let Some(parent) = widget.parent()
                && let Ok(stack) = parent.downcast::<gtk::Stack>()
            {
                stack.remove(&widget);
            }
        }
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
                    .filter(|document| document.lifecycle != DocumentLifecycle::Suspended)
                    .map(|document| document.key.clone())
            })
            .collect();
        let released_views = self.document_views.borrow_mut().retain_keys(&live_keys);
        if released_views > 0 {
            eprintln!("released {released_views} retained document view(s)");
        }
        for project in &model.projects {
            let page = self.project_page(
                project,
                model.selected_project.as_ref() == Some(&project.key),
            );
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
        let identity = project_identity(&project.key);
        let tab = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        tab.set_widget_name(&format!("project-tab-{identity}"));
        tab.update_property(&[gtk::accessible::Property::Label(&format!(
            "Project {}, {}",
            project.label,
            project_accessible_identity(&project.key)
        ))]);
        let label = gtk::Label::new(Some(&project.label));
        label.set_widget_name(&format!("project-tab-label-{identity}"));
        label.update_property(&[gtk::accessible::Property::Label(&format!(
            "Project {}, {}",
            project.label,
            project_accessible_identity(&project.key)
        ))]);
        label.set_tooltip_text(project.raw_tab_name.as_deref());
        let close = gtk::Button::from_icon_name("window-close-symbolic");
        close.set_widget_name(&format!("project-close-{identity}"));
        close.set_tooltip_text(Some(&format!("Close project {}", project.label)));
        close.update_property(&[gtk::accessible::Property::Label(&format!(
            "Close project {}, {}",
            project.label,
            project_accessible_identity(&project.key)
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

    fn project_page(
        self: &Rc<Self>,
        project: &lavish_browser_core::Project,
        globally_selected: bool,
    ) -> gtk::Paned {
        let identity = project_identity(&project.key);
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Single);
        list.set_widget_name(&format!("project-documents-{identity}"));
        list.update_property(&[gtk::accessible::Property::Label(&format!(
            "Documents for project {}, {}",
            project.label,
            project_accessible_identity(&project.key)
        ))]);
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
        stack.set_widget_name(&format!("project-document-stack-{identity}"));
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        if project.documents.is_empty() {
            let empty = gtk::Label::new(Some("No documents in this project"));
            empty.set_widget_name(&format!("empty-project-placeholder-{identity}"));
            empty.update_property(&[gtk::accessible::Property::Label(&format!(
                "No documents in project {}, {}",
                project.label,
                project_accessible_identity(&project.key)
            ))]);
            stack.add_named(&empty, Some("empty"));
        } else {
            for (index, document) in project.documents.iter().enumerate() {
                stack.add_named(
                    &self.document_surface(
                        document,
                        globally_selected,
                        project.selected_document.as_deref(),
                    ),
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
        paned.set_widget_name(&format!("project-page-{identity}"));
        paned.update_property(&[gtk::accessible::Property::Label(&format!(
            "Project page {}, {}",
            project.label,
            project_accessible_identity(&project.key)
        ))]);
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
        project_is_globally_selected: bool,
        selected_source: Option<&str>,
    ) -> gtk::Widget {
        let document_is_locally_selected =
            selected_source == Some(&document.key.canonical_source_file);
        let already_materialized = self.document_views.borrow().contains_key(&document.key);
        match memory::materialization_plan(
            project_is_globally_selected,
            document_is_locally_selected,
            already_materialized,
        ) {
            memory::MaterializationPlan::Create | memory::MaterializationPlan::Reuse => {
                self.document_view(document).widget()
            }
            memory::MaterializationPlan::Placeholder => document_placeholder(document, self),
        }
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
        let view = self.create_document_view(document, false);
        self.document_views
            .borrow_mut()
            .insert(document.key.clone(), view.clone());
        view
    }

    fn create_document_view(
        self: &Rc<Self>,
        document: &lavish_browser_core::Document,
        automation: bool,
    ) -> Rc<webview::DocumentView> {
        let key = document.key.clone();
        let weak = Rc::downgrade(self);
        let event_key = key.clone();
        let emit = Rc::new(move |event| {
            if let Some(controller) = weak.upgrade() {
                controller.apply_document_event(&event_key, event);
            }
        });
        let identity = document_identity(&document.key);
        let basename = Path::new(&document.key.canonical_source_file)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&document.key.canonical_source_file);
        let accessible_description = format!(
            "{basename}, source {}, {}",
            document.key.canonical_source_file,
            project_accessible_identity(&document.key.project)
        );
        if automation {
            webview::DocumentView::new_for_automation(
                &document.lavish_url,
                &identity,
                &accessible_description,
                &self.window,
                emit,
            )
        } else {
            webview::DocumentView::new(
                &document.lavish_url,
                &identity,
                &accessible_description,
                &self.window,
                emit,
            )
        }
    }

    fn automation_webview(self: &Rc<Self>) -> Option<webkit6::WebView> {
        let document = {
            let model = self.model.borrow();
            let project_key = model.selected_project.as_ref()?;
            let project = model
                .projects
                .iter()
                .find(|project| &project.key == project_key)?;
            let source = project.selected_document.as_ref()?;
            project
                .documents
                .iter()
                .find(|document| &document.key.canonical_source_file == source)?
                .clone()
        };
        let view = self.create_document_view(&document, true);
        let webview = view.automation_webview();
        self.document_views
            .borrow_mut()
            .insert(document.key.clone(), view);
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(controller) = weak.upgrade() {
                controller.render();
            }
        });
        Some(webview)
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
    let (basename, detail_text) =
        document_row_labels(&document.key.canonical_source_file, &document.lifecycle);
    let labels = gtk::Box::new(gtk::Orientation::Vertical, 1);
    labels.set_hexpand(true);
    let name = gtk::Label::new(Some(&basename));
    name.set_xalign(0.0);
    name.set_hexpand(true);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.set_max_width_chars(28);
    let detail = gtk::Label::new(Some(detail_text));
    detail.set_xalign(0.0);
    detail.add_css_class("dim-label");
    detail.add_css_class("caption");
    labels.append(&name);
    labels.append(&detail);

    let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    labels.set_hexpand(true);
    content.append(&labels);
    let identity = document_identity(&document.key);
    let close = gtk::Button::from_icon_name("window-close-symbolic");
    close.set_widget_name(&format!("document-close-{identity}"));
    close.set_tooltip_text(Some(&format!("Close document {basename}")));
    close.update_property(&[gtk::accessible::Property::Label(&format!(
        "Close document {basename}, {}, document identity {identity}",
        project_accessible_identity(&document.key.project)
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
    row.set_widget_name(&format!("document-row-{identity}"));
    row.update_property(&[gtk::accessible::Property::Label(&format!(
        "Document {basename}, {}, {}, document identity {identity}",
        project_accessible_identity(&document.key.project),
        lifecycle_text(&document.lifecycle)
    ))]);
    row.set_tooltip_text(Some(&document.key.canonical_source_file));
    row.set_child(Some(&content));
    row
}

fn document_row_labels(source: &str, lifecycle: &DocumentLifecycle) -> (String, &'static str) {
    let basename = Path::new(source)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(source)
        .to_owned();
    (basename, lifecycle_text(lifecycle))
}

fn document_placeholder(
    document: &lavish_browser_core::Document,
    controller: &Rc<AppController>,
) -> gtk::Widget {
    let identity = document_identity(&document.key);
    let container = gtk::Box::new(gtk::Orientation::Vertical, 12);
    container.set_widget_name(&format!("document-placeholder-{identity}"));
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
    status.set_widget_name(&format!("document-placeholder-status-{identity}"));
    status.update_property(&[gtk::accessible::Property::Label(&format!(
        "Document placeholder for {basename}, source {}, {}, {}",
        document.key.canonical_source_file,
        project_accessible_identity(&document.key.project),
        lifecycle_text(&document.lifecycle)
    ))]);
    container.append(&title);
    container.append(&status);
    if matches!(document.lifecycle, DocumentLifecycle::Failed) {
        let reload = gtk::Button::from_icon_name("view-refresh-symbolic");
        reload.set_tooltip_text(Some("Retry loading"));
        reload.set_widget_name(&format!("document-placeholder-retry-{identity}"));
        reload.update_property(&[gtk::accessible::Property::Label(&format!(
            "Retry loading document {basename}, source {}, {}",
            document.key.canonical_source_file,
            project_accessible_identity(&document.key.project)
        ))]);
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

fn project_accessible_identity(key: &ProjectKey) -> String {
    match key {
        ProjectKey::Zellij {
            session_name,
            stable_tab_id,
        } => format!("Zellij session {session_name}, tab {stable_tab_id}"),
        ProjectKey::Standalone { label } => format!("standalone project {label}"),
    }
}

fn project_identity(key: &ProjectKey) -> String {
    match key {
        ProjectKey::Zellij {
            session_name,
            stable_tab_id,
        } => format!(
            "zellij-{}-tab-{stable_tab_id}",
            encode_identifier(session_name)
        ),
        ProjectKey::Standalone { label } => {
            format!("standalone-{}", encode_identifier(label))
        }
    }
}

fn document_identity(key: &DocumentKey) -> String {
    format!(
        "{}-source-{}",
        project_identity(&key.project),
        encode_identifier(&key.canonical_source_file)
    )
}

fn encode_identifier(value: &str) -> String {
    use std::fmt::Write;

    value.as_bytes().iter().fold(
        String::with_capacity(value.len().saturating_mul(2)),
        |mut encoded, byte| {
            write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
            encoded
        },
    )
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
    use super::{document_identity, document_row_labels, project_identity, snapshot};
    use lavish_browser_core::{BrowserModel, DocumentKey, DocumentLifecycle};
    use lavish_browser_protocol::{ProjectKey, ProjectMetadata};

    #[test]
    fn stable_identifiers_ignore_mutable_project_and_document_state() {
        let key = ProjectKey::Zellij {
            session_name: "main/session".into(),
            stable_tab_id: 17,
        };
        let project_id = project_identity(&key);
        let mut model = BrowserModel::default();
        model.open_url(
            ProjectMetadata {
                key: key.clone(),
                label: "Initial label".into(),
                raw_tab_name: Some("initial raw title".into()),
            },
            "/tmp/report.html",
            "http://127.0.0.1:4387/session/report",
            1,
        );
        let document_key = model.projects[0].documents[0].key.clone();
        let document_id = document_identity(&document_key);
        model.projects[0].label = "Renamed project".into();
        model.projects[0].raw_tab_name = Some("renamed raw title".into());
        model.projects[0].documents[0].title = Some("Changed Lavish title".into());
        model.projects[0].documents[0].lifecycle = DocumentLifecycle::Failed;

        assert_eq!(project_identity(&model.projects[0].key), project_id);
        assert_eq!(
            document_identity(&model.projects[0].documents[0].key),
            document_id
        );
        assert_eq!(project_id, "zellij-6d61696e2f73657373696f6e-tab-17");
        assert_eq!(
            document_id,
            "zellij-6d61696e2f73657373696f6e-tab-17-source-2f746d702f7265706f72742e68746d6c"
        );
    }

    #[test]
    fn document_identifiers_disambiguate_paths_and_projects_without_lossy_escaping() {
        let first_project = ProjectKey::Standalone {
            label: "First".into(),
        };
        let second_project = ProjectKey::Standalone {
            label: "Second".into(),
        };
        let first = DocumentKey {
            project: first_project.clone(),
            canonical_source_file: "/work/a/report.html".into(),
        };
        let same_basename = DocumentKey {
            project: first_project,
            canonical_source_file: "/work/b/report.html".into(),
        };
        let same_source_other_project = DocumentKey {
            project: second_project,
            canonical_source_file: first.canonical_source_file.clone(),
        };
        assert_ne!(document_identity(&first), document_identity(&same_basename));
        assert_ne!(
            document_identity(&first),
            document_identity(&same_source_other_project)
        );
        assert_ne!(
            document_identity(&DocumentKey {
                project: first.project.clone(),
                canonical_source_file: "/work/a-b/report.html".into(),
            }),
            document_identity(&DocumentKey {
                project: first.project.clone(),
                canonical_source_file: "/work/a/b-report.html".into(),
            })
        );
    }

    #[test]
    fn document_row_visible_labels_do_not_include_the_canonical_parent_path() {
        let source = "/very/long/worktree/path/that/must/not/size/the/sidebar/report.html";
        let (name, detail) = document_row_labels(source, &DocumentLifecycle::Ready);

        assert_eq!(name, "report.html");
        assert_eq!(detail, "Ready");
        assert!(!name.contains("/very/long"));
        assert!(!detail.contains("/very/long"));
    }

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
