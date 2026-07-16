# Installed end-to-end release smoke

Run the release gate from the development shell:

```bash
nix-shell --run 'LAVISH_MANUAL_EXCALIDRAW=1 scripts/e2e-installed.sh'
```

The harness builds and stages release artifacts beneath an isolated prefix,
then uses only those installed binaries and the installed WASM plugin. It
creates private XDG runtime/state/config/data directories and an isolated X11,
D-Bus, AT-SPI and Zellij environment. Upstream sessions and polls are always
provided by real `npx -y lavish-axi`; WebKit content is driven through
WebKitWebDriver and native chrome through AT-SPI/Dogtail. Fake servers or mocked
control clients are not accepted by this release gate.

Evidence is retained at `target/e2e/<run-id>/` and includes an installation
manifest, versions, complete compatibility evidence, browser/control output,
Zellij/plugin logs, focus samples, persistence snapshots, and memory/process
samples. The script exits nonzero if a required tool, display/accessibility
bus, live scenario, or manual residual is unavailable. In particular,
`LAVISH_MANUAL_EXCALIDRAW=1` requires the operator to complete the real
whiteboard gesture when prompted; omitting it is intentionally a failing
release blocker.

For package staging without running E2E:

```bash
scripts/install.sh --prefix /usr --destdir "$PWD/target/package-root"
```

The installed files are:

- `$prefix/bin/lavish-browser`
- `$prefix/bin/lavish-open`
- `$prefix/bin/lavish-browser-ctl`
- `$prefix/share/applications/works.from-nibly.LavishBrowser.desktop`
- `$prefix/share/icons/hicolor/scalable/apps/works.from-nibly.LavishBrowser.svg`
- `$prefix/share/lavish-browser/zellij/lavish-browser-zellij.wasm`

Keep any existing local Bash `lavish-open` earlier on a rollback path until an
installed live run passes. The installer does not remove or overwrite files
outside the selected prefix.
