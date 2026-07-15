# Lavish WebKitGTK production compatibility gate

This gate tests the production `lavish-browser` WebView against an unchanged
session served by upstream `lavish-axi`. It does not post to private Lavish APIs,
serve artifacts, inject JavaScript into the application, or substitute a test
renderer.

## Baseline recorded 2026-07-15

| Component | Version |
|---|---|
| Rust | 1.95.0 |
| GTK | 4.22.4 |
| WebKitGTK 6 | 2.52.4 |
| Rust `gtk4` / `webkit6` | 0.11.4 / 0.6.1 |
| upstream `lavish-axi` | 0.1.42 |
| Selenium | 4.40.0 |
| WebKitWebDriver | WebKitGTK 2.52.4 |

The checked-in fixture is `tests/fixtures/lavish-compat/`. It contains only
representative, secret-free HTML, local CSS/JS/SVG/WOFF2/download assets, and
controls for browser policy checks. The JetBrains Mono WOFF2 fixture is from the
open-source JetBrains Mono 2.304 package supplied by the development shell.

## Automated live harness

Build the production binaries, start a display and accessibility-capable D-Bus
session, then run:

```bash
nix-shell --run '
  cargo build -p lavish-browser -p lavish-browser-cli &&
  python -u tests/fixtures/lavish-compat/run.py \
    --browser target/debug/lavish-browser \
    --launcher target/debug/lavish-open \
    --webdriver WebKitWebDriver \
    --evidence target/compatibility/production
'
```

The harness copies the fixture before live-reload/layout-warning edits, uses
short private XDG paths, invokes real `lavish-axi --no-open`, routes through the
production launcher, launches the production binary through
WebKitWebDriver, and owns real `lavish-axi poll`/`poll --agent-reply` child
processes. Evidence includes versions, upstream and launcher output, browser and
driver logs, poll results, a screenshot, persisted browser metadata, and
`capabilities.json`.

Automation is opt-in with `LAVISH_BROWSER_AUTOMATION=1`. WebDriver's requested
browsing context is a `WebView` created by the same production
`DocumentView` constructor, signals, settings, navigation policy, and retained
registry as normal documents. The required WebKit
`is-controlled-by-automation` construct property is set; without it,
WebKitWebDriver returns `session not created: failed to create a new browsing
context`. Automation remains disabled by default.

The harness exits nonzero while any required manual residual has status
`not_run`; this is deliberate. Unit/API-only evidence must not turn that result
into a pass.

## Live run result in this implementation leg

Evidence was attempted under `target/compatibility/evidence/live-*` on the
baseline above. A real custom-binary W3C session was successfully created and
reported `browserName: Lavish Browser`, `browserVersion: 0.1`, proving the
production automation handshake and controlled production WebView. Normal
production smoke from the preceding controller gate loaded the same real
upstream session successfully.

The complete release gate is **blocked**, not passed: after navigating the
controlled production view to the real Lavish session, WebKitWebDriver 2.52.4
stops returning W3C DOM commands even though the underlying `WebView` reports
that loading has ended. Multiple bounded harness attempts reached “WebDriver
session created” and “production controller navigation started” but could not
obtain `#layoutGateOverlay`; logs are retained in the ignored evidence paths.
This prevents honest automated proof of downstream DOM/poll/reload paths and
also prevents the required manual residuals from being performed in this agent
run. No Lavish internals were patched to work around the incompatibility.

## Capability matrix

| Capability | Result | Required evidence |
|---|---|---|
| Production WebDriver session creation | PASS | Real WebKitWebDriver launched `target/debug/lavish-browser`; custom application capabilities returned |
| Session chrome/layout gate/artifact iframe | BLOCKED | W3C DOM command stalls after real-session navigation |
| Relative CSS/JS/image/font | BLOCKED | Must be asserted inside the real artifact iframe |
| Live reload | BLOCKED | Must edit the copied source and observe the production iframe |
| Element annotation | BLOCKED | Must drive real iframe pointer interaction |
| Text-range annotation | NOT RUN | Explicit live pointer-range selection required |
| Message waking real poll | BLOCKED | Harness has tracked real poll process, but cannot reach composer |
| Agent reply/presence | BLOCKED | Requires completed browser message/reply loop |
| Layout warning poll | BLOCKED | Requires observable source-edit/reload loop |
| Mermaid rendering | BLOCKED | Must observe real rendered diagram/inline whiteboard |
| Clipboard | NOT RUN | Explicit live desktop clipboard check required |
| Export/download | NOT RUN | Explicit live GTK chooser and file-content check required |
| External link/popup | NOT RUN | Explicit live desktop handler and focus check required |
| Excalidraw persistence/feedback | NOT RUN | Explicit live gestures, reopen persistence, and real poll feedback required |
| Browser-only close | PASS (preceding production gate) | Closing retained production view did not invoke upstream end |

`BLOCKED` and `NOT RUN` are failing gate outcomes. They must not be relabeled as
passes based on unit tests, direct HTTP requests, or private API calls.

## Required live manual residual procedure

After the W3C blocker is resolved, perform these in the visible production app
and add exact results to the evidence manifest:

1. Click the fixture clipboard button and paste into an unrelated native text
   field; record exact pasted text or permission error.
2. Use **Export standalone HTML** and the fixture download link, complete the GTK
   destination chooser, and compare written bytes with the expected asset.
3. Activate the external target link and popup; record desktop-handler launch,
   target URL, retained Lavish tab, and focus behavior.
4. Open the Mermaid whiteboard, make a visible Excalidraw edit, close/reopen it,
   verify persistence, send its feedback, and retain the real poll output.
5. Select a text range with live pointer input and verify its annotation reaches
   the conversation and poll output.

Manual observations must be labeled `manual-live`; WebDriver, screenshots, or
API calls are supporting evidence, not substitutes.
