#!/usr/bin/env python3
"""Validate real evidence inputs and generate the installed capability verdict."""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path

REQUIRED_SCENARIOS = {
    "standalone", "multi_project_single_instance", "dedupe",
    "shared_session_feedback", "stale_url_reconnect", "native_close_reopen",
    "plugin_focus", "plugin_degradation", "accessibility", "native_download",
    "memory", "persistence",
}


def load(path: Path):
    if not path.is_file():
        raise SystemExit(f"required evidence missing: {path}")
    return json.loads(path.read_text())


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--automated-root", type=Path, required=True)
    parser.add_argument("--manual-root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    scenarios_path = args.automated_root / "integration/scenarios.json"
    compatibility_path = args.automated_root / "compatibility/capabilities.json"
    artifacts_path = args.automated_root / "integration/installed-artifacts.json"
    scenarios = load(scenarios_path)
    compatibility = load(compatibility_path)
    artifacts = load(artifacts_path)
    observation = load(args.manual_root / "manual-observation.json")
    poll_text = (args.manual_root / "poll-whiteboard.txt").read_text()
    manual_hash_line = (args.manual_root / "installed-browser-sha256.txt").read_text().strip()

    missing = REQUIRED_SCENARIOS - scenarios.keys()
    failed = {name for name in REQUIRED_SCENARIOS if scenarios.get(name, {}).get("status") != "pass"}
    if missing or failed:
        raise SystemExit(f"installed scenarios incomplete: missing={sorted(missing)}, failed={sorted(failed)}")
    automated_caps = compatibility["capabilities"]
    blockers = {name for name, result in automated_caps.items() if result["status"] != "pass"}
    if blockers - {"excalidraw"}:
        raise SystemExit(f"automated compatibility blockers remain: {sorted(blockers)}")

    expected_hash = artifacts["bin/lavish-browser"]["sha256"]
    manual_hash, manual_path = manual_hash_line.split(maxsplit=1)
    actual_manual_hash = hashlib.sha256(Path(manual_path).read_bytes()).hexdigest()
    manual_ok = all([
        observation.get("classification") == "manual-live",
        observation.get("shape_visible_before_reload") is True,
        observation.get("shape_persisted_after_reload_artifact") is True,
        observation.get("feedback_sent_to_agent") is True,
        observation.get("poll_exit_code") == 0,
        observation.get("poll_tag") == "whiteboard",
        observation.get("added_rectangle_id") in poll_text,
        observation.get("note") in poll_text,
        "tag: whiteboard" in poll_text,
        "stats:" in poll_text and "added: 1" in poll_text,
        ".excalidraw" in poll_text and "PNG preview:" in poll_text,
        manual_hash == expected_hash == actual_manual_hash,
    ])
    if not manual_ok:
        raise SystemExit("manual Excalidraw evidence failed integrity correlation")

    capability_results = {name: value for name, value in automated_caps.items()}
    capability_results["excalidraw"] = {
        "status": "pass",
        "classification": "manual-live",
        "evidence": f"{args.manual_root}/manual-observation.json and poll-whiteboard.txt; installed browser sha256 {manual_hash}",
    }
    result = {
        "schema_version": 1,
        "status": "pass",
        "classification": "installed-live",
        "generated_from": {
            "scenarios": str(scenarios_path),
            "compatibility": str(compatibility_path),
            "installed_artifacts": str(artifacts_path),
            "manual_observation": str(args.manual_root / "manual-observation.json"),
            "manual_poll": str(args.manual_root / "poll-whiteboard.txt"),
        },
        "installed_browser_sha256": manual_hash,
        "scenarios": scenarios,
        "compatibility": capability_results,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(f"complete installed capability gate: PASS ({args.output})")


if __name__ == "__main__":
    main()
