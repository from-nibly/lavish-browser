#!/usr/bin/env python3
import argparse
import json
import time
import pyatspi

parser = argparse.ArgumentParser()
parser.add_argument("--trace", required=True)
parser.add_argument("--gesture-only", action="store_true")
args = parser.parse_args()
trace = []

def walk(node):
    yield node
    try:
        for child in node:
            yield from walk(child)
    except Exception:
        return

def find_exact(name, timeout=30):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        for node in walk(pyatspi.Registry.getDesktop(0)):
            try:
                if (node.name or "") == name:
                    trace.append({"find": name, "role": node.getRoleName()})
                    return node
            except Exception:
                pass
        time.sleep(0.2)
    raise TimeoutError(name)

def activate(node):
    action = node.queryAction()
    trace.append({"activate": node.name, "actions": [action.getName(i) for i in range(action.nActions)]})
    if not action.doAction(0):
        raise RuntimeError(f"could not activate {node.name}")

try:
    editor = find_exact("Excalidraw whiteboard")
    click_to_edit = find_exact("Click to edit")
    activate(click_to_edit)
    time.sleep(1)
    extents = editor.queryComponent().getExtents(pyatspi.DESKTOP_COORDS)
    x = int(extents.x + extents.width * 0.55)
    y = int(extents.y + extents.height * 0.62)
    trace.append({"editor_extents": [extents.x, extents.y, extents.width, extents.height], "drag_start": [x, y]})
    pyatspi.Registry.generateKeyboardEvent(ord("r"), None, pyatspi.KEY_PRESSRELEASE)
    pyatspi.Registry.generateMouseEvent(x, y, "abs")
    pyatspi.Registry.generateMouseEvent(x, y, "b1p")
    for step in range(1, 6):
        pyatspi.Registry.generateMouseEvent(x + step * 20, y + step * 12, "abs")
        time.sleep(0.05)
    pyatspi.Registry.generateMouseEvent(x + 100, y + 60, "b1r")
    time.sleep(2)
    if args.gesture_only:
        trace.append({"gesture": "rectangle keyboard shortcut plus native pointer drag"})
        from pathlib import Path
        Path(args.trace).write_text(json.dumps(trace, indent=2))
        import os
        os._exit(0)
    note = find_exact("Optional note for the agent about these edits...")
    activate(note)
    for character in "live native Excalidraw rectangle persistence check":
        pyatspi.Registry.generateKeyboardEvent(ord(character), None, pyatspi.KEY_SYM)
    time.sleep(1)
    try:
        text = note.queryText()
        trace.append({"note_text": text.getText(0, text.characterCount)})
    except Exception as error:
        trace.append({"note_text_error": str(error)})
    queue = find_exact("Queue feedback")
    trace.append({"queue_states": [state.name for state in queue.getState().getStates()]})
    activate(queue)
    time.sleep(3)
    trace.append({"gesture": "rectangle keyboard shortcut plus native pointer drag", "queued": True})
finally:
    from pathlib import Path
    Path(args.trace).write_text(json.dumps(trace, indent=2))
