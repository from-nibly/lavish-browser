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
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request

from selenium import webdriver
from selenium.webdriver.common.by import By
from selenium.webdriver.remote.client_config import ClientConfig
from selenium.webdriver.support.ui import WebDriverWait
from selenium.webdriver.webkitgtk.options import Options


def run(argv: list[str], env: dict[str, str], *, check: bool = True, timeout: int = 120) -> subprocess.CompletedProcess[str]:
    return subprocess.run(argv, env=env, text=True, capture_output=True, check=check, timeout=timeout)


def free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


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


def browser_request(runtime: Path, command: dict) -> dict:
    request = {
        "protocol_version": 1,
        "request_id": f"e2e-{time.monotonic_ns()}",
        "command": command,
    }
    with socket.socket(socket.AF_UNIX) as client:
        client.settimeout(10)
        client.connect(str(runtime / "lavish-browser/control.sock"))
        client.sendall(json.dumps(request).encode() + b"\n")
        response = b""
        while not response.endswith(b"\n"):
            chunk = client.recv(65536)
            if not chunk:
                break
            response += chunk
    envelope = json.loads(response)
    if envelope.get("status") not in ("ok", "ignored"):
        raise RuntimeError(f"browser request failed: {envelope}")
    return envelope


def native(root: Path, env: dict[str, str], command: str, needle: str = "", *, timeout: int = 30, output: Path | None = None):
    argv = [sys.executable, str(root / "tests/e2e/native_accessibility.py"), command]
    if needle:
        argv.append(needle)
    argv += ["--timeout", str(timeout)]
    if output:
        argv += ["--output", str(output)]
    return run(argv, env, timeout=timeout + 5)


def terminate_pid(pid: int, timeout: float = 10):
    try:
        os.kill(pid, 15)
    except ProcessLookupError:
        return
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try: os.kill(pid, 0)
        except ProcessLookupError: return
        time.sleep(.1)
    try: os.kill(pid, 9)
    except ProcessLookupError: pass


