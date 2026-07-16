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

Build the production binaries, then run the display/D-Bus/AT-SPI wrapper:

```bash
nix-shell --run '
  cargo build -p lavish-browser -p lavish-browser-cli &&
  tests/fixtures/lavish-compat/run-live.sh target/compatibility/production
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

The retained evidence bundle is
`target/compatibility/evidence/final-live-3/`. The original “W3C hang” was two
separate test/controller defects, not a WebKit command deadlock:

1. the controlled production view was being navigated before WebDriver owned
   its new browsing context; and
2. the inherited 911-pixel display caused the real Lavish layout gate to remain
   active. On an isolated 1600×1000 Xvfb display, bounded W3C commands returned
   in milliseconds. `w3c-trace.json` records blank-context lookup, navigation,
   one selected handle, real URL/title, `document.readyState=complete`, and the
   first `#layoutGateOverlay` lookup.

The run proves real annotation, message/reply, live reload, layout warning,
Mermaid, clipboard, download chooser/write, external handler, text range, and
browser-only close paths. Download testing found and fixed three production
policy defects: same-origin blob export was rejected, `FileDialog` had no
non-portal chooser fallback, and WebKitGTK 6 expects an absolute destination
path rather than a `file:` URI.

The complete release gate remains **blocked, not passed**, only on the required
Excalidraw edit/persistence/feedback residual. The inline real upstream
whiteboard loads and is visible. Both WebKitWebDriver and a native AT-SPI
pointer attempt reached the real editor, but the automated canvas gesture was
not accepted faithfully enough to queue feedback. No Lavish API or internals
were used as a substitute.

## Capability matrix

| Capability | Result | Evidence |
|---|---|---|
| Production WebDriver and real DOM | PASS | Custom production capabilities plus timed URL/title/ready-state/element trace |
| Session chrome/layout gate/artifact iframe | PASS | Gate cleared on isolated 1600×1000 display |
| Relative CSS/JS/image/font | PASS | Real iframe assertions |
| Live reload | PASS | Copied source edit observed in retained iframe |
| Element annotation | PASS | Production SDK card sent and real poll woke |
| Text-range annotation | PASS | Live pointer range card sent and real poll woke |
| Message waking real poll | PASS | Owned poll output retained |
| Agent reply/presence | PASS | Real `poll --agent-reply` rendered and was woken |
| Layout warning poll | PASS | Intentional 200vw edit returned a real warning poll |
| Mermaid rendering | PASS | Vendored fixture Mermaid rendered SVG and inline whiteboard |
| Clipboard | PASS | Live click plus exact native X11 clipboard read |
| Export/download | PASS | Live AT-SPI chooser wrote and verified 3,691,256-byte export |
| External link/popup | PASS | Isolated desktop HTTPS handler received exact URL |
| Excalidraw persistence/feedback | BLOCKED | Real editor loaded; faithful edit/queue/persistence proof incomplete |
| Browser-only close | PASS | Teardown did not call upstream end |

`BLOCKED` remains a failing gate outcome and must not be relabeled from unit,
direct HTTP, or private API evidence.

## Remaining live manual residual

Open the real inline Mermaid whiteboard in the visible production app, make a
visible Excalidraw edit, close/reopen it, verify persistence, queue its feedback,
and retain the real poll output. Manual observations must be labeled
`manual-live`; WebDriver, screenshots, or API calls are supporting evidence,
not substitutes.
