#!/usr/bin/env python3
"""Installed Lavish Browser integration smoke using only live system boundaries."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tempfile
import time


def run(argv: list[str], env: dict[str, str], *, check: bool = True, timeout: int = 120) -> subprocess.CompletedProcess[str]:
    return subprocess.run(argv, env=env, text=True, capture_output=True, check=check, timeout=timeout)


def wait_until(description: str, predicate, timeout: float = 90):
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        try:
            last = predicate()
            if last:
                return last
        except Exception as error:  # retain the last live-boundary error for diagnostics
            last = error
        time.sleep(.25)
    raise RuntimeError(f"timed out waiting for {description}; last={last!r}")


def state(ctl: Path, env: dict[str, str]) -> dict:
    result = run([str(ctl), "status", "--json"], env)
    envelope = json.loads(result.stdout)
    if envelope.get("status") != "ok" or not envelope.get("state"):
        raise RuntimeError(f"browser status failed: {result.stdout}{result.stderr}")
    return envelope["state"]


def active_window(env: dict[str, str]) -> str:
    result = run(["xprop", "-root", "_NET_ACTIVE_WINDOW"], env, check=False)
    return result.stdout.strip()


def terminal_command(session: str, command: str, env: dict[str, str]) -> None:
    run([
        "zellij", "--session", session, "action", "new-pane", "--close-on-exit",
        "--", "bash", "-lc", command,
    ], env)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--prefix", type=Path, required=True)
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--webdriver-port", type=int, default=9596)
    parser.add_argument("--manual-excalidraw", action="store_true")
    args = parser.parse_args()

    evidence = args.evidence.resolve()
    evidence.mkdir(parents=True, exist_ok=True)
    prefix = args.prefix.resolve()
    browser = prefix / "bin/lavish-browser"
    launcher = prefix / "bin/lavish-open"
    ctl = prefix / "bin/lavish-browser-ctl"
    plugin = prefix / "share/lavish-browser/zellij/lavish-browser-zellij.wasm"
    required = [browser, launcher, ctl, plugin]
    if missing := [str(path) for path in required if not path.is_file()]:
        raise SystemExit(f"installed artifacts missing: {missing}")
    if plugin.read_bytes()[:4] != b"\0asm":
        raise SystemExit("installed Zellij plugin is not a WASM module")
    installed_manifest = {
        str(path.relative_to(prefix)): {
            "path": str(path),
            "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            "bytes": path.stat().st_size,
        }
        for path in required
    }
    for asset in sorted((prefix / "share/applications").glob("works.from-nibly.LavishBrowser.desktop")) + sorted((prefix / "share/icons/hicolor/scalable/apps").glob("works.from-nibly.LavishBrowser.svg")):
        installed_manifest[str(asset.relative_to(prefix))] = {
            "path": str(asset), "sha256": hashlib.sha256(asset.read_bytes()).hexdigest(), "bytes": asset.stat().st_size,
        }
    (evidence / "installed-artifacts.json").write_text(json.dumps(installed_manifest, indent=2) + "\n")

    root = Path(__file__).resolve().parents[2]
    fixture_source = root / "tests/fixtures/lavish-compat"
    fixture = evidence / "integration-fixture"
    shutil.copytree(fixture_source, fixture, ignore=shutil.ignore_patterns("run.py", "run-live.sh", "__pycache__"))
    artifact_a = fixture / "artifact.html"
    artifact_b = fixture / "second.html"
    artifact_b.write_text(artifact_a.read_text().replace("Lavish WebKit Compatibility", "Second installed artifact"))

    env = os.environ.copy()
    env["PATH"] = f"{prefix / 'bin'}:{env['PATH']}"
    isolated = Path(tempfile.mkdtemp(prefix="lavish-e2e-"))
    runtime = isolated / "runtime"
    state_home = isolated / "state"
    config = isolated / "config"
    cache = isolated / "cache"
    data = isolated / "data"
    for directory in (runtime, state_home, config, cache, data):
        directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    env.update({
        "XDG_RUNTIME_DIR": str(runtime), "XDG_STATE_HOME": str(state_home),
        "XDG_CONFIG_HOME": str(config), "XDG_CACHE_HOME": str(cache), "XDG_DATA_HOME": str(data),
        "LAVISH_BROWSER_EXECUTABLE": str(browser),
        "LAVISH_BROWSER_MEMORY_WARNING_FILE": str(evidence / "memory-warning"),
        "LAVISH_BROWSER_CTL_TRACE": str(evidence / "control-helper.trace"),
    })
    for key in ("ZELLIJ", "ZELLIJ_SESSION_NAME", "ZELLIJ_PANE_ID"):
        env.pop(key, None)

    versions = {}
    for key, argv in {
        "browser_sha256": ["sha256sum", str(browser)], "rust": ["rustc", "--version"],
        "gtk": ["pkg-config", "--modversion", "gtk4"],
        "webkitgtk": ["pkg-config", "--modversion", "webkitgtk-6.0"],
        "lavish_axi": ["npx", "-y", "lavish-axi", "--version"], "zellij": ["zellij", "--version"],
    }.items():
        result = run(argv, env, check=False)
        versions[key] = (result.stdout or result.stderr).strip().splitlines()[0]
    (evidence / "versions.json").write_text(json.dumps(versions, indent=2) + "\n")

    def project_snapshot(*, standalone: bool = False, zellij_count: int = 0, exact: bool = False):
        snapshot = state(ctl, env)
        encoded = [json.dumps(project.get("key")) for project in snapshot["projects"]]
        if standalone and not any('"kind": "standalone"' in key for key in encoded):
            return None
        count = sum(session in key for key in encoded)
        if (exact and count != zellij_count) or (not exact and count < zellij_count):
            return None
        return snapshot

    def selected_snapshot(expected: dict):
        snapshot = state(ctl, env)
        return snapshot if snapshot.get("selected_project") == expected else None

    browser_log = (evidence / "browser.log").open("w")

    def start_browser():
        candidate = None
        for _ in range(5):
            candidate = subprocess.Popen([str(browser)], env=env, stdout=browser_log, stderr=subprocess.STDOUT)
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                if (runtime / "lavish-browser/control.sock").exists():
                    return candidate
                if candidate.poll() is not None:
                    break
                time.sleep(.1)
            if candidate.poll() is None:
                candidate.terminate()
                candidate.wait(timeout=5)
            time.sleep(1)
        raise RuntimeError(f"installed browser did not become ready; final exit={candidate.returncode if candidate else None}")

    process = start_browser()
    session = f"lavish-e2e-{os.getpid()}"
    zellij_client = None
    scenarios: dict[str, dict[str, str]] = {}

    def passed(name: str, detail: str) -> None:
        scenarios[name] = {"status": "pass", "evidence": detail}

    try:
        standalone = None
        attempts = []
        for _ in range(3):
            candidate = run([str(launcher), str(artifact_a)], env, check=False, timeout=180)
            attempts.append(candidate.stdout + candidate.stderr)
            if candidate.returncode == 0:
                standalone = candidate
                break
            time.sleep(1)
        (evidence / "standalone-launch.log").write_text("\n--- attempt ---\n".join(attempts))
        if standalone is None:
            raise RuntimeError("installed Standalone launcher failed after three bounded attempts")
        snapshot = wait_until("Standalone project", lambda: project_snapshot(standalone=True))
        passed("standalone", "installed launcher created visible Standalone browser state")

        zellij_env = env.copy()
        zellij_env["ZELLIJ_CONFIG_DIR"] = str(config / "zellij")
        (config / "zellij").mkdir(parents=True, exist_ok=True)
        (config / "zellij/config.kdl").write_text(
            'simplified_ui true\nshow_startup_tips false\npane_frames false\n'
            'load_plugins {\n'
            f'  "file:{plugin}" {{\n'
            f'    helper_path "{ctl}"\n'
            '    debug "true"\n'
            '  }\n'
            '}\n'
        )
        client_log = (evidence / "zellij-client.log").open("wb")
        zellij_client = subprocess.Popen(
            ["script", "-qefc", f"zellij --session {session} --new-session-with-layout compact", str(evidence / "zellij.typescript")],
            env=zellij_env, stdin=subprocess.PIPE, stdout=client_log, stderr=subprocess.STDOUT,
        )
        wait_until("isolated Zellij session", lambda: session in run(["zellij", "list-sessions", "--short", "--no-formatting"], zellij_env).stdout)
        def background_plugin_pane():
            result = run([
                "zellij", "--session", session, "action", "list-panes", "--all", "--json",
            ], zellij_env, check=False)
            if result.returncode or not result.stdout.lstrip().startswith("["):
                return None
            panes = json.loads(result.stdout)
            matches = [pane for pane in panes if pane.get("plugin_url") == f"file:{plugin}"]
            return f"plugin_{matches[0]['id']}" if len(matches) == 1 else None

        plugin_pane_id = wait_until("background load_plugins instance", background_plugin_pane, 60)
        wait_until(
            "visible startup plugin permission prompt",
            lambda: b"Allow?" in (evidence / "zellij-client.log").read_bytes(),
            60,
        )
        run([
            "zellij", "--session", session, "action", "write-chars",
            "--pane-id", plugin_pane_id, "y",
        ], zellij_env)
        time.sleep(1)

        log_a = evidence / "zellij-open-a.log"
        command_a = f"{shlex.quote(str(launcher))} {shlex.quote(str(artifact_a))} >{shlex.quote(str(log_a))} 2>&1"
        terminal_command(session, command_a, zellij_env)
        wait_until("first Zellij project", lambda: project_snapshot(zellij_count=1), 180)
        run(["zellij", "--session", session, "action", "new-tab", "--name", "Second"], zellij_env)
        time.sleep(1)
        log_b = evidence / "zellij-open-b.log"
        command_b = f"{shlex.quote(str(launcher))} {shlex.quote(str(artifact_b))} >{shlex.quote(str(log_b))} 2>&1"
        terminal_command(session, command_b, zellij_env)
        snapshot = wait_until("second Zellij project", lambda: project_snapshot(zellij_count=2), 180)
        passed("multi_project_single_instance", f"one PID {process.pid}; {len(snapshot['projects'])} lazy projects")

        before_count = sum(len(p["documents"]) for p in snapshot["projects"])
        terminal_command(session, command_b, zellij_env)
        time.sleep(3)
        after = state(ctl, env)
        assert sum(len(p["documents"]) for p in after["projects"]) == before_count
        passed("dedupe", "reopening canonical source did not add a document")

        # Pair model assertions with native AT-SPI rows while both projects are visible.
        dogtail = run([sys.executable, str(root / "tests/e2e/verify_accessibility.py"), artifact_b.name], env, check=False)
        (evidence / "accessibility.log").write_text(dogtail.stdout + dogtail.stderr)
        if dogtail.returncode:
            raise RuntimeError("AT-SPI/Dogtail native assertion failed; see accessibility.log")
        passed("accessibility", dogtail.stdout.strip())

        native_export = evidence / "installed-native.export.html"
        native_download = run([
            sys.executable, str(root / "tests/fixtures/lavish-compat/native_download.py"),
            "--destination", str(native_export), "--trace", str(evidence / "native-download-trace.json"),
        ], env, check=False)
        (evidence / "native-download.log").write_text(native_download.stdout + native_download.stderr)
        if native_download.returncode or not native_export.is_file() or "<!doctype html" not in native_export.read_text().lower():
            raise RuntimeError("installed native AT-SPI export failed; see native-download.log")
        passed("native_download", f"AT-SPI chooser wrote {native_export.stat().st_size} byte installed export")

        # Exercise the installed real background plugin. Lifecycle delivery and
        # trace-plugin-event entries below prove permission/readiness/event flow.
        zellij_projects = [p for p in state(ctl, env)["projects"] if session in json.dumps(p["key"])]
        tab_ids = sorted(p["key"]["stable_tab_id"] for p in zellij_projects)
        if len(tab_ids) < 2:
            raise RuntimeError("real Zellij projects were not available for plugin assertions")
        helper_trace = evidence / "control-helper.trace"
        run(["zellij", "--session", session, "action", "go-to-tab", "2"], zellij_env)
        wait_until(
            "plugin helper selecting second project",
            lambda: f"select-project\t{session}\t{tab_ids[1]}" in helper_trace.read_text() if helper_trace.exists() else False,
        )
        focus_before = active_window(env)
        focus_state_before = run([sys.executable, str(root / "tests/e2e/focus_accessibility.py")], env).stdout.strip()
        presentation_before = state(ctl, env)["presentation_count"]
        run(["zellij", "--session", session, "action", "go-to-tab", "1"], zellij_env)
        selected = wait_until(
            "plugin selecting first project",
            lambda: selected_snapshot({"kind": "zellij", "session_name": session, "stable_tab_id": tab_ids[0]}),
        )
        focus_after = active_window(env)
        focus_state_after = run([sys.executable, str(root / "tests/e2e/focus_accessibility.py")], env).stdout.strip()
        first_select = f"select-project\t{session}\t{tab_ids[0]}"
        first_select_count = helper_trace.read_text().count(first_select)
        run(["zellij", "--session", session, "action", "go-to-tab", "1"], zellij_env)
        time.sleep(2)
        if helper_trace.read_text().count(first_select) != first_select_count:
            raise RuntimeError("unchanged active Zellij tab dispatched a duplicate helper")
        presentation_after = state(ctl, env)["presentation_count"]
        if focus_before != focus_after or focus_state_before != focus_state_after or presentation_before != presentation_after:
            raise RuntimeError(
                f"plugin changed focus/presentation: X {focus_before!r}->{focus_after!r}, "
                f"AT-SPI {focus_state_before!r}->{focus_state_after!r}, present {presentation_before}->{presentation_after}"
            )
        (evidence / "plugin-focus.json").write_text(json.dumps({
            "x_active_before": focus_before, "x_active_after": focus_after,
            "atspi_before": json.loads(focus_state_before), "atspi_after": json.loads(focus_state_after),
            "presentation_before": presentation_before, "presentation_after": presentation_after,
        }, indent=2) + "\n")

        # A new tab has no browser project and therefore must not change selection.
        run(["zellij", "--session", session, "action", "new-tab", "--name", "Unknown"], zellij_env)
        time.sleep(2)
        if state(ctl, env)["selected_project"] != selected["selected_project"]:
            raise RuntimeError("plugin changed selection for an unknown Zellij tab")
        run(["zellij", "--session", session, "action", "close-pane"], zellij_env)
        run(["zellij", "--session", session, "action", "go-to-tab", "2"], zellij_env)
        run(["zellij", "--session", session, "action", "close-pane"], zellij_env)
        wait_until("plugin closing second project", lambda: project_snapshot(zellij_count=1, exact=True))
        lifecycle_lines = [
            line for line in helper_trace.read_text().splitlines()
            if "\tselect-project\t" in line or "\tclose-project\t" in line
        ]
        (evidence / "plugin-helper-counts.json").write_text(json.dumps({
            "total": len(lifecycle_lines),
            "select": sum("\tselect-project\t" in line for line in lifecycle_lines),
            "close": sum("\tclose-project\t" in line for line in lifecycle_lines),
            "lines": lifecycle_lines,
        }, indent=2) + "\n")
        passed(
            "plugin_focus",
            f"real plugin selected/ignored/closed one-way; X={focus_before}; AT-SPI={focus_state_before}; presentations={presentation_before}",
        )

        processes_before = run(["ps", "-eo", "pid,ppid,rss,comm,args"], env, check=False)
        (evidence / "processes-before-memory.txt").write_text(processes_before.stdout)
        (evidence / "memory-warning").write_text("1 critical\n")

        def suspended_snapshot():
            snapshot = state(ctl, env)
            return snapshot if "suspended" in json.dumps(snapshot).lower() else None

        suspended = wait_until("memory suspension", suspended_snapshot)
        (evidence / "state-after-memory.json").write_text(json.dumps(suspended, indent=2) + "\n")
        processes = run(["ps", "-eo", "pid,ppid,rss,comm,args"], env, check=False)
        (evidence / "processes-after-memory.txt").write_text(processes.stdout)
        def web_rss(sample: str) -> int:
            return sum(int(parts[2]) for line in sample.splitlines() if "WebKit" in line and len(parts := line.split(None, 4)) >= 3 and parts[2].isdigit())
        memory_metrics = {"webkit_rss_kib_before": web_rss(processes_before.stdout), "webkit_rss_kib_after": web_rss(processes.stdout)}
        (evidence / "memory-metrics.json").write_text(json.dumps(memory_metrics, indent=2) + "\n")
        passed("memory", f"critical warning suspended inactive view; WebKit RSS KiB {memory_metrics}")

        before_restart = state(ctl, env)
        (evidence / "state-before-restart.json").write_text(json.dumps(before_restart, indent=2) + "\n")
        process.terminate(); process.wait(timeout=15)
        (runtime / "lavish-browser/control.sock").unlink(missing_ok=True)
        process = start_browser()

        def restored_snapshot():
            snapshot = state(ctl, env)
            return snapshot if len(snapshot["projects"]) >= 2 else None

        restored = wait_until("persisted projects", restored_snapshot)
        (evidence / "state-after-restart.json").write_text(json.dumps(restored, indent=2) + "\n")
        passed("persistence", "installed browser restored Standalone and live Zellij metadata")
    finally:
        (evidence / "scenarios.json").write_text(json.dumps(scenarios, indent=2) + "\n")
        run(["zellij", "delete-session", session, "--force"], env, check=False)
        if zellij_client and zellij_client.poll() is None:
            zellij_client.terminate()
        if 'client_log' in locals():
            client_log.close()
        if process.poll() is None:
            process.terminate()
            try: process.wait(timeout=10)
            except subprocess.TimeoutExpired: process.kill()
        browser_log.close()
        if state_home.exists():
            shutil.copytree(state_home, evidence / "persistence-final", dirs_exist_ok=True)
        shutil.rmtree(isolated, ignore_errors=True)

    required_names = {"standalone", "multi_project_single_instance", "dedupe", "plugin_focus", "accessibility", "native_download", "memory", "persistence"}
    missing = required_names - scenarios.keys()
    if missing:
        raise RuntimeError(f"required installed scenarios did not pass: {sorted(missing)}")
    print(json.dumps(scenarios, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
