#!/usr/bin/env python3
"""Assert installed native GTK chrome through the live AT-SPI tree."""
import sys
import time
import pyatspi

expected = sys.argv[1:]


def walk(node, depth=0):
    try:
        yield node
        for index in range(node.childCount):
            yield from walk(node.getChildAtIndex(index), depth + 1)
    except (LookupError, RuntimeError):
        return


def snapshot():
    desktop = pyatspi.Registry.getDesktop(0)
    nodes = list(walk(desktop))
    names = [node.name for node in nodes if getattr(node, "name", "")]
    return names


deadline = time.monotonic() + 20
names = []
while time.monotonic() < deadline:
    names = snapshot()
    joined = "\n".join(names)
    if "Lavish projects" in joined and all(name in joined for name in expected):
        print(f"AT-SPI exposed Lavish projects and document rows: {', '.join(expected)}")
        raise SystemExit(0)
    time.sleep(.25)

print("AT-SPI names observed:", file=sys.stderr)
print("\n".join(names), file=sys.stderr)
raise SystemExit("required Lavish native accessible labels were not observed")
