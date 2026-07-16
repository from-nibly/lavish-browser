use std::fs;
use std::path::PathBuf;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/lavish-compat")
}

#[test]
fn representative_fixture_stays_secret_free_and_covers_browser_boundaries() {
    let root = fixture();
    let html = fs::read_to_string(root.join("artifact.html")).unwrap();
    let css = fs::read_to_string(root.join("assets/fixture.css")).unwrap();
    let script = fs::read_to_string(root.join("assets/fixture.js")).unwrap();

    for required in [
        "assets/fixture.css",
        "assets/fixture.svg",
        "class=\"mermaid\"",
        "id=\"clipboard\"",
        "id=\"download\"",
        "id=\"external\"",
    ] {
        assert!(html.contains(required), "fixture is missing {required}");
    }
    assert!(css.contains("fixture.woff2"));
    assert!(script.contains("navigator.clipboard.writeText"));
    assert!(root.join("assets/download.txt").is_file());
    assert!(root.join("assets/fixture.svg").is_file());
    assert!(root.join("assets/fixture.woff2").is_file());
    assert!(root.join("assets/mermaid.min.js").is_file());
    assert!(root.join("assets/MERMAID-LICENSE").is_file());

    let combined = format!("{html}\n{css}\n{script}").to_ascii_lowercase();
    for forbidden in ["api_key", "token=", "password=", "authorization:"] {
        assert!(
            !combined.contains(forbidden),
            "fixture contains {forbidden}"
        );
    }
}
