"""Read only the explicitly selected diagnostic process's AT-SPI tree.

Run as the graphical-session user, with that session's D-Bus environment.
No node names, text, values or passwords are read or printed. This performs
no accessibility actions and does not enable remote control.
"""
import argparse
import json
import os
from pathlib import Path

import pyatspi

parser = argparse.ArgumentParser()
parser.add_argument("pid", type=int)
parser.add_argument("--exe", type=Path, required=True)
parser.add_argument("--extents", action="store_true")
args = parser.parse_args()
expected = args.exe.resolve(strict=True)
process = Path("/proc") / str(args.pid)
assert process.stat().st_uid == os.getuid(), "Process is not owned by this user"
assert Path(os.readlink(process / "exe")) == expected, "Process executable changed"
apps = [app for app in pyatspi.Registry.getDesktop(0)
        if app and app.get_process_id() == args.pid]
rows = []
for app in apps:
    queue = [(app, 0)]
    while queue and len(rows) < 350:
        node, depth = queue.pop(0)
        try:
            role = node.getRoleName()
            state = node.getState()
            row = {"depth": depth, "role": role,
                   "enabled": state.contains(pyatspi.STATE_ENABLED),
                   "focused": state.contains(pyatspi.STATE_FOCUSED)}
            if args.extents and role in ("frame", "drawing area", "push button", "toggle button"):
                row["rect"] = list(node.queryComponent().getExtents(pyatspi.DESKTOP_COORDS))
            rows.append(row)
            if depth < 18:
                queue.extend((child, depth + 1) for child in node if child)
        except (LookupError, RuntimeError, NotImplementedError) as error:
            rows.append({"depth": depth, "error_type": type(error).__name__})
print(json.dumps({"pid": args.pid, "apps": len(apps), "nodes": rows}))
