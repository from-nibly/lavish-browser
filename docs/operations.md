# Installation and operations

## Source installation

From a clean checkout, use the pinned Nix environment and lockfile:

```bash
nix-shell --run './scripts/check.sh'
nix-shell --run './packaging/install.sh --prefix "$HOME/.local"'
```

The installer accepts `--prefix`, `--destdir`, `--profile debug|release`, and `--no-build`. GTK 4, WebKitGTK 6, `npx`, and upstream `lavish-axi` are runtime requirements. Zellij is optional.

## Launcher migration and rollback

Do not overwrite an existing local `lavish-open` script until the installed Rust launcher has passed a real open in your environment.

1. Install to a separate prefix and invoke `<prefix>/bin/lavish-open artifact.html` explicitly.
2. Confirm the upstream TOON is preserved and the session appears in Lavish Browser.
3. Back up or rename the old script, then put the new prefix before it on `PATH`.
4. To roll back, restore the previous `PATH` order or script. This does not migrate or alter upstream Lavish state.

The launcher forwards its arguments to `lavish-axi` and appends `--no-open` when absent. Upstream failures and `user-ended` behavior are preserved. There is no launcher-owned `--help`; run without arguments for local usage (it prints usage and exits nonzero) and consult upstream Lavish for its open options. `lavish-browser-ctl --help` likewise prints the control usage through its invalid-argument path and exits nonzero; use the forms documented below rather than treating it as a health check.

## Runtime and state paths

| Purpose | Default | Override |
|---|---|---|
| Control socket | `$XDG_RUNTIME_DIR/lavish-browser/control.sock` | `LAVISH_BROWSER_SOCKET` |
| Browser metadata | `$XDG_STATE_HOME/lavish-browser/state.json` | set `XDG_STATE_HOME`; fallback is `$HOME/.local/state` |
| Browser executable used by launcher | `lavish-browser` on `PATH` | `LAVISH_BROWSER_EXECUTABLE` |

The socket directory is mode `0700`; the socket is user-restricted and Linux peers are UID checked. Metadata contains browser project/document ordering, labels, source paths, session URLs, selections, and timestamps—not feedback or artifact contents. Writes are atomic and restrictive. Corrupt or unsupported state is ignored safely. On startup, Standalone entries remain and Zellij projects are reconciled by exact live session/stable-tab identity. Restored views begin dormant and materialize lazily.

## Diagnostics and recovery

- `lavish-browser-ctl ping` reports a healthy response or `{"status":"absent"}`.
- `lavish-browser-ctl status --json` returns the bounded read-only browser snapshot.
- If a view shows **Failed** or **Reconnect**, confirm upstream Lavish is running and run `lavish-open <file>` again. The browser does not start Lavish during reconnect or memory resume.
- If startup cannot create the socket, verify ownership and permissions of `$XDG_RUNTIME_DIR/lavish-browser`, or set `LAVISH_BROWSER_SOCKET` to a private path.
- If Zellij routing fails, verify both `ZELLIJ_SESSION_NAME` and `ZELLIJ_PANE_ID` and that `zellij action list-panes --all --json` can find the invoking pane.
- Closing browser tabs does not end upstream review. Use upstream `lavish-axi end <file>` only when review semantics require it.

`LAVISH_BROWSER_AUTOMATION`, `LAVISH_BROWSER_MEMORY_WARNING_FILE`, and debug-build `LAVISH_BROWSER_INSPECTOR` are test/diagnostic controls, not normal user configuration. Memory behavior is documented in [memory.md](memory.md).
