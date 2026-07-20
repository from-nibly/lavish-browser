# Optional Zellij lifecycle plugin

The plugin is not required for normal opens. `lavish-open` resolves its invoking pane directly. The plugin only adds one-way lifecycle synchronization:

- active known tab → silently select its existing browser project;
- unknown tab → no-op;
- closed tab → close its browser project and retained views;
- browser project changes → never change Zellij.

It does not launch or raise the browser. A missing helper, missing browser, or ignored project degrades quietly.

## Install

The source installer places the WASM at:

```text
<PREFIX>/share/lavish-browser/zellij/lavish-browser-zellij.wasm
```

Copy and edit [`examples/zellij/lavish-browser.kdl`](../examples/zellij/lavish-browser.kdl), using absolute installed paths for both the WASM and `lavish-browser-ctl`:

```kdl
load_plugins {
    "file:/home/me/.local/share/lavish-browser/zellij/lavish-browser-zellij.wasm" {
        helper_path "/home/me/.local/bin/lavish-browser-ctl"
        debug "false"
    }
}
```

On first load, approve exactly `ReadApplicationState` and `RunCommands`. `RunCommands` is required because Zellij WASM cannot open the Unix socket; the plugin invokes the native helper with direct argv. It does not request `WebAccess` and exposes no HTTP control endpoint.

Use `debug "true"` only for bounded diagnostics. If synchronization is absent, confirm permissions, absolute paths, Zellij 0.44 compatibility, and `lavish-browser-ctl ping`. Normal `lavish-open` remains functional if the plugin is removed.
