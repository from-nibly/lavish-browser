#!/usr/bin/env python3
"""Query and drive stable Lavish Browser controls through the live AT-SPI tree."""
from __future__ import annotations

import argparse
import json
import time

import pyatspi


def walk(node, depth=0):
    if node is None or depth > 128:
        return
    try:
        yield node
        for index in range(node.childCount):
            yield from walk(node.getChildAtIndex(index), depth + 1)
    except (AttributeError, LookupError, RuntimeError):
        return


def entries():
    result = []
    for node in walk(pyatspi.Registry.getDesktop(0)):
        try:
            name = node.name or ""
            if not name:
                continue
            actions = []
            try:
                interface = node.queryAction()
                actions = [interface.getName(index) for index in range(interface.nActions)]
            except (AttributeError, LookupError, NotImplementedError, RuntimeError):
                pass
            result.append({"name": name, "role": node.getRoleName(), "actions": actions, "node": node})
        except (AttributeError, LookupError, RuntimeError):
            continue
    return result


def matching(needle):
    return [entry for entry in entries() if needle in entry["name"]]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=("assert", "absent", "click", "snapshot"))
    parser.add_argument("needle", nargs="?", default="")
    parser.add_argument("--timeout", type=float, default=30)
    parser.add_argument("--output")
    args = parser.parse_args()

    deadline = time.monotonic() + args.timeout
    while True:
        found = matching(args.needle)
        if args.command == "snapshot":
            serializable = [{key: value for key, value in entry.items() if key != "node"} for entry in entries()]
            text = json.dumps(serializable, indent=2) + "\n"
            if args.output:
                open(args.output, "w").write(text)
            else:
                print(text, end="")
            return 0
        if args.command == "assert" and found:
            print(json.dumps([{key: value for key, value in item.items() if key != "node"} for item in found], indent=2))
            return 0
        if args.command == "absent" and not found:
            print(json.dumps({"absent": args.needle}))
            return 0
        if args.command == "click" and found:
            actionable = next((item for item in found if item["actions"]), None)
            if actionable:
                interface = actionable["node"].queryAction()
                if not interface.doAction(0):
                    raise SystemExit(f"AT-SPI action failed for {actionable['name']}")
                print(json.dumps({"clicked": actionable["name"], "action": actionable["actions"][0]}))
                return 0
        if time.monotonic() >= deadline:
            observed = [item["name"] for item in entries()]
            raise SystemExit(f"timed out: {args.command} {args.needle!r}; observed={observed!r}")
        time.sleep(.2)


if __name__ == "__main__":
    raise SystemExit(main())
