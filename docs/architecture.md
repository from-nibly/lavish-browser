# Architecture and product boundary

```text
local HTML path
    │
    ▼
lavish-open ── npx -y lavish-axi … --no-open
    │                         │
    │                         └─ upstream server, review state, polling,
    │                            live reload, annotations and whiteboards
    ▼
user-only Unix socket
    │
    ▼
GTK4 Lavish Browser ── retained WebKitGTK view of the unchanged /session/ URL
```

The Rust workspace separates the browser control plane from upstream Lavish:

- `protocol` defines bounded, versioned commands and loopback session URL validation.
- `core` owns project/document state, labels, persistence DTOs, reconciliation, and LRU policy without GUI types.
- `control` implements the user-restricted Unix socket client/server.
- `browser-cli` provides `lavish-open`, `lavish-browser-ctl`, and the native HerdR lifecycle helper.
- `browser-app` provides the single-instance plain GTK4 shell and retained WebKitGTK views.
- `zellij-plugin` is an optional WASM lifecycle adapter.

The browser loads normal upstream HTTP(S) loopback `/session/…` URLs. It does not read or serve artifacts, register a custom content URI, proxy Lavish routes, inject a native review bridge, or implement Lavish APIs. Remote subresources used by a local artifact remain subject to WebKit policy, but remote top-level documents cannot be submitted over browser control.

A project is `HerdR(session name, workspace ID, stable tab ID)`, `Zellij(session name, stable tab ID)`, or `Standalone`. Labels and working directories are display metadata, never identity. A document is keyed by project plus canonical source path. Duplicate paths in different projects retain separate WebViews but share the globally path-keyed upstream Lavish session and review state.

`OpenUrl` may present the application. Plugin `SelectProject` and `CloseProject` calls do not present it; browser navigation never sends commands back to Zellij. Browser close operations only release views and browser metadata, leaving upstream sessions unchanged.
