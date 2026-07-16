#!/usr/bin/env python3
import argparse
import json
import time
import pyatspi

parser = argparse.ArgumentParser()
parser.add_argument("--destination", required=True)
parser.add_argument("--trace", required=True)
args = parser.parse_args()
trace = []

def walk(node):
    yield node
    try:
        for child in node:
            yield from walk(child)
    except Exception:
        return

def find_name(fragment, timeout=20):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        fallback = None
        for node in walk(pyatspi.Registry.getDesktop(0)):
            try:
                name = node.name or ""
                if name.lower() == fragment.lower():
                    trace.append({"find": fragment, "role": node.getRoleName(), "name": node.name})
                    return node
                if fallback is None and fragment.lower() in name.lower():
                    fallback = node
            except Exception:
                pass
        if fallback is not None:
            trace.append({"find": fragment, "role": fallback.getRoleName(), "name": fallback.name})
            return fallback
        time.sleep(0.2)
    raise TimeoutError(fragment)

def activate(node):
    actions = node.queryAction()
    trace.append({"activate": node.name, "actions": [actions.getName(i) for i in range(actions.nActions)]})
    if not actions.doAction(0):
        raise RuntimeError(f"could not activate {node.name}")

try:
    activate(find_name("More"))
    activate(find_name("Export standalone HTML"))
    save = find_name("Save", timeout=45)
    entries = []
    for node in walk(pyatspi.Registry.getDesktop(0)):
        try:
            if node.getRoleName() in ("entry", "text"):
                entries.append(node)
        except Exception:
            pass
    if not entries:
        raise RuntimeError("save chooser exposed no editable entry")
    target = entries[-1]
    trace.append({"editable": target.name, "role": target.getRoleName()})
    target.queryEditableText().setTextContents(args.destination)
    activate(save)
    deadline = time.monotonic() + 30
    from pathlib import Path
    destination = Path(args.destination)
    while time.monotonic() < deadline and not destination.exists():
        time.sleep(0.2)
    if not destination.exists():
        raise RuntimeError("download destination was not written")
    trace.append({"written": str(destination), "bytes": destination.stat().st_size})
finally:
    from pathlib import Path
    Path(args.trace).write_text(json.dumps(trace, indent=2))
