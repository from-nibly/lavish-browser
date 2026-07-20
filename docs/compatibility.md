# Lavish WebKitGTK compatibility gate

The compatibility gate drives the production `lavish-browser` WebView against an unchanged session served by upstream `lavish-axi`. It does not post to private Lavish APIs, serve artifacts, inject application JavaScript, or substitute a test renderer.

## Validated baseline

| Component | Baseline |
|---|---|
| Rust | 1.95.0 |
| GTK | 4.22.4 |
| WebKitGTK 6 | 2.52.4 |
| Rust `gtk4` / `webkit6` | 0.11.4 / 0.6.1 |
| upstream `lavish-axi` | 0.1.42 |
| Selenium | 4.40.0 |

The secret-free fixture is `tests/fixtures/lavish-compat/`. It contains representative HTML, local CSS/JS/SVG/WOFF2/download assets, Mermaid, and controls for browser policy checks.

## Release command and evidence contract

The release proof is the installed-system harness, run with no special environment:

```bash
nix-shell --run './scripts/e2e-installed.sh'
```

It builds and installs locked production artifacts into an isolated prefix, provisions private XDG paths plus display/D-Bus/AT-SPI services, and runs:

- real upstream Lavish open, poll, reply, layout warning, live reload, and browser-only close paths;
- the production WebKit view through WebKitWebDriver;
- native GTK controls through AT-SPI/Dogtail;
- download and external-handler checks;
- persistence and memory suspension/resume; and
- an isolated real Zellij session with the installed WASM and helper.

Each run writes an ignored bundle under `target/e2e/<run-id>/`. Durable outputs include:

- `manifest.json` and `integration/installed-artifacts.json` for versions, paths, sizes, and hashes;
- `compatibility/capabilities.json` for the automated/live browser matrix;
- `integration/scenarios.json` for installed scenarios;
- complete browser, WebDriver, upstream, poll, native, plugin, focus, persistence, and memory logs/evidence; and
- `installed-capabilities.json`, generated only when every required scenario and compatibility capability validates.

The tracked `tests/e2e/installed-capabilities.json` is only the required capability contract. It intentionally cannot claim that a future run passed. A release claim must point to a complete generated bundle whose top-level `installed-capabilities.json` has `status: "pass"` and classification `installed-live`.

For focused compatibility development, use:

```bash
nix-shell --run '
  cargo build --locked -p lavish-browser -p lavish-browser-cli &&
  tests/fixtures/lavish-compat/run-live.sh target/compatibility/production
'
```

That focused harness intentionally exits nonzero while a required human residual is unavailable. Unit tests, direct HTTP/API calls, or screenshots alone do not convert it to PASS.

## Human Excalidraw evidence

WebKitWebDriver cannot reliably produce a faithful Excalidraw canvas gesture. The Excalidraw capability therefore requires retained `manual-live` evidence containing:

- a human-created edit visible before reload and persisted after reload/reopen;
- feedback sent through the real browser surface;
- a successful owned `lavish-axi poll` result with `tag: whiteboard`; and
- the added element identity correlated between observation and poll.

The capability generator verifies the recorded installed `lavish-browser` SHA-256 against the freshly installed browser in the same release bundle before accepting retained manual evidence. When evidence is reused across a docs/packaging-only candidate, release validation must additionally compare the installed launcher, control helper, and WASM hashes and reject the residual if any required executable differs. The retained evidence remains ignored run input; documentation does not depend on a particular historical worktree path.

## Required capability matrix

| Capability | Required evidence |
|---|---|
| Production WebDriver and real DOM | W3C production context, URL/title/readiness and DOM assertions |
| Session chrome/layout gate/artifact iframe | Visible gate transition and real iframe |
| Relative CSS/JS/image/font | Real iframe asset assertions |
| Live reload | Source edit observed in retained iframe |
| Element and text-range annotations | Browser interaction wakes real poll |
| Message and agent reply/presence | Real `lavish-axi poll`/`poll --agent-reply` |
| Layout warning | Intentional overflow returned through poll |
| Mermaid | Rendered fixture SVG/whiteboard |
| Clipboard | Native clipboard read after live click |
| Export/download | Native chooser writes and verifies export |
| External link/popup | Isolated desktop handler receives exact URL |
| Excalidraw persistence/feedback | Exact-hash correlated human edit and real whiteboard poll |
| Browser-only close | Browser teardown without upstream `end` |

Automation is enabled only when `LAVISH_BROWSER_AUTOMATION=1`; normal application runs do not expose a WebDriver context.
