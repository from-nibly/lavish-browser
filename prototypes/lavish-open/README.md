# `lavish-open` adapter prototype

This native prototype demonstrates the intended browser-only integration:

1. delegate session creation to `npx -y lavish-axi ... --no-open`;
2. forward upstream stdout/stderr unchanged;
3. parse `session.file`, `session.url`, and `session.status` from current TOON output;
4. map `ZELLIJ_PANE_ID` to stable `tab_id` with `zellij action list-panes --all --json`;
5. derive a recognizable label from Super Tabs `directory` and `worktree` metadata; and
6. send a versioned `open_url` JSON line to the browser's Unix-domain socket.

The mock listener makes the wire message visible:

```bash
nix-shell
cargo run -p lavish-open-prototype -- listen /tmp/lavish-browser-test.sock
```

In another terminal:

```bash
LAVISH_BROWSER_SOCKET=/tmp/lavish-browser-test.sock \
  cargo run -p lavish-open-prototype -- /absolute/path/to/artifact.html
```

## Zellij plugin subprocess bridge

A native launcher can use `std::os::unix::net::UnixStream`, but a normal Zellij plugin targets `wasm32-wasip1` (`target_family="wasm"`, not Unix). Zellij's plugin API does not expose arbitrary Unix-domain socket connections.

The selected integration uses Zellij's `run_command` host API with the `RunCommands` permission. On active/closed tab events, the plugin invokes a short-lived installed helper such as:

```text
lavish-browser-ctl select-project <zellij-session> <stable-tab-id>
lavish-browser-ctl close-project <zellij-session> <stable-tab-id>
```

The native helper sends the corresponding versioned message over the same user-restricted Unix socket used by `lavish-open`, then exits. If the helper or browser is unavailable, synchronization degrades to a no-op and the plugin should avoid repeated noisy retries.

This keeps browser control on one Unix-socket protocol and avoids adding a loopback HTTP endpoint or `WebAccess` permission.
