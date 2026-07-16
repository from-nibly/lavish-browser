# Memory-pressure suspension

Lavish Browser normally retains one WebKit WebView per materialized document. It listens to the desktop `GMemoryMonitor` `low-memory-warning` signal and releases inactive views in least-recently-used order:

- a low warning suspends one materialized inactive document;
- a medium warning suspends two; and
- a critical warning suspends every materialized inactive document.

The selected document in the selected project is always protected. Dormant documents have no WebView to release and are skipped.

Suspension removes the WebView from its GTK stack and drops the browser's strong reference to it. The document metadata, URL, title, ordering, and activation timestamp remain. Its sidebar row is visibly marked **Suspended**. A suspended view remains absent while its project is inactive, even though that project remembers its locally selected document. Selecting that project and document creates exactly one new WebView and loads the persisted upstream Lavish URL. This can lose volatile DOM, form, scroll, or annotation-editing state; `WebViewSessionState` is deliberately not used because it does not promise to preserve that state.

If upstream Lavish is no longer available, the recreated view shows the normal reconnect placeholder. The browser does not start or end Lavish as part of suspension or resume. Run `lavish-open <file>` explicitly to reacquire or refresh an unavailable session.

## Deterministic warning hook

For installed-app tests and diagnostics, start the browser with `LAVISH_BROWSER_MEMORY_WARNING_FILE` set to a private temporary file. Replacing that file's contents with `low`, `medium`, or `critical` triggers the same suspension path. Numeric GIO levels `50`, `100`, and `255` are also accepted. Change the file contents for each trigger (for example, write `1 medium`, then `2 medium`). Invalid values are ignored and logged.

This hook is disabled unless the environment variable is set. It does not alter the control protocol or expose a production browser action.

`GMemoryMonitor` availability and warning quality depend on the desktop memory-monitor backend. The deterministic hook remains available for diagnosis when the backend does not emit useful warnings; normal retention behavior is unchanged when no warning arrives.
