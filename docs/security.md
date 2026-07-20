# Security model

Lavish Browser accepts executable, potentially untrusted artifact content only through the complete page served by upstream Lavish. The browser does not read local artifact files, serve sibling assets, expose a filesystem bridge, or implement review endpoints.

Browser-control submissions must be syntactically valid HTTP(S) URLs without credentials, on `127.0.0.1`, `::1`, or `localhost`, with a non-empty `/session/…` path. Arbitrary remote top-level pages and unsafe schemes are rejected. User-activated external HTTP(S) links are handed to the desktop browser; normal remote subresources requested by the upstream page remain governed by WebKit.

The versioned newline-delimited JSON protocol has bounded messages and explicit acknowledgements. Its Unix socket lives in a current-user directory with restrictive permissions, and supported Linux connections verify peer UID. Labels, Zellij metadata, paths, and URLs are treated as untrusted display/input values.

The optional Zellij plugin requests `ReadApplicationState` and `RunCommands`; it uses direct argv to invoke the installed control helper and never requests `WebAccess`. Browser controls cannot run arbitrary commands. Close, reconnect, persistence, and memory-pressure operations never call upstream `lavish-axi end`.

Upstream Lavish remains responsible for artifact sandboxing, asset confinement, review data, sharing warnings, and server lifecycle. Consult its security guidance before using export/share or including secrets in artifacts.
