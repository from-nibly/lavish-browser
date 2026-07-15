# Lavish WebKitGTK compatibility prototype

This prototype deliberately does **not** serve artifacts or implement Lavish APIs. It accepts the normal loopback `/session/<key>` URL returned by upstream `lavish-axi --no-open` and loads it in WebKitGTK 6.

## Run

Enter the repository's Nix shell, create/resume a Lavish session without opening Chromium, then pass its returned URL to the prototype:

```bash
nix-shell
npx -y lavish-axi /absolute/path/to/artifact.html --no-open
cargo run -p lavish-webview-prototype -- http://127.0.0.1:4387/session/<key>
```

The **Inspector** button opens WebKit's inspector for compatibility investigation.

## Compatibility proof checklist

Test against a representative real Lavish session rather than a synthetic HTML page:

- [ ] Session chrome renders and the artifact iframe becomes visible after the layout gate.
- [ ] Relative local CSS, JavaScript, images, and fonts load through upstream Lavish.
- [ ] Live reload occurs after editing the source artifact.
- [ ] Annotation mode selects elements and text ranges.
- [ ] Conversation messages queue and wake an attached `lavish-axi poll`.
- [ ] `poll --agent-reply` updates the conversation/presence UI.
- [ ] Layout warnings reach the normal poll channel.
- [ ] Clipboard actions work or fail with understandable WebKit permission behavior.
- [ ] Export downloads to the expected GTK/WebKit download destination.
- [ ] External links and popups follow an explicit browser policy.
- [ ] Mermaid diagrams render.
- [ ] Excalidraw whiteboard opens, edits, persists, and queues feedback.
- [ ] Closing the GTK window leaves upstream Lavish session semantics unchanged.

A passing checklist proves browser compatibility. Failures should first be classified as WebKit policy/API integration issues; they do not justify porting Lavish server internals into this project.
