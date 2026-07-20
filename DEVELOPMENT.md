# Lavish Browser development

Lavish Browser is a production Rust browser/container for unchanged local sessions served by upstream `lavish-axi`. It does not serve artifacts or implement Lavish review APIs. Product boundaries and crate responsibilities are documented in [`docs/architecture.md`](docs/architecture.md).

## Environment

Enter the reproducible development shell from the repository root:

```bash
nix-shell
```

The shell supplies Rust/Cargo, GTK 4 and WebKitGTK 6, the `wasm32-wasip1` target, Clippy, WebKitWebDriver/Selenium, AT-SPI/Dogtail, Xvfb, Zellij, and active-window tooling. GTK 3 is present only for Dogtail. Outside Nix, equivalent development libraries and tools are required.

Ordinary runtime dependencies are Linux GTK 4/WebKitGTK 6 plus `npx` and upstream `lavish-axi`. Zellij is required only for contextual routing and the optional lifecycle plugin. Browser automation and accessibility tools are development/E2E dependencies.

## Build and checks

Run the complete lockfile-backed repository check:

```bash
nix-shell --run './scripts/check.sh'
```

It enforces formatting, warning-denied Clippy for every workspace target, all workspace tests, and the Zellij plugin `wasm32-wasip1` build. Focused commands and production `cargo run` examples are in [`docs/development.md`](docs/development.md).

## Install and run

Build and install the production browser, launcher, control helper, desktop metadata, and optional plugin:

```bash
nix-shell --run './packaging/install.sh --prefix "$HOME/.local"'
lavish-open path/to/artifact.html
```

See [`README.md`](README.md) for clean-checkout instructions and usage, [`docs/operations.md`](docs/operations.md) for migration, paths, persistence, and troubleshooting, and [`docs/zellij.md`](docs/zellij.md) for the plugin's required `ReadApplicationState` and `RunCommands` permissions.

## Release evidence

Unit and component tests are necessary but not release proof. Run the installed real-system gate:

```bash
nix-shell --run './scripts/e2e-installed.sh'
```

It installs into an isolated prefix and exercises the production GTK/WebKit application, real upstream Lavish open/poll/reply/live reload, WebKitWebDriver, AT-SPI/Dogtail, downloads, persistence, memory suspension, and a real isolated Zellij plugin session. Evidence is written under ignored `target/e2e/<run-id>/`; inspect the generated manifest and complete capability matrix. The Excalidraw result requires exact-hash retained human `manual-live` evidence and cannot be replaced by unit, API, or screenshot-only evidence.

The detailed compatibility contract and recorded browser results are in [`docs/compatibility.md`](docs/compatibility.md), with memory behavior in [`docs/memory.md`](docs/memory.md) and the security boundary in [`docs/security.md`](docs/security.md).
