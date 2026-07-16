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

The complete release gate now **passes**. The final human residual is retained
at `target/compatibility/evidence/manual-residual-6/`: the user added one
rectangle, and the owned unchanged `lavish-axi poll` returned a `whiteboard`
payload identifying added element `279VBcgiM_4nEYTYcWCG-` and the real scene and
preview paths. The edited scene remained visible after a full production browser
close/reopen and another top-left Reload. This is classified `manual-live`; no
Lavish API, private internals, or synthetic feedback was used as a substitute.

That residual also exposed a production controller defect: a load lifecycle
render tried to attach a retained document widget while it was still parented
to the previous `GtkStack`, producing duplicate-child/parent assertions and a
blank surface despite a `Ready` model and unchanged URL/title. Rendering now
detaches retained document widgets before rebuilding their project stacks and
rejects nested render entry. A fixed-binary live reload retained the visible
whiteboard with no GTK criticals.

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
| Excalidraw persistence/feedback | PASS | Human-added rectangle returned by owned real whiteboard poll and persisted across full browser reopen/reload |
| Browser-only close | PASS | Teardown did not call upstream end |

The Excalidraw result is specifically `manual-live`; automated canvas gestures,
unit tests, screenshots alone, direct HTTP, and private APIs do not qualify as
substitutes for this evidence.
