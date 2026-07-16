#!/usr/bin/env python3
"""Assert installed native GTK chrome through the live AT-SPI tree."""
import sys
import time
import pyatspi

expected = sys.argv[1:]


def walk(node, depth=0):
    if node is None or depth > 128:
        return
    try:
        yield node
        for index in range(node.childCount):
            yield from walk(node.getChildAtIndex(index), depth + 1)
    except (AttributeError, LookupError, RuntimeError):
        return


def snapshot():
    desktop = pyatspi.Registry.getDesktop(0)
    names = []
    for node in walk(desktop):
        try:
            if name := node.name:
                names.append(name)
        except Exception:
            continue
    return names


deadline = time.monotonic() + 20
names = []
while time.monotonic() < deadline:
    names = snapshot()
    joined = "\n".join(names)
    project_tabs = sum(name.startswith("Project ") and "Close project" in name for name in names)
    if "Lavish projects" in joined and project_tabs >= 3 and all(name in joined for name in expected):
        print(f"AT-SPI exposed {project_tabs} project tabs and active document rows: {', '.join(expected)}")
        raise SystemExit(0)
    time.sleep(.25)

print("AT-SPI names observed:", file=sys.stderr)
print("\n".join(names), file=sys.stderr)
raise SystemExit("required Lavish native accessible labels were not observed")
