# Lavish Browser

Lavish Browser is a dedicated Linux desktop browser for local review sessions created and served by upstream [`lavish-axi`](https://github.com/kunchenguid/lavish-axi). It adds a single GTK window with project tabs, document lists, retained WebKitGTK views, persistence, and optional one-way HerdR or Zellij lifecycle synchronization.

It is **not** a general-purpose browser or a replacement for `lavish-axi`. Upstream Lavish remains a runtime dependency and continues to own artifact serving, review state, annotations, feedback and polling, live reload, layout checks, whiteboards, export, and session lifecycle.

## Requirements

- Linux with GTK 4 and WebKitGTK 6
- Rust 1.95 or newer for a source build
- `npx` with access to upstream `lavish-axi`
- HerdR for native workspace/tab routing, or Zellij 0.44.x for its optional plugin
- Nix (recommended for a reproducible build and validation environment)

## Build and install from a clean checkout

Set `LAVISH_BROWSER_REPOSITORY` to this repository's actual clone URL or an existing local repository path; no public URL is assumed here.

```bash
LAVISH_BROWSER_REPOSITORY='/replace/with/clone-url-or-local-path'
git clone "$LAVISH_BROWSER_REPOSITORY" lavish-browser
cd lavish-browser
nix-shell --run './scripts/check.sh'
nix-shell --run './packaging/install.sh --prefix "$HOME/.local"'
```

Ensure `$HOME/.local/bin` is on `PATH`. The installer builds with `Cargo.lock` and installs:

- `lavish-browser`
- `lavish-open`
- `lavish-browser-ctl`
- `lavish-browser-herdr-sync`
- `share/lavish-browser/zellij/lavish-browser-zellij.wasm`
- desktop application and icon metadata

Use `DESTDIR` for package staging, `--profile debug` for a debug install, or `--no-build` to install existing artifacts. Run `packaging/install.sh --help` for installer options.

## Use

Open a local HTML artifact with:

```bash
lavish-open path/to/artifact.html
```

`lavish-open` delegates to `npx -y lavish-axi … --no-open`, forwards the upstream output unchanged, and routes the returned loopback `/session/…` URL to the browser. The normal `lavish-axi poll`, `--agent-reply`, `end`, export/share, playbook, and design commands remain upstream commands.

Inside HerdR, the invoking pane maps to a project keyed by the HerdR session, workspace, and stable tab ID. Inside Zellij, it maps to the Zellij session and stable tab ID. Outside either runtime, documents open under **Standalone**. Opening the same canonical file in one project focuses its existing view. The same upstream session may appear in multiple projects; those views share upstream review state.

Closing a document or project only closes browser metadata and WebViews. It never runs `lavish-axi end`. If an upstream session becomes unavailable, reopen the file with `lavish-open` to reacquire and refresh its URL.

The control helper is intended primarily for the plugin and diagnostics:

```text
lavish-browser-ctl select-project <zellij-session> <stable-tab-id>
lavish-browser-ctl close-project  <zellij-session> <stable-tab-id>
lavish-browser-ctl select-herdr-project <herdr-session> <workspace-id> <tab-id>
lavish-browser-ctl close-herdr-project  <herdr-session> <workspace-id> <tab-id>
lavish-browser-ctl status [--json]
lavish-browser-ctl ping
```

`lavish-open` has no launcher-owned `--help`; arguments are forwarded to upstream Lavish. Running it without arguments prints its local usage.

## HerdR integration

`lavish-open` reads the stable HerdR identity exported to each pane. Run `lavish-browser-herdr-sync` as a user service to silently select existing browser projects when visible HerdR tabs change and close projects when their tabs or workspaces close. It never starts or raises the browser.

See [HerdR lifecycle setup](docs/herdr.md).

## Optional Zellij integration

The plugin silently selects an existing browser project when the active Zellij tab changes and closes the corresponding browser project when that tab closes. It never starts or raises the browser and never changes Zellij in response to browser navigation. Installation requires explicit `ReadApplicationState` and `RunCommands` permissions.

See [Zellij plugin setup](docs/zellij.md).

## Documentation

- [Architecture and scope](docs/architecture.md)
- [Installation, migration, and operations](docs/operations.md)
- [HerdR lifecycle helper](docs/herdr.md)
- [Zellij plugin](docs/zellij.md)
- [Security model](docs/security.md)
- [Memory-pressure behavior](docs/memory.md)
- [WebKitGTK/Lavish compatibility evidence](docs/compatibility.md)
- [Development and validation](docs/development.md)

## Release validation

Run the reproducible repository gate:

```bash
nix-shell --run './scripts/check.sh'
```

The installed real-system gate is separate and required for release claims:

```bash
nix-shell --run './scripts/e2e-installed.sh'
```

It installs into an isolated prefix and records ignored evidence under `target/e2e/<run-id>/`. The tracked capability contract cannot itself claim PASS; inspect the generated manifest, logs, and capability matrix. Unit tests alone are not release proof.
