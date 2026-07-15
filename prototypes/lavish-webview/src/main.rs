use gtk::prelude::*;
use webkit6::prelude::*;

fn main() -> gtk::glib::ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(url) = args.next() else {
        eprintln!("usage: lavish-webview-prototype <lavish-session-url>");
        return gtk::glib::ExitCode::FAILURE;
    };

    if !is_loopback_lavish_url(&url) {
        eprintln!("warning: expected a loopback HTTP(S) Lavish session URL, got {url}");
    }

    let app = gtk::Application::builder()
        .application_id("works.from-nibly.LavishWebviewPrototype")
        .build();

    app.connect_activate(move |app| build_window(app, &url));
    app.run_with_args::<&str>(&[])
}

fn build_window(app: &gtk::Application, url: &str) {
    let webview = webkit6::WebView::new();
    if let Some(settings) = webkit6::prelude::WebViewExt::settings(&webview) {
        settings.set_enable_developer_extras(true);
    }

    let status = gtk::Label::new(Some("Loading upstream Lavish session…"));
    status.set_xalign(0.0);
    status.add_css_class("dim-label");

    let reload = gtk::Button::with_label("Reload");
    {
        let webview = webview.clone();
        reload.connect_clicked(move |_| webview.reload());
    }

    let inspector = gtk::Button::with_label("Inspector");
    {
        let webview = webview.clone();
        inspector.connect_clicked(move |_| {
            if let Some(inspector) = webview.inspector() {
                inspector.show();
            }
        });
    }

    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    toolbar.set_margin_top(8);
    toolbar.set_margin_bottom(8);
    toolbar.set_margin_start(8);
    toolbar.set_margin_end(8);
    toolbar.append(&reload);
    toolbar.append(&inspector);
    toolbar.append(&status);

    {
        let status = status.clone();
        webview.connect_load_changed(move |view, event| {
            let message = match event {
                webkit6::LoadEvent::Started => "Loading…".to_owned(),
                webkit6::LoadEvent::Redirected => "Redirected…".to_owned(),
                webkit6::LoadEvent::Committed => "Rendering…".to_owned(),
                webkit6::LoadEvent::Finished => format!(
                    "Loaded: {}",
                    view.title().as_deref().unwrap_or("untitled Lavish session")
                ),
                _ => "Loading…".to_owned(),
            };
            status.set_text(&message);
            if event == webkit6::LoadEvent::Finished {
                println!("{message}");
            }
        });
    }

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&toolbar);
    content.append(&webview);

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Lavish WebKitGTK compatibility prototype")
        .default_width(1440)
        .default_height(960)
        .child(&content)
        .build();

    webview.set_vexpand(true);
    webview.set_hexpand(true);
    webview.load_uri(url);
    window.present();
}

fn is_loopback_lavish_url(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    (value.starts_with("http://127.0.0.1:")
        || value.starts_with("http://localhost:")
        || value.starts_with("http://[::1]:")
        || value.starts_with("https://127.0.0.1:")
        || value.starts_with("https://localhost:")
        || value.starts_with("https://[::1]:"))
        && value.contains("/session/")
}

#[cfg(test)]
mod tests {
    use super::is_loopback_lavish_url;

    #[test]
    fn accepts_loopback_session_urls() {
        assert!(is_loopback_lavish_url(
            "http://127.0.0.1:4387/session/0123456789abcdef"
        ));
        assert!(is_loopback_lavish_url(
            "http://localhost:9000/session/key?noGate=1"
        ));
    }

    #[test]
    fn rejects_remote_and_non_session_urls() {
        assert!(!is_loopback_lavish_url(
            "https://example.com/session/0123456789abcdef"
        ));
        assert!(!is_loopback_lavish_url("http://127.0.0.1:4387/health"));
    }
}