def webdriver_session(browser: Path, port: int):
    options = Options()
    options.binary_location = str(browser)
    options.page_load_strategy = "none"
    options.set_capability("browserName", "Lavish Browser")
    endpoint = f"http://127.0.0.1:{port}"
    return webdriver.Remote(
        command_executor=endpoint,
        options=options,
        client_config=ClientConfig(remote_server_addr=endpoint, timeout=45),
    )


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
    lavish_port = free_port()
    refresh_port = free_port()
    lavish_state = evidence / "lavish-state"
    lavish_state.mkdir(mode=0o700)
    env.update({
        "XDG_RUNTIME_DIR": str(runtime), "XDG_STATE_HOME": str(state_home),
        "XDG_CONFIG_HOME": str(config), "XDG_CACHE_HOME": str(cache), "XDG_DATA_HOME": str(data),
        "LAVISH_BROWSER_EXECUTABLE": str(browser),
        "LAVISH_BROWSER_AUTOMATION": "1",
        "LAVISH_BROWSER_MEMORY_WARNING_FILE": str(evidence / "memory-warning"),
        "LAVISH_BROWSER_CTL_TRACE": str(evidence / "control-helper.trace"),
        "LAVISH_AXI_STATE_DIR": str(lavish_state),
        "LAVISH_AXI_PORT": str(lavish_port),
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
    webdriver_process = None
    driver = None
    ctl_disabled = None
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

        # Browser restart reconciliation must query the same isolated Zellij
        # server/config as the client, not the ambient user's sessions.
        env["ZELLIJ_CONFIG_DIR"] = str(config / "zellij")
        zellij_env = env.copy()
        (config / "zellij").mkdir(parents=True, exist_ok=True)
        (config / "zellij/config.kdl").write_text(
            'simplified_ui true\nshow_startup_tips false\nshow_release_notes false\npane_frames false\n'
            'load_plugins {\n'
            f'  "file:{plugin}" {{\n'
            f'    helper_path "{ctl}"\n'
            '    debug "true"\n'
            '  }\n'
            '}\n'
        )
        shutil.copy2(config / "zellij/config.kdl", evidence / "zellij-config.kdl")
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
        onboarding = {"show_release_notes": False, "whats_new_observed": False, "dismissed": False}

        def permission_prompt_visible():
            terminal = (evidence / "zellij-client.log").read_bytes()
            if b"What's New" in terminal and not onboarding["dismissed"]:
                onboarding["whats_new_observed"] = True
                if zellij_client.stdin is None:
                    raise RuntimeError("Zellij onboarding blocked permission prompt without writable terminal")
                zellij_client.stdin.write(b"q")
                zellij_client.stdin.flush()
                onboarding["dismissed"] = True
            return b"Allow?" in terminal

        wait_until("visible startup plugin permission prompt", permission_prompt_visible, 60)
        (evidence / "zellij-onboarding.json").write_text(json.dumps(onboarding, indent=2) + "\n")
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

        # Materialize the same canonical source in both real Zellij projects. The
        # launcher still delegates to one unchanged upstream Lavish session.
        shared_log = evidence / "zellij-open-shared.log"
        shared_command = f"{shlex.quote(str(launcher))} {shlex.quote(str(artifact_a))} >{shlex.quote(str(shared_log))} 2>&1"
        terminal_command(session, shared_command, zellij_env)

        def shared_projects():
            snapshot = state(ctl, env)
            matches = [
                project for project in snapshot["projects"]
                if any(document["source_file"] == str(artifact_a.resolve()) for document in project["documents"])
                and project["key"].get("kind") == "zellij"
            ]
            return (snapshot, matches) if len(matches) == 2 else None

        _, matching_projects = wait_until("shared canonical source in two projects", shared_projects, 180)
        shared_keys = [project["key"] for project in matching_projects]
        shared_urls = {
            document["url"]
            for project in matching_projects
            for document in project["documents"]
            if document["source_file"] == str(artifact_a.resolve())
        }
        if len(shared_urls) != 1:
            raise RuntimeError(f"shared source did not use one upstream session: {shared_urls}")

        webdriver_log = (evidence / "integration-webdriver.log").open("w")
        webdriver_process = subprocess.Popen(
            ["WebKitWebDriver", f"--port={args.webdriver_port}"],
            env=env, stdout=webdriver_log, stderr=subprocess.STDOUT,
        )
        time.sleep(1)
        first = shared_keys[0]
        run([str(ctl), "select-project", first["session_name"], str(first["stable_tab_id"])], env)
        time.sleep(1)
        process.terminate(); process.wait(timeout=15)
        (runtime / "lavish-browser/control.sock").unlink(missing_ok=True)
        driver = webdriver_session(browser, args.webdriver_port)
        driver.get(next(iter(shared_urls)))
        wait = WebDriverWait(driver, 45)
        wait.until(lambda d: d.find_element(By.ID, "chatInput").is_displayed())
        shared_poll = subprocess.Popen(
            ["npx", "-y", "lavish-axi", "poll", str(artifact_a.resolve())],
            env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        time.sleep(1)
        shared_message = "installed shared-session duplicate feedback"
        driver.find_element(By.ID, "chatInput").send_keys(shared_message)
        driver.find_element(By.ID, "send").click()
        poll_stdout, poll_stderr = shared_poll.communicate(timeout=120)
        (evidence / "shared-session-poll.txt").write_text(poll_stdout + poll_stderr)
        if shared_poll.returncode or shared_message not in poll_stdout:
            raise RuntimeError("shared-session browser feedback did not wake real upstream poll")
        driver.save_screenshot(str(evidence / "shared-session-first-project.png"))
        automation_pid = state(ctl, env)["process_id"]
        driver.quit(); driver = None
        terminate_pid(automation_pid)
        (runtime / "lavish-browser/control.sock").unlink(missing_ok=True)
        process = start_browser()

        second = shared_keys[1]
        run([str(ctl), "select-project", second["session_name"], str(second["stable_tab_id"])], env)
        time.sleep(1)
        process.terminate(); process.wait(timeout=15)
        (runtime / "lavish-browser/control.sock").unlink(missing_ok=True)
        driver = webdriver_session(browser, args.webdriver_port)
        driver.get(next(iter(shared_urls)))
        wait = WebDriverWait(driver, 45)
        wait.until(lambda d: shared_message in d.find_element(By.ID, "chatLog").text)
        driver.save_screenshot(str(evidence / "shared-session-second-project.png"))
        (evidence / "shared-session.json").write_text(json.dumps({
            "canonical_source": str(artifact_a.resolve()),
            "project_keys": shared_keys,
            "session_url": next(iter(shared_urls)),
            "poll_exit_code": shared_poll.returncode,
            "message_visible_in_second_retained_view": True,
        }, indent=2) + "\n")
        automation_pid = state(ctl, env)["process_id"]
        driver.quit(); driver = None
        terminate_pid(automation_pid)
        (runtime / "lavish-browser/control.sock").unlink(missing_ok=True)
        process = start_browser()
        passed("shared_session_feedback", "one real upstream session in two retained project views; first-view feedback woke real poll and appeared in the second view")

        # Re-materialize both project-scoped views after the WebDriver-owned
        # process cycles; startup restoration is intentionally lazy.
        for key in shared_keys:
            run([str(ctl), "select-project", key["session_name"], str(key["stable_tab_id"])], env)
            wait_until(
                f"materialized shared view {key}",
                lambda key=key: next(
                    (
                        document for project in state(ctl, env)["projects"] if project["key"] == key
                        for document in project["documents"]
                        if document["source_file"] == str(artifact_a.resolve()) and document["lifecycle"] == "ready"
                    ),
                    None,
                ),
                90,
            )

        # Stop the original real upstream server, then globally replace the
        # canonical source with a syntactically valid but unavailable loopback
        # URL. OpenUrl is the public browser-control contract, not a Lavish API.
        stopped = run(["npx", "-y", "lavish-axi", "stop"], env, check=False)
        (evidence / "stale-stop-upstream.log").write_text(stopped.stdout + stopped.stderr)
        stale_url = "http://127.0.0.1:9/session/installed-stale"
        browser_request(runtime, {
            "type": "open_url",
            "project": {"key": first, "label": matching_projects[0]["label"], "raw_tab_name": None},
            "source_file": str(artifact_a.resolve()),
            "url": stale_url,
        })

        def all_matching_lifecycle(expected: str, expected_url: str):
            snapshot = state(ctl, env)
            documents = [
                document for project in snapshot["projects"] if project["key"].get("kind") == "zellij"
                for document in project["documents"] if document["source_file"] == str(artifact_a.resolve())
            ]
            return snapshot if len(documents) == 2 and all(
                document["lifecycle"] == expected and document["url"] == expected_url
                for document in documents
            ) else None

        failed = wait_until("all shared views showing stale failure", lambda: all_matching_lifecycle("failed", stale_url), 90)
        (evidence / "state-stale-failed.json").write_text(json.dumps(failed, indent=2) + "\n")
        native(root, env, "assert", "Failed", output=None)
        native(root, env, "assert", "Reconnect document")
        native(root, env, "snapshot", output=evidence / "atspi-stale-failed.json")

        # Reacquire the same canonical session through the ordinary installed
        # launcher on a different real upstream port. The browser model updates
        # every matching project-scoped document and each materialized view.
        refreshed_log = evidence / "stale-refresh-launcher.log"
        refresh_command = (
            f"LAVISH_AXI_PORT={refresh_port} {shlex.quote(str(launcher))} {shlex.quote(str(artifact_a))} "
            f">{shlex.quote(str(refreshed_log))} 2>&1"
        )
        terminal_command(session, refresh_command, zellij_env)

        def refreshed_snapshot():
            snapshot = state(ctl, env)
            documents = [
                document for project in snapshot["projects"] if project["key"].get("kind") == "zellij"
                for document in project["documents"] if document["source_file"] == str(artifact_a.resolve())
            ]
            return snapshot if len(documents) == 2 and all(
                document["lifecycle"] == "ready" and f":{refresh_port}/session/" in document["url"]
                for document in documents
            ) else None

        refreshed = wait_until("global stale URL recovery", refreshed_snapshot, 180)
        (evidence / "state-stale-recovered.json").write_text(json.dumps(refreshed, indent=2) + "\n")
        native(root, env, "assert", "Ready")
        native(root, env, "absent", "Reconnect document")
        native(root, env, "snapshot", output=evidence / "atspi-stale-recovered.json")
        recovered_urls = {
            document["url"] for project in refreshed["projects"] for document in project["documents"]
            if document["source_file"] == str(artifact_a.resolve())
        }
        if len(recovered_urls) != 1:
            raise RuntimeError(f"global refresh diverged across projects: {recovered_urls}")
        recovered_url = next(iter(recovered_urls))
        with urllib.request.urlopen(recovered_url, timeout=10) as response:
            if response.status != 200:
                raise RuntimeError(f"recovered upstream returned HTTP {response.status}")
        time.sleep(1)
        process.terminate(); process.wait(timeout=15)
        (runtime / "lavish-browser/control.sock").unlink(missing_ok=True)
        driver = webdriver_session(browser, args.webdriver_port)
        driver.get(recovered_url)
        wait = WebDriverWait(driver, 45)
        wait.until(lambda d: f":{refresh_port}/session/" in d.current_url)
        wait.until(lambda d: d.find_element(By.ID, "chatInput").is_displayed())
        driver.save_screenshot(str(evidence / "stale-recovered-webdriver.png"))
        automation_pid = state(ctl, env)["process_id"]
        driver.quit(); driver = None
        terminate_pid(automation_pid)
        (runtime / "lavish-browser/control.sock").unlink(missing_ok=True)
        process = start_browser()
        passed("stale_url_reconnect", "all project-scoped matching views exposed native Failed/reconnect state, then ordinary installed launch on a real owned refresh port globally recovered them; production WebDriver loaded the recovered session")

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

        # Drive native document and project close controls. Closing browser
        # metadata must not call upstream end; ordinary lavish-open (without
        # --reopen) restores each surface while the real server remains alive.
        second_project = next(project for project in state(ctl, env)["projects"] if project["key"] == second)
        identity_text = f"Zellij session {session}, tab {second['stable_tab_id']}"
        document_close_name = f"Close document {artifact_a.name}, {identity_text}"
        native(root, env, "click", document_close_name)

        def document_closed():
            project = next((item for item in state(ctl, env)["projects"] if item["key"] == second), None)
            return project if project and all(document["source_file"] != str(artifact_a.resolve()) for document in project["documents"]) else None

        wait_until("native document close", document_closed)
        native(root, env, "absent", document_close_name)
        with urllib.request.urlopen(recovered_url, timeout=10) as response:
            assert response.status == 200
        reopen_document_log = evidence / "native-reopen-document.log"
        terminal_command(session, (
            f"LAVISH_AXI_PORT={refresh_port} {shlex.quote(str(launcher))} {shlex.quote(str(artifact_a))} "
            f">{shlex.quote(str(reopen_document_log))} 2>&1"
        ), zellij_env)
        wait_until("ordinary reopen after native document close", lambda: shared_projects())
        native(root, env, "assert", document_close_name)

        project_close_name = f"Close project {second_project['label']}, {identity_text}"
        native(root, env, "click", project_close_name)
        wait_until("native project close", lambda: second if all(project["key"] != second for project in state(ctl, env)["projects"]) else None)
        native(root, env, "absent", project_close_name)
        with urllib.request.urlopen(recovered_url, timeout=10) as response:
            assert response.status == 200
        reopen_project_log = evidence / "native-reopen-project.log"
        terminal_command(session, (
            f"LAVISH_AXI_PORT={refresh_port} {shlex.quote(str(launcher))} {shlex.quote(str(artifact_a))} "
            f">{shlex.quote(str(reopen_project_log))} 2>&1"
        ), zellij_env)
        wait_until("ordinary reopen after native project close", lambda: shared_projects())
        native(root, env, "assert", project_close_name)
        native(root, env, "snapshot", output=evidence / "atspi-after-native-reopen.json")
        (evidence / "native-reopen-argv.json").write_text(json.dumps({
            "document": [str(launcher), str(artifact_a)],
            "project": [str(launcher), str(artifact_a)],
            "contains_reopen_flag": False,
        }, indent=2) + "\n")
        passed("native_close_reopen", "AT-SPI document/project close controls removed visible native state; upstream stayed HTTP-live and ordinary installed lavish-open restored both without --reopen")

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

        # Optional integration must degrade quietly. First remove the installed
        # helper and emit changed/unchanged active events. No helper process can
        # run, the browser must not be launched/replaced, and Zellij stays live.
        degradation = {}
        ctl_disabled = ctl.with_name("lavish-browser-ctl.disabled-e2e")
        ctl.rename(ctl_disabled)
        trace_before_missing = helper_trace.read_text().splitlines()
        pid_before_missing = process.pid
        focus_before_missing = active_window(env)
        run(["zellij", "--session", session, "action", "go-to-tab", "2"], zellij_env)
        run(["zellij", "--session", session, "action", "go-to-tab", "2"], zellij_env)
        run(["zellij", "--session", session, "action", "go-to-tab", "1"], zellij_env)
        run(["zellij", "--session", session, "action", "new-tab", "--name", "MissingHelperUnknown"], zellij_env)
        time.sleep(1)
        run(["zellij", "--session", session, "action", "close-pane"], zellij_env)
        time.sleep(3)
        if process.poll() is not None or process.pid != pid_before_missing:
            raise RuntimeError("missing helper disturbed or replaced the browser")
        if helper_trace.read_text().splitlines() != trace_before_missing:
            raise RuntimeError("missing helper unexpectedly wrote lifecycle trace")
        if session not in run(["zellij", "list-sessions", "--short", "--no-formatting"], zellij_env).stdout:
            raise RuntimeError("missing helper destabilized Zellij")
        focus_after_missing = active_window(env)
        if focus_after_missing != focus_before_missing:
            raise RuntimeError("missing helper lifecycle changed desktop focus")
        degradation["missing_helper"] = {
            "browser_pid_unchanged": True,
            "trace_growth": 0,
            "events_exercised": ["active", "unchanged", "unknown", "close"],
            "focus_before": focus_before_missing,
            "focus_after": focus_after_missing,
        }
        ctl_disabled.rename(ctl); ctl_disabled = None

        # Next stop only the browser and exercise active/unchanged events while
        # the restored helper has no socket. The helper never starts the app and
        # event coalescing bounds invocations. Restore the browser, then prove a
        # later changed event recovers normal selection.
        process.terminate(); process.wait(timeout=15)
        (runtime / "lavish-browser/control.sock").unlink(missing_ok=True)
        lifecycle_before_stopped = sum(
            "\tselect-project\t" in line or "\tclose-project\t" in line
            for line in helper_trace.read_text().splitlines()
        )
        focus_before_stopped = active_window(env)
        run(["zellij", "--session", session, "action", "go-to-tab", "2"], zellij_env)
        run(["zellij", "--session", session, "action", "go-to-tab", "2"], zellij_env)
        run(["zellij", "--session", session, "action", "go-to-tab", "1"], zellij_env)
        run(["zellij", "--session", session, "action", "new-tab", "--name", "StoppedBrowserUnknown"], zellij_env)
        time.sleep(1)
        run(["zellij", "--session", session, "action", "close-pane"], zellij_env)
        time.sleep(3)
        if (runtime / "lavish-browser/control.sock").exists() or process.poll() is None:
            raise RuntimeError("plugin helper launched a browser while the browser was stopped")
        stopped_growth = sum(
            "\tselect-project\t" in line or "\tclose-project\t" in line
            for line in helper_trace.read_text().splitlines()
        ) - lifecycle_before_stopped
        # Changed active (2), return active, unknown active, unknown close and
        # post-close active can each emit once; unchanged duplicates must not.
        if stopped_growth > 6:
            raise RuntimeError(f"plugin/helper retry storm while browser absent: {stopped_growth} calls")
        process = start_browser()
        restored_after_dependency = wait_until("browser restore after optional dependency test", lambda: state(ctl, env))
        zellij_projects = [project for project in restored_after_dependency["projects"] if session in json.dumps(project["key"])]
        recovery_key = zellij_projects[0]["key"]
        ordered_ids = sorted(project["key"]["stable_tab_id"] for project in zellij_projects)
        recovery_tab_position = str(ordered_ids.index(recovery_key["stable_tab_id"]) + 1)
        other_tab_position = "2" if recovery_tab_position == "1" else "1"
        run(["zellij", "--session", session, "action", "go-to-tab", other_tab_position], zellij_env)
        run(["zellij", "--session", session, "action", "go-to-tab", recovery_tab_position], zellij_env)
        wait_until("plugin recovery after browser restore", lambda: selected_snapshot(recovery_key))
        focus_after_stopped = active_window(env)
        if focus_after_stopped != focus_before_stopped:
            raise RuntimeError("browser-absent plugin lifecycle changed desktop focus")
        degradation["stopped_browser"] = {
            "helper_invocation_growth": stopped_growth,
            "browser_not_launched_by_helper": True,
            "events_exercised": ["active", "unchanged", "unknown", "close"],
            "focus_before": focus_before_stopped,
            "focus_after": focus_after_stopped,
            "recovered_selected_project": recovery_key,
        }
        (evidence / "plugin-degradation.json").write_text(json.dumps(degradation, indent=2) + "\n")
        passed("plugin_degradation", "real plugin remained quiet with installed helper missing and browser stopped, emitted no retry storm or browser launch/focus change, and recovered after dependencies returned")

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

        # Ensure the surviving project has two materialized real WebViews after
        # the dependency-recovery process restart: open B, then A so B is the
        # deterministic inactive suspension candidate.
        for artifact in (artifact_b, artifact_a):
            materialize_log = evidence / f"memory-materialize-{artifact.name}.log"
            terminal_command(session, (
                f"LAVISH_AXI_PORT={refresh_port} {shlex.quote(str(launcher))} {shlex.quote(str(artifact))} "
                f">{shlex.quote(str(materialize_log))} 2>&1"
            ), zellij_env)
            wait_until(
                f"materialized {artifact.name} before memory warning",
                lambda artifact=artifact: next((
                    document for project in state(ctl, env)["projects"]
                    for document in project["documents"]
                    if document["source_file"] == str(artifact.resolve()) and document["lifecycle"] == "ready"
                ), None),
                180,
            )

        processes_before = run(["ps", "-eo", "pid,ppid,rss,comm,args"], env, check=False)
        (evidence / "processes-before-memory.txt").write_text(processes_before.stdout)
        (evidence / "memory-warning").write_text("1 critical\n")

        def suspended_snapshot():
            snapshot = state(ctl, env)
            return snapshot if "suspended" in json.dumps(snapshot).lower() else None

        suspended = wait_until("memory suspension", suspended_snapshot)
        (evidence / "state-after-memory.json").write_text(json.dumps(suspended, indent=2) + "\n")
        native(root, env, "assert", "Suspended")
        native(root, env, "snapshot", output=evidence / "atspi-suspended.json")
        processes = run(["ps", "-eo", "pid,ppid,rss,comm,args"], env, check=False)
        (evidence / "processes-after-memory.txt").write_text(processes.stdout)
        def web_rss(sample: str) -> int:
            return sum(int(parts[2]) for line in sample.splitlines() if "WebKit" in line and len(parts := line.split(None, 4)) >= 3 and parts[2].isdigit())
        rss_before = web_rss(processes_before.stdout)
        rss_after = web_rss(processes.stdout)
        memory_metrics = {
            "webkit_rss_kib_before": rss_before,
            "webkit_rss_kib_after": rss_after,
            "webkit_rss_kib_delta": rss_after - rss_before,
            "rss_reduced_in_sample": rss_after < rss_before,
            "interpretation": "point-in-time process/RSS sample; suspension is asserted separately and RSS reduction is not required or claimed",
        }
        (evidence / "memory-metrics.json").write_text(json.dumps(memory_metrics, indent=2) + "\n")
        passed("memory", f"critical warning exposed native Suspended state; retained process/RSS samples without claiming reduction: {memory_metrics}")

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
        if "dormant" not in json.dumps(restored).lower():
            raise RuntimeError("persisted restart did not restore any lazy Dormant document")
        native(root, env, "assert", "Dormant")
        native(root, env, "snapshot", output=evidence / "atspi-dormant.json")
        passed("persistence", "installed browser restored Standalone/live Zellij metadata and exposed lazy Dormant rows through AT-SPI")
    finally:
        (evidence / "scenarios.json").write_text(json.dumps(scenarios, indent=2) + "\n")
        if driver is not None:
            try: driver.quit()
            except Exception: pass
        if webdriver_process is not None and webdriver_process.poll() is None:
            webdriver_process.terminate()
            try: webdriver_process.wait(timeout=10)
            except subprocess.TimeoutExpired: webdriver_process.kill()
        if 'webdriver_log' in locals():
            webdriver_log.close()
        if ctl_disabled is not None and ctl_disabled.exists() and not ctl.exists():
            ctl_disabled.rename(ctl)
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
        refresh_env = env.copy(); refresh_env["LAVISH_AXI_PORT"] = str(refresh_port)
        stopped_refresh = run(["npx", "-y", "lavish-axi", "stop"], refresh_env, check=False)
        (evidence / "cleanup-lavish-refresh.log").write_text(stopped_refresh.stdout + stopped_refresh.stderr)
        stopped_initial = run(["npx", "-y", "lavish-axi", "stop"], env, check=False)
        (evidence / "cleanup-lavish-initial.log").write_text(stopped_initial.stdout + stopped_initial.stderr)
        if state_home.exists():
            shutil.copytree(state_home, evidence / "persistence-final", dirs_exist_ok=True)
        shutil.rmtree(isolated, ignore_errors=True)

    required_names = {
        "standalone", "multi_project_single_instance", "dedupe",
        "shared_session_feedback", "stale_url_reconnect", "native_close_reopen",
        "plugin_focus", "plugin_degradation", "accessibility", "native_download",
        "memory", "persistence",
    }
    missing = required_names - scenarios.keys()
    if missing:
        raise RuntimeError(f"required installed scenarios did not pass: {sorted(missing)}")
    print(json.dumps(scenarios, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
