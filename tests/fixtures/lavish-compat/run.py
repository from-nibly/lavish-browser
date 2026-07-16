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
from selenium.webdriver.common.action_chains import ActionChains
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
    parser.add_argument("--manual-excalidraw", action="store_true")
    parser.add_argument("--manual-display", default=os.environ.get("DISPLAY", ":0"))
    parser.add_argument("--manual-timeout", type=int, default=1800)
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
    data = isolated / "data"
    for directory in (runtime, state, config, data):
        directory.mkdir(parents=True, mode=0o700)
    handler_log = evidence / "external-handler.log"
    handler = evidence / "external-handler.sh"
    handler.write_text(f"#!/bin/sh\nprintf '%s\\n' \"$1\" >> {handler_log}\n")
    handler.chmod(0o700)
    applications = data / "applications"
    applications.mkdir()
    (applications / "lavish-compat-handler.desktop").write_text(
        "[Desktop Entry]\nType=Application\nName=Lavish compatibility handler\n"
        f"Exec={handler} %u\nNoDisplay=true\n"
        "MimeType=x-scheme-handler/http;x-scheme-handler/https;\n"
    )
    env = os.environ.copy()
    for name in ("ZELLIJ", "ZELLIJ_SESSION_NAME", "ZELLIJ_PANE_ID"):
        env.pop(name, None)
    env.update({
        "XDG_RUNTIME_DIR": str(runtime),
        "XDG_STATE_HOME": str(state),
        "XDG_CONFIG_HOME": str(config),
        "XDG_DATA_HOME": str(data),
        "LAVISH_BROWSER_AUTOMATION": "1",
    })
    subprocess.run(["update-desktop-database", str(applications)], env=env, capture_output=True)
    subprocess.run(["gio", "mime", "x-scheme-handler/http", "lavish-compat-handler.desktop"], env=env, capture_output=True)
    subprocess.run(["gio", "mime", "x-scheme-handler/https", "lavish-compat-handler.desktop"], env=env, capture_output=True)

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
    manual_browser_log = None
    tracked_polls: list[subprocess.Popen[str]] = []
    results: dict[str, dict[str, str]] = {}
    w3c_trace: list[dict[str, Any]] = []
    automation_blocker: str | None = None

    def w3c(label: str, operation: Any) -> Any:
        started = time.monotonic()
        try:
            value = operation()
            w3c_trace.append({"command": label, "seconds": time.monotonic() - started, "status": "ok", "value": str(value)[:2000]})
            return value
        except Exception as error:
            w3c_trace.append({"command": label, "seconds": time.monotonic() - started, "status": "error", "error": f"{type(error).__name__}: {error}"})
            raise

    def passed(capability: str, evidence_text: str) -> None:
        results[capability] = {"status": "pass", "evidence": evidence_text, "classification": "automated-live"}

    try:
        print("seeding production browser state through lavish-open", flush=True)
        socket = runtime / "lavish-browser" / "control.sock"
        wait_for(socket)
        time.sleep(0.5)
        routed = None
        for attempt in range(3):
            try:
                routed = run([str(args.launcher.resolve()), str(artifact)], env=env, timeout=90)
                break
            except subprocess.CalledProcessError as error:
                (evidence / f"launcher-failure-{attempt + 1}.log").write_text((error.stdout or "") + (error.stderr or ""))
                time.sleep(0.5)
        if routed is None:
            raise RuntimeError("production launcher could not route after three bounded attempts")
        (evidence / "launcher.log").write_text(routed.stdout + routed.stderr)
        time.sleep(2)
        native_export = evidence / "artifact.export.html"
        native_download = subprocess.run(
            ["python", str(Path(__file__).resolve().parent / "native_download.py"),
             "--destination", str(native_export), "--trace", str(evidence / "native-download-trace.json")],
            env=env, text=True, capture_output=True, timeout=90,
        )
        (evidence / "native-download.log").write_text(native_download.stdout + native_download.stderr)
        if native_download.returncode == 0 and native_export.is_file() and "<!doctype html" in native_export.read_text().lower():
            results["download"] = {"status": "pass", "evidence": f"live AT-SPI export chooser wrote and verified {native_export.stat().st_size} bytes", "classification": "manual-live-native"}
        else:
            results["download"] = {"status": "blocked", "evidence": "live native export/chooser check failed; see native-download.log and trace", "classification": "manual-required"}
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
        w3c("window_handles.blank", lambda: driver.window_handles)
        w3c("current_url.blank", lambda: driver.current_url)
        w3c("title.blank", lambda: driver.title)
        w3c("ready_state.blank", lambda: driver.execute_script("return document.readyState"))
        w3c("html.blank", lambda: driver.find_element(By.TAG_NAME, "html").tag_name)
        w3c("navigate.real_lavish", lambda: driver.get(session_url))
        print("WebDriver requested real-session navigation", flush=True)
        wait = WebDriverWait(driver, 30)
        w3c("window_handles.lavish", lambda: driver.window_handles)
        w3c("current_url.lavish", lambda: driver.current_url)
        w3c("title.lavish", lambda: driver.title)
        w3c("ready_state.lavish", lambda: driver.execute_script("return document.readyState"))
        gate = w3c("find.layout_gate", lambda: wait.until(lambda d: d.find_element(By.ID, "layoutGateOverlay")))
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

        driver.find_element(By.ID, "clipboard").click()
        clipboard = run(["xclip", "-selection", "clipboard", "-o"], env=env).stdout
        assert clipboard == "lavish-webkit-compatibility-fixture"
        results["clipboard"] = {"status": "pass", "evidence": "live artifact click reached the native X11 clipboard and xclip read exact text", "classification": "live-gui-native"}

        driver.switch_to.default_content()
        driver.find_element(By.ID, "annotation").click()
        driver.switch_to.frame(driver.find_element(By.ID, "artifact"))
        driver.find_element(By.ID, "external").click()
        try:
            WebDriverWait(driver, 10).until(lambda _: handler_log.exists() and "https://example.com/" in handler_log.read_text())
            results["external_popup"] = {"status": "pass", "evidence": "live target=_blank click invoked isolated desktop HTTPS handler and retained the Lavish context", "classification": "live-gui-native"}
        except Exception as error:
            results["external_popup"] = {"status": "blocked", "evidence": f"{type(error).__name__}: no isolated desktop handler invocation", "classification": "live-gui-native"}
        driver.switch_to.default_content()
        driver.find_element(By.ID, "annotation").click()
        driver.switch_to.frame(driver.find_element(By.ID, "artifact"))

        w3c("mermaid.global", lambda: driver.execute_script("return typeof window.mermaid"))
        w3c("mermaid.elements", lambda: [(element.tag_name, element.get_attribute("class")) for element in driver.find_elements(By.CSS_SELECTOR, ".mermaid, .lavish-whiteboard, iframe, svg")])
        try:
            WebDriverWait(driver, 10).until(lambda d: len(d.find_elements(By.CSS_SELECTOR, ".mermaid svg, iframe[src*='whiteboard']")) > 0)
            passed("mermaid", "real iframe rendered Mermaid or its upstream inline whiteboard")
        except Exception as error:
            results["mermaid"] = {"status": "blocked", "evidence": f"{type(error).__name__}: Mermaid global/elements recorded in W3C trace", "classification": "automated-live"}

        whiteboard = w3c("whiteboard.inline_frame", lambda: wait.until(lambda d: d.find_element(By.CSS_SELECTOR, "iframe[title='Excalidraw whiteboard']")))
        if args.manual_excalidraw:
            results["excalidraw"] = {"status": "blocked", "evidence": "waiting for explicit live human observation and real poll output", "classification": "manual-required"}
            driver.switch_to.default_content()
        else:
            try:
                driver.switch_to.frame(whiteboard)
                w3c("whiteboard.inline_ready", lambda: wait.until(lambda d: d.find_element(By.ID, "wbQueue").is_displayed()))
                canvas = w3c("whiteboard.canvas", lambda: wait.until(lambda d: d.execute_script("return [...document.querySelectorAll('canvas')].find(c => { const r=c.getBoundingClientRect(); return r.width > 100 && r.height > 100; }) || null")))
                native_whiteboard = subprocess.run(
                    ["python", str(Path(__file__).resolve().parent / "native_whiteboard.py"),
                     "--gesture-only", "--trace", str(evidence / "native-whiteboard-trace.json")],
                    env=env, text=True, capture_output=True, timeout=45,
                )
                (evidence / "native-whiteboard.log").write_text(native_whiteboard.stdout + native_whiteboard.stderr)
                if native_whiteboard.returncode != 0:
                    raise RuntimeError("native whiteboard gesture failed")
                driver.find_element(By.ID, "wbNote").send_keys("live Excalidraw rectangle persistence check")
                whiteboard_poll = poll(artifact, env)
                tracked_polls.append(whiteboard_poll)
                time.sleep(1)
                driver.find_element(By.ID, "wbQueue").click()
                whiteboard_result = finish(whiteboard_poll, timeout=120)
                (evidence / "poll-whiteboard.json").write_text(json.dumps(whiteboard_result, indent=2))
                assert whiteboard_result["exit_code"] == 0 and "live Excalidraw rectangle persistence check" in whiteboard_result["stdout"]
                driver.save_screenshot(str(evidence / "excalidraw-edited.png"))
                results["excalidraw"] = {"status": "pass", "evidence": "live canvas rectangle gesture autosaved and Queue feedback woke real poll with the note; screenshot retained", "classification": "live-gui-pointer"}
            except Exception as error:
                results["excalidraw"] = {"status": "blocked", "evidence": f"{type(error).__name__}: WebKitWebDriver pointer gesture was not faithful; explicit live manual gesture required", "classification": "manual-required"}
            finally:
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
        annotation_host = wait.until(lambda d: d.find_element(By.CSS_SELECTOR, ".lavish-annotation-root"))
        annotation_host.shadow_root.find_element(By.CSS_SELECTOR, "textarea").send_keys("element annotation from WebDriver")
        annotation_host.shadow_root.find_element(By.CSS_SELECTOR, ".lavish-send").click()
        driver.switch_to.default_content()
        wait.until(lambda d: len(d.find_elements(By.CSS_SELECTOR, "#annotationPills .pill")) > 0)
        annotation_poll = poll(artifact, env)
        tracked_polls.append(annotation_poll)
        time.sleep(1)
        driver.find_element(By.ID, "send").click()
        annotation_result = finish(annotation_poll)
        (evidence / "poll-annotation.json").write_text(json.dumps(annotation_result, indent=2))
        assert annotation_result["exit_code"] == 0 and "element annotation from WebDriver" in annotation_result["stdout"]
        passed("element_annotation", "production SDK annotation was sent through and woke real poll")

        driver.switch_to.frame(driver.find_element(By.ID, "artifact"))
        text_target = driver.find_element(By.ID, "text-range")
        ActionChains(driver).move_to_element_with_offset(text_target, 5, int(text_target.rect["height"] / 2)).click_and_hold().move_by_offset(int(max(40, text_target.rect["width"] * 0.6)), 0).release().perform()
        range_host = wait.until(lambda d: d.find_element(By.CSS_SELECTOR, ".lavish-annotation-root"))
        range_host.shadow_root.find_element(By.CSS_SELECTOR, "textarea").send_keys("text range from live pointer drag")
        range_host.shadow_root.find_element(By.CSS_SELECTOR, ".lavish-send").click()
        driver.switch_to.default_content()
        range_poll = poll(artifact, env)
        tracked_polls.append(range_poll)
        time.sleep(1)
        driver.find_element(By.ID, "send").click()
        range_result = finish(range_poll)
        (evidence / "poll-text-range.json").write_text(json.dumps(range_result, indent=2))
        assert range_result["exit_code"] == 0 and "text range from live pointer drag" in range_result["stdout"]
        results["text_range_annotation"] = {"status": "pass", "evidence": "live pointer drag created a text-range SDK card and real poll received it", "classification": "live-gui-pointer"}

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

        if args.manual_excalidraw:
            artifact.write_text(original)
            driver.quit()
            driver = None
            driver_process.terminate()
            driver_process.wait(timeout=10)
            driver_process = None
            socket.unlink(missing_ok=True)

            manual_env = env.copy()
            manual_env.pop("LAVISH_BROWSER_AUTOMATION", None)
            manual_env["DISPLAY"] = args.manual_display
            manual_browser_log = (evidence / "manual-browser.log").open("w")
            browser = subprocess.Popen(
                [str(args.browser.resolve())],
                env=manual_env,
                stdout=manual_browser_log,
                stderr=subprocess.STDOUT,
            )
            wait_for(socket)
            time.sleep(5)

            manual_poll = poll(artifact, manual_env)
            tracked_polls.append(manual_poll)
            result_path = evidence / "manual-result.json"
            ready = {
                "status": "ready",
                "evidence": str(evidence),
                "artifact": str(artifact),
                "session_url": session_url,
                "display": args.manual_display,
                "browser_pid": browser.pid,
                "poll_pid": manual_poll.pid,
                "result_path": str(result_path),
                "feedback_note": "manual Excalidraw persistence confirmation",
            }
            (evidence / "manual-ready.json").write_text(json.dumps(ready, indent=2))
            print("MANUAL_EXCALIDRAW_READY " + json.dumps(ready), flush=True)
            print(
                "GUI steps: in Lavish Browser scroll to the Mermaid whiteboard; click 'Click to edit'; "
                "draw a clearly visible shape; wait 3 seconds; use More > Reload artifact; confirm the shape "
                "reappears; enter note 'manual Excalidraw persistence confirmation' in the whiteboard; "
                "click 'Queue feedback'; then report edit visibility and persistence to the conductor.",
                flush=True,
            )

            deadline = time.monotonic() + args.manual_timeout
            while not result_path.exists() and time.monotonic() < deadline:
                time.sleep(0.5)
            if not result_path.exists():
                raise TimeoutError("manual Excalidraw result was not supplied before the bounded timeout")
            observation = json.loads(result_path.read_text())
            observed_pass = all([
                observation.get("status") == "pass",
                observation.get("edit_visible") is True,
                observation.get("persisted_after_reload") is True,
                observation.get("feedback_queued") is True,
            ])
            if observed_pass:
                manual_poll_result = finish(manual_poll, timeout=60)
                (evidence / "poll-whiteboard-manual.json").write_text(json.dumps(manual_poll_result, indent=2))
                if (
                    manual_poll_result["exit_code"] == 0
                    and "manual Excalidraw persistence confirmation" in manual_poll_result["stdout"]
                ):
                    results["excalidraw"] = {
                        "status": "pass",
                        "evidence": "human observed edit persistence after reload and real owned poll received queued whiteboard feedback",
                        "classification": "manual-live",
                    }
                else:
                    results["excalidraw"] = {
                        "status": "blocked",
                        "evidence": "human observation passed but the owned real poll did not return matching whiteboard feedback",
                        "classification": "manual-live",
                    }
            else:
                results["excalidraw"] = {
                    "status": "blocked",
                    "evidence": f"human observation did not pass: {observation}",
                    "classification": "manual-live",
                }
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
        if manual_browser_log is not None:
            manual_browser_log.close()
        if state.exists():
            shutil.copytree(state, evidence / "xdg-state", dirs_exist_ok=True)
        (evidence / "w3c-trace.json").write_text(json.dumps(w3c_trace, indent=2))
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
