# Lavish Browser development

Lavish Browser is a Rust browser/container for sessions served by upstream
`lavish-axi`. It does not serve artifacts or implement Lavish review APIs.

## Development environment

Enter the Nix shell from the repository root:

```bash
nix-shell
```

The shell supplies Rust/Cargo, GTK 3 and 4, WebKitGTK 6,
`WebKitWebDriver`, the `wasm32-wasip1` standard library and linker, Clippy,
Selenium, AT-SPI/Dogtail, Xvfb, and `wmctrl`. GTK 3 is present for Dogtail;
the production browser targets plain GTK 4.

Outside Nix, equivalent Rust and development packages are required. Exact
package names vary by distribution. At minimum, install Rust with `rustfmt`,
Clippy, and the `wasm32-wasip1` target, plus GTK 4 and WebKitGTK 6 development
headers. Accessibility tests additionally need GTK 3 and AT-SPI typelibs,
Dogtail, and a session accessibility bus.

## Build and test

The repository check runs formatting, warning-free Clippy, all workspace tests,
and the Zellij plugin WASM build:

```bash
nix-shell --run ./scripts/check.sh
```

Individual checks are also available:

```bash
nix-shell --run 'cargo fmt --check'
nix-shell --run 'cargo check --workspace'
nix-shell --run 'cargo test -p lavish-browser-protocol'
nix-shell --run 'cargo test -p lavish-browser-core'
nix-shell --run 'cargo test --workspace'
nix-shell --run 'cargo build -p lavish-browser-zellij --target wasm32-wasip1'
```

Verify the native automation Python APIs, including their typelibs, with:

```bash
nix-shell --run 'python -c "import dogtail.tree; import pyatspi"'
```

## Running components

The production workspace currently establishes protocol/domain foundations and
component seams. These commands compile, but are intentionally skeletal and do
not yet launch or control a browser:

```bash
nix-shell --run 'cargo run -p lavish-browser'
nix-shell --run 'cargo run -p lavish-browser-cli --bin lavish-open'
nix-shell --run 'cargo run -p lavish-browser-cli --bin lavish-browser-ctl'
```

Likewise, `lavish-browser-zellij` currently exports only a target-compatible
plugin skeleton. It does not subscribe to Zellij events or invoke the control
helper yet.

The two `prototypes/` packages remain buildable reference implementations, not
production entry points. The WebView prototype requires an already-running,
loopback upstream Lavish `/session/…` URL:

```bash
nix-shell --run 'cargo run -p lavish-webview-prototype -- http://127.0.0.1:4387/session/example'
```

## Eventual runtime dependencies

The completed native workflow will require:

- Linux with GTK 4 and WebKitGTK 6 runtime libraries;
- `npx` and upstream `lavish-axi`, which continues to create and serve Lavish
  sessions; and
- Zellij only for contextual project routing and the optional lifecycle plugin.

WebKitWebDriver, Selenium, AT-SPI/Dogtail, Xvfb, and active-window tooling are
development/E2E dependencies, not ordinary end-user runtime requirements.
