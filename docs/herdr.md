# HerdR lifecycle integration

`lavish-open` detects the stable identity that HerdR exports to every pane:

- `HERDR_SESSION`
- `HERDR_WORKSPACE_ID`
- `HERDR_TAB_ID`
- `HERDR_SOCKET_PATH`

A launch is assigned to exactly that HerdR tab, even if another tab becomes visible while upstream Lavish starts. Missing or inconsistent HerdR identity is an error rather than a silent Standalone fallback.

## Lifecycle helper

Run one helper for each named HerdR session whose visible tabs should control Lavish Browser:

```bash
lavish-browser-herdr-sync --session main
```

The helper connects to HerdR's user-owned Unix socket, reads the initial session snapshot, and subscribes to workspace/tab focus and closure events. It provides one-way synchronization:

- visible known HerdR tab → silently select its existing browser project;
- unknown tab → no-op;
- closed tab or workspace → close the corresponding browser project and retained views;
- browser navigation → never change HerdR.

Focus events do not start or raise Lavish Browser. The helper reconnects after a HerdR server restart. Use `--socket PATH` for a nonstandard or test socket and `--once` to synchronize only the current snapshot.

A typical systemd user service is:

```ini
[Unit]
Description=Synchronize HerdR tabs with Lavish Browser

[Service]
ExecStart=%h/.local/bin/lavish-browser-herdr-sync --session main
Restart=on-failure
RestartSec=1

[Install]
WantedBy=default.target
```

HerdR's API is accessed only through its local Unix socket. No plugin, HTTP endpoint, or browser-to-HerdR control path is required.
