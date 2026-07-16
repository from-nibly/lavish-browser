#!/usr/bin/env python3
"""Installed Lavish Browser integration smoke using only live system boundaries."""
from __future__ import annotations

import argparse
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
    run(["zellij", "--session", session, "action", "write-chars", command], env)
    run(["zellij", "--session", session, "action", "write", "13"], env)


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
    data = isolated / "data"
    for directory in (runtime, state_home, config, data):
        directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    env.update({
        "XDG_RUNTIME_DIR": str(runtime), "XDG_STATE_HOME": str(state_home),
        "XDG_CONFIG_HOME": str(config), "XDG_DATA_HOME": str(data),
        "LAVISH_BROWSER_EXECUTABLE": str(browser),
        "LAVISH_BROWSER_MEMORY_WARNING_FILE": str(evidence / "memory-warning"),
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
        standalone = run([str(launcher), str(artifact_a)], env, timeout=180)
        (evidence / "standalone-launch.log").write_text(standalone.stdout + standalone.stderr)
        snapshot = wait_until("Standalone project", lambda: project_snapshot(standalone=True))
        passed("standalone", "installed launcher created visible Standalone browser state")

        zellij_env = env.copy()
        zellij_env["ZELLIJ_CONFIG_DIR"] = str(config / "zellij")
        (config / "zellij").mkdir(parents=True, exist_ok=True)
        (config / "zellij/config.kdl").write_text(
            'simplified_ui true\nshow_startup_tips false\npane_frames false\n'
        )
        client_log = (evidence / "zellij-client.log").open("w")
        zellij_client = subprocess.Popen(
            ["script", "-qefc", f"zellij --session {session} --new-session-with-layout compact", str(evidence / "zellij.typescript")],
            env=zellij_env, stdin=subprocess.PIPE, stdout=client_log, stderr=subprocess.STDOUT,
        )
        wait_until("isolated Zellij session", lambda: session in run(["zellij", "list-sessions", "--short", "--no-formatting"], zellij_env).stdout)

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

        # Exercise the installed real plugin. Permission approval is sent to the
        # isolated attached client; failure to observe lifecycle state is fatal.
        configuration = f"helper_path={ctl},debug=true"
        run(["zellij", "--session", session, "action", "start-or-reload-plugin", "--configuration", configuration, f"file:{plugin}"], zellij_env)
        time.sleep(1)
        if not zellij_client.stdin:
            raise RuntimeError("isolated Zellij client has no permission-input stream")
        zellij_client.stdin.write(b"y\r")
        zellij_client.stdin.flush()
        time.sleep(2)
        zellij_projects = [p for p in state(ctl, env)["projects"] if session in json.dumps(p["key"])]
        tab_ids = sorted(p["key"]["stable_tab_id"] for p in zellij_projects)
        if len(tab_ids) < 2:
            raise RuntimeError("real Zellij projects were not available for plugin assertions")
        focus_before = active_window(env)
        run(["zellij", "--session", session, "action", "go-to-tab", "1"], zellij_env)
        selected = wait_until(
            "plugin selecting first project",
            lambda: selected_snapshot({"kind": "zellij", "session_name": session, "stable_tab_id": tab_ids[0]}),
        )
        focus_after = active_window(env)
        if focus_before != focus_after:
            raise RuntimeError(f"plugin selection changed active window: {focus_before!r} -> {focus_after!r}")

        # A new tab has no browser project and therefore must not change selection.
        run(["zellij", "--session", session, "action", "new-tab", "--name", "Unknown"], zellij_env)
        time.sleep(2)
        if state(ctl, env)["selected_project"] != selected["selected_project"]:
            raise RuntimeError("plugin changed selection for an unknown Zellij tab")
        run(["zellij", "--session", session, "action", "close-pane"], zellij_env)
        run(["zellij", "--session", session, "action", "go-to-tab", "2"], zellij_env)
        run(["zellij", "--session", session, "action", "close-pane"], zellij_env)
        wait_until("plugin closing second project", lambda: project_snapshot(zellij_count=1, exact=True))
        passed("plugin_focus", f"real plugin selected/ignored/closed one-way; active window unchanged: {focus_before}")

        # AT-SPI is a required independent visible-native assertion.
        dogtail = run([sys.executable, str(root / "tests/installed/verify_accessibility.py"), artifact_a.name, artifact_b.name], env, check=False)
        (evidence / "accessibility.log").write_text(dogtail.stdout + dogtail.stderr)
        if dogtail.returncode:
            raise RuntimeError("AT-SPI/Dogtail native assertion failed; see accessibility.log")
        passed("accessibility", dogtail.stdout.strip())

        (evidence / "memory-warning").write_text("1 critical\n")
        suspended = wait_until("memory suspension", lambda: (s := state(ctl, env)) if "suspended" in json.dumps(s).lower() else None)
        (evidence / "state-after-memory.json").write_text(json.dumps(suspended, indent=2) + "\n")
        processes = run(["ps", "-eo", "pid,ppid,rss,comm,args"], env, check=False)
        (evidence / "processes-after-memory.txt").write_text(processes.stdout)
        passed("memory", "critical warning suspended at least one inactive installed WebView")

        before_restart = state(ctl, env)
        (evidence / "state-before-restart.json").write_text(json.dumps(before_restart, indent=2) + "\n")
        process.terminate(); process.wait(timeout=15)
        (runtime / "lavish-browser/control.sock").unlink(missing_ok=True)
        process = start_browser()
        restored = wait_until("persisted projects", lambda: (s := state(ctl, env)) if len(s["projects"]) >= 2 else None)
        (evidence / "state-after-restart.json").write_text(json.dumps(restored, indent=2) + "\n")
        passed("persistence", "installed browser restored Standalone and live Zellij metadata")
    finally:
        (evidence / "scenarios.json").write_text(json.dumps(scenarios, indent=2) + "\n")
        run(["zellij", "delete-session", session, "--force"], env, check=False)
        if zellij_client and zellij_client.poll() is None:
            zellij_client.terminate()
        if process.poll() is None:
            process.terminate()
            try: process.wait(timeout=10)
            except subprocess.TimeoutExpired: process.kill()
        browser_log.close()
        if state_home.exists():
            shutil.copytree(state_home, evidence / "persistence-final", dirs_exist_ok=True)
        shutil.rmtree(isolated, ignore_errors=True)

    required_names = {"standalone", "multi_project_single_instance", "dedupe", "plugin_focus", "accessibility", "memory", "persistence"}
    missing = required_names - scenarios.keys()
    if missing:
        raise RuntimeError(f"required installed scenarios did not pass: {sorted(missing)}")
    print(json.dumps(scenarios, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
