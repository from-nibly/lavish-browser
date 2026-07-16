#!/usr/bin/env python3
"""Return the live Lavish Browser AT-SPI window focus/active state."""
import json
import time
import pyatspi


def walk(node, depth=0):
    if node is None or depth > 128:
        return
    yield node
    try:
        for index in range(node.childCount):
            yield from walk(node.getChildAtIndex(index), depth + 1)
    except (AttributeError, LookupError, RuntimeError):
        return


deadline = time.monotonic() + 10
while time.monotonic() < deadline:
    for node in walk(pyatspi.Registry.getDesktop(0)):
        try:
            name = node.name or ""
            if "Lavish Browser" not in name:
                continue
            states = node.getState()
            print(json.dumps({
                "name": name,
                "active": states.contains(pyatspi.STATE_ACTIVE),
                "focused": states.contains(pyatspi.STATE_FOCUSED),
            }, sort_keys=True))
            raise SystemExit(0)
        except (LookupError, RuntimeError):
            pass
    time.sleep(.2)
raise SystemExit("Lavish Browser window missing from AT-SPI")
