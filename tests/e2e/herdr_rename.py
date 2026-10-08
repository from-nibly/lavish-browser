#!/usr/bin/env python3
"""Run under a private D-Bus/Xvfb session; takes the browser executable as argv[1]."""
import http.server
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import threading
import time


def wait_for(check):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        try:
            result = check()
            if result:
                return result
        except (OSError, KeyError):
            pass
        time.sleep(0.1)
    raise AssertionError("timed out")


loads = []


class Page(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        loads.append(self.path)
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.end_headers()
        self.wfile.write(b"<title>Retained rename page</title><input value='preserve me'>")

    def log_message(self, *_args):
        pass


with tempfile.TemporaryDirectory(prefix="lavish-rename-ui-") as directory:
    root = Path(directory)
    env = os.environ.copy()
    for key in ("XDG_RUNTIME_DIR", "XDG_STATE_HOME", "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME"):
        path = root / key
        path.mkdir(mode=0o700)
        env[key] = str(path)
    env["LAVISH_BROWSER_SOCKET"] = str(root / "browser.sock")
    for key in ("ZELLIJ", "ZELLIJ_SESSION_NAME", "ZELLIJ_PANE_ID"):
        env.pop(key, None)
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Page)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    log = (root / "browser.log").open("w+")
    app = subprocess.Popen([sys.argv[1]], env=env, stdout=log, stderr=log)

    def request(command):
        with socket.socket(socket.AF_UNIX) as client:
            client.settimeout(10)
            client.connect(env["LAVISH_BROWSER_SOCKET"])
            client.sendall((json.dumps({"protocol_version": 2, "request_id": "rename-test", "command": command}) + "\n").encode())
            return json.loads(client.makefile().readline())

    def state():
        return request({"type": "inspect_state"})["state"]

    def key(tab):
        return {"kind": "herdr", "session_name": "rename-test", "workspace_id": "w1", "tab_id": tab}

    def rename(tab, name):
        identity = key(tab)
        identity.pop("kind")
        return request({"type": "rename_herdr_project", **identity, "name": name})

    try:
        wait_for(lambda: request({"type": "ping"}))
        for tab in ("t1", "t2"):
            assert request({"type": "open_url", "project": {"key": key(tab), "label": "Old", "raw_tab_name": "Old"},
                            "source_file": f"/tmp/{tab}.html", "url": f"http://127.0.0.1:{server.server_port}/session/{tab}"})["status"] == "ok"
        wait_for(lambda: all(d["lifecycle"] == "ready" for p in state()["projects"] for d in p["documents"]))
        before = state()
        load_count = len(loads)
        for name in ("Review <日本語>", "", "Duplicate", "Final"):
            assert rename("t1", name)["status"] == "ok"
            after = state()
            expected = json.loads(json.dumps(before))
            expected["projects"][0]["label"] = name or "t1"
            assert after == expected, (after, expected)
            assert rename("t1", name)["status"] == "ignored"
        assert rename("unknown", "Ignored")["status"] == "ignored"
        time.sleep(0.5)
        assert len(loads) == load_count, "rename reloaded a retained document"
        saved = json.loads((Path(env["XDG_STATE_HOME"]) / "lavish-browser/state.json").read_text())
        assert "Final" in json.dumps(saved), "rename was not persisted"
        assert app.poll() is None
        print("PASS: native rename, Unicode/empty labels, selection/presentation preservation, unknown no-op, persistence and retained pages")
    finally:
        app.terminate()
        app.wait(timeout=10)
        server.shutdown()
        log.seek(0)
        print(log.read())
        log.close()
