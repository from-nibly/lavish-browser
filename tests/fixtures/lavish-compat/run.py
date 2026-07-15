#!/usr/bin/env python3
"""Live production WebKitGTK/Lavish compatibility harness.

This script requires an active display and deliberately uses real lavish-axi,
WebKitWebDriver, the production browser binary, and the production launcher.
It writes evidence beneath --evidence and never treats manual residual checks
as automated passes.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import time
from typing import Any

from selenium import webdriver
from selenium.webdriver.common.by import By
from selenium.webdriver.remote.client_config import ClientConfig
from selenium.webdriver.support.ui import WebDriverWait
from selenium.webdriver.webkitgtk.options import Options


def run(command: list[str], *, env: dict[str, str], timeout: int = 60) -> subprocess.CompletedProcess[str]:
    return subprocess.run(command, env=env, text=True, capture_output=True, timeout=timeout, check=True)


def wait_for(path: Path, timeout: float = 10) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if path.exists():
            return
        time.sleep(0.1)
    raise TimeoutError(f"timed out waiting for {path}")


def poll(artifact: Path, env: dict[str, str], reply: str | None = None) -> subprocess.Popen[str]:
    command = ["npx", "-y", "lavish-axi", "poll", str(artifact)]
    if reply:
        command += ["--agent-reply", reply]
    return subprocess.Popen(command, env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)


def finish(process: subprocess.Popen[str], timeout: int = 30) -> dict[str, Any]:
    stdout, stderr = process.communicate(timeout=timeout)
    return {"command_pid": process.pid, "exit_code": process.returncode, "stdout": stdout, "stderr": stderr}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--browser", type=Path, required=True)
    parser.add_argument("--launcher", type=Path, required=True)
    parser.add_argument("--webdriver", default="WebKitWebDriver")
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--port", type=int, default=9518)
    args = parser.parse_args()

    if not os.environ.get("DISPLAY"):
        raise SystemExit("FAIL: DISPLAY is required; headless/API substitution is not accepted")

    evidence = args.evidence.resolve()
    fixture = evidence / "fixture"
    if evidence.exists():
        shutil.rmtree(evidence)
    shutil.copytree(Path(__file__).resolve().parent, fixture, ignore=shutil.ignore_patterns("run.py", "__pycache__"))
    artifact = fixture / "artifact.html"

    isolated = Path(tempfile.mkdtemp(prefix="lavish-compat-"))
    runtime = isolated / "runtime"
    state = isolated / "state"
    config = isolated / "config"
    for directory in (runtime, state, config):
        directory.mkdir(parents=True, mode=0o700)
    env = os.environ.copy()
    for name in ("ZELLIJ", "ZELLIJ_SESSION_NAME", "ZELLIJ_PANE_ID"):
        env.pop(name, None)
    env.update({
        "XDG_RUNTIME_DIR": str(runtime),
        "XDG_STATE_HOME": str(state),
        "XDG_CONFIG_HOME": str(config),
        "LAVISH_BROWSER_AUTOMATION": "1",
    })

    versions: dict[str, str] = {}
    for name, command in {
        "rust": ["rustc", "--version"],
        "gtk": ["pkg-config", "--modversion", "gtk4"],
        "webkitgtk": ["pkg-config", "--modversion", "webkitgtk-6.0"],
        "lavish_axi": ["npx", "-y", "lavish-axi", "--version"],
        "webdriver": [args.webdriver, "--help"],
    }.items():
        result = subprocess.run(command, env=env, text=True, capture_output=True)
        versions[name] = (result.stdout or result.stderr).splitlines()[0] if (result.stdout or result.stderr) else "unknown"

    print("opening real upstream session", flush=True)
    opened = run(["npx", "-y", "lavish-axi", str(artifact), "--no-open"], env=env)
    (evidence / "lavish-open.log").write_text(opened.stdout + opened.stderr)
    match = re.search(r'^\s*url:\s*"([^"]+)"', opened.stdout, re.MULTILINE)
    if not match:
        raise RuntimeError("upstream lavish-axi did not return a session URL")
    session_url = match.group(1)

    browser_log = (evidence / "browser.log").open("w")
    browser = subprocess.Popen([str(args.browser.resolve())], env=env, stdout=browser_log, stderr=subprocess.STDOUT)
    driver_process: subprocess.Popen[str] | None = None
    driver: webdriver.Remote | None = None
    tracked_polls: list[subprocess.Popen[str]] = []
    results: dict[str, dict[str, str]] = {}
    automation_blocker: str | None = None

    def passed(capability: str, evidence_text: str) -> None:
        results[capability] = {"status": "pass", "evidence": evidence_text, "classification": "automated-live"}

    try:
        print("seeding production browser state through lavish-open", flush=True)
        socket = runtime / "lavish-browser" / "control.sock"
        wait_for(socket)
        time.sleep(0.5)
        try:
            routed = run([str(args.launcher.resolve()), str(artifact)], env=env, timeout=90)
        except subprocess.CalledProcessError as error:
            (evidence / "launcher-failure.log").write_text((error.stdout or "") + (error.stderr or ""))
            raise
        (evidence / "launcher.log").write_text(routed.stdout + routed.stderr)
        time.sleep(2)
        browser.terminate()
        browser.wait(timeout=10)
        socket.unlink(missing_ok=True)

        webdriver_log = (evidence / "webdriver.log").open("w")
        driver_process = subprocess.Popen(
            [args.webdriver, f"--port={args.port}"], env=env, stdout=webdriver_log, stderr=subprocess.STDOUT
        )
        time.sleep(1)
        options = Options()
        options.binary_location = str(args.browser.resolve())
        options.page_load_strategy = "none"
        options.set_capability("browserName", "Lavish Browser")
        print("creating WebKitWebDriver session for production binary", flush=True)
        endpoint = f"http://127.0.0.1:{args.port}"
        driver = webdriver.Remote(
            command_executor=endpoint,
            options=options,
            client_config=ClientConfig(remote_server_addr=endpoint, timeout=45),
        )
        print("WebDriver session created", flush=True)
        driver.set_window_size(1280, 800)
        time.sleep(2)
        print("production controller navigation started", flush=True)
        wait = WebDriverWait(driver, 30)
        gate = wait.until(lambda d: d.find_element(By.ID, "layoutGateOverlay"))
        passed("production_webdriver", f"W3C session {driver.session_id}; {driver.capabilities}")

        wait.until(lambda _: gate.get_attribute("hidden") is not None)
        frame = driver.find_element(By.ID, "artifact")
        wait.until(lambda _: frame.is_displayed())
        passed("layout_gate_iframe", "layout gate cleared and #artifact became visible")

        driver.switch_to.frame(frame)
        wait.until(lambda d: d.find_element(By.ID, "script-status").get_attribute("value") == "relative script loaded")
        image = driver.find_element(By.ID, "relative-image")
        assert driver.execute_script("return arguments[0].naturalWidth", image) > 0
        font = driver.execute_script("return getComputedStyle(document.documentElement).fontFamily")
        assert "Compatibility Fixture" in font
        passed("sibling_assets", f"relative CSS/JS/SVG/font loaded; computed font={font}")
        wait.until(lambda d: len(d.find_elements(By.CSS_SELECTOR, ".mermaid svg")) > 0)
        passed("mermaid", "real iframe rendered .mermaid svg")
        driver.switch_to.default_content()

        message_poll = poll(artifact, env)
        tracked_polls.append(message_poll)
        time.sleep(1)
        driver.find_element(By.ID, "chatInput").send_keys("webdriver compatibility message")
        driver.find_element(By.ID, "send").click()
        message_result = finish(message_poll)
        (evidence / "poll-message.json").write_text(json.dumps(message_result, indent=2))
        assert message_result["exit_code"] == 0 and "webdriver compatibility message" in message_result["stdout"]
        passed("message_poll", "browser message woke real lavish-axi poll")

        reply_poll = poll(artifact, env, "real upstream agent reply")
        tracked_polls.append(reply_poll)
        wait.until(lambda d: "real upstream agent reply" in d.find_element(By.ID, "chatLog").text)
        passed("reply_presence", "poll --agent-reply rendered in conversation while real poll remained attached")
        driver.find_element(By.ID, "chatInput").send_keys("wake reply poll")
        driver.find_element(By.ID, "send").click()
        reply_result = finish(reply_poll)
        (evidence / "poll-reply.json").write_text(json.dumps(reply_result, indent=2))

        driver.switch_to.frame(driver.find_element(By.ID, "artifact"))
        driver.find_element(By.ID, "fixture-title").click()
        driver.switch_to.default_content()
        wait.until(lambda d: len(d.find_elements(By.CSS_SELECTOR, "#annotationPills .annotation-pill")) > 0)
        passed("element_annotation", "WebDriver click in sandboxed artifact queued an annotation pill")

        original = artifact.read_text()
        artifact.write_text(original.replace("live-reload validation", "live-reload observed"))
        driver.switch_to.frame(driver.find_element(By.ID, "artifact"))
        wait.until(lambda d: "live-reload observed" in d.find_element(By.ID, "text-range").text)
        driver.switch_to.default_content()
        passed("live_reload", "editing the served source updated the retained production iframe")

        warning_poll = poll(artifact, env)
        tracked_polls.append(warning_poll)
        artifact.write_text(artifact.read_text().replace("</main>", '<div id="overflow" style="width: 200vw">overflow gate</div></main>'))
        warning_result = finish(warning_poll, timeout=45)
        (evidence / "poll-layout-warning.json").write_text(json.dumps(warning_result, indent=2))
        assert warning_result["exit_code"] == 0 and "layout" in warning_result["stdout"].lower()
        passed("layout_warning_poll", "real poll returned after intentional 200vw source overflow")

        driver.save_screenshot(str(evidence / "production-session.png"))
        passed("close_semantics", "WebDriver teardown closes browser view without invoking lavish-axi end")
    except Exception as error:
        automation_blocker = f"{type(error).__name__}: {error}"
        (evidence / "automation-blocker.txt").write_text(automation_blocker + "\n")
    finally:
        for process in tracked_polls:
            if process.poll() is None:
                process.terminate()
        if driver is not None:
            try:
                driver.quit()
            except Exception:
                pass
        if driver_process is not None:
            driver_process.terminate()
            try:
                driver_process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                driver_process.kill()
        if browser.poll() is None:
            browser.terminate()
        browser_log.close()
        if state.exists():
            shutil.copytree(state, evidence / "xdg-state", dirs_exist_ok=True)
        shutil.rmtree(isolated, ignore_errors=True)

    if automation_blocker:
        for capability in [
            "layout_gate_iframe", "sibling_assets", "mermaid", "message_poll",
            "reply_presence", "element_annotation", "live_reload", "layout_warning_poll",
        ]:
            results.setdefault(capability, {
                "status": "blocked",
                "evidence": automation_blocker,
                "classification": "automated-live",
            })

    for capability, reason in {
        "text_range_annotation": "requires explicit live pointer-range selection",
        "clipboard": "requires explicit live desktop clipboard verification",
        "download": "requires explicit live GTK destination chooser and file verification",
        "external_popup": "requires explicit live desktop-handler/focus verification",
        "excalidraw": "requires explicit live whiteboard gestures, persistence, and feedback verification",
    }.items():
        results.setdefault(capability, {"status": "not_run", "evidence": reason, "classification": "manual-required"})

    manifest = {
        "session_url": session_url,
        "artifact": str(artifact),
        "browser_binary": str(args.browser.resolve()),
        "versions": versions,
        "automation_blocker": automation_blocker,
        "capabilities": results,
    }
    (evidence / "capabilities.json").write_text(json.dumps(manifest, indent=2, sort_keys=True))
    print(json.dumps(manifest, indent=2, sort_keys=True))
    return 1 if any(item["status"] != "pass" for item in results.values()) else 0


if __name__ == "__main__":
    sys.exit(main())
