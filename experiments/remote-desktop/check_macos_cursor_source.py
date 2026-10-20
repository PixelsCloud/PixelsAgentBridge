"""Opt-in Mac live cursor check using only the test's own foreground window.

Build tests/native_macos_cursor_source.mm on the Mac, then provide that fixture
and the real pab-media-worker. Refuses locked/non-console sessions. It does not
request permissions, change credentials, or lock/unlock the system.
"""
import argparse
import hashlib
import json
import os
import pathlib
import plistlib
import select
import struct
import subprocess
import time


def console_sessions(node):
    if isinstance(node, dict):
        if "IOConsoleUsers" in node:
            return node["IOConsoleUsers"]
        values = node.values()
    elif isinstance(node, list):
        values = node
    else:
        return []
    for value in values:
        result = console_sessions(value)
        if result:
            return result
    return []


def read(process, size):
    result = bytearray()
    deadline = time.monotonic() + 5
    while len(result) < size:
        if not select.select([process.stdout], [], [], max(0, deadline-time.monotonic()))[0]:
            raise RuntimeError("Native cursor IPC timed out")
        part = os.read(process.stdout.fileno(), size-len(result))
        if not part:
            raise RuntimeError("Native cursor IPC closed")
        result.extend(part)
    return bytes(result)


def send(process, data):
    process.stdin.write(data)
    process.stdin.flush()


def stop(process):
    if process.poll() is None:
        try:
            send(process, b"\0")
            process.wait(timeout=3)
            process.communicate(timeout=3)
        except (OSError, subprocess.TimeoutExpired):
            process.kill()
            process.communicate(timeout=3)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("worker", type=pathlib.Path)
    parser.add_argument("fixture", type=pathlib.Path)
    args = parser.parse_args()
    worker, fixture_path = args.worker.resolve(strict=True), args.fixture.resolve(strict=True)
    tree = plistlib.loads(subprocess.check_output(["/usr/sbin/ioreg", "-a", "-n", "Root", "-d", "1"], timeout=10))
    assert os.getuid() != 0 and os.stat("/dev/console").st_uid == os.getuid()
    assert any(s.get("kCGSSessionUserIDKey") == os.getuid() and s.get("kCGSSessionOnConsoleKey")
               and not s.get("CGSSessionScreenIsLocked", False) for s in console_sessions(tree)), "Unlock the Mac before running this check"
    screen = next(s for s in json.loads(subprocess.check_output([str(worker), "inventory"], timeout=10)) if s["primary"])
    fixture = subprocess.Popen([str(fixture_path), "fixture"], stdin=subprocess.PIPE,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, bufsize=0)
    cursor = None
    reports = []
    sequence, image_id, shape = 0, 0, None
    try:
        assert struct.unpack("<I", read(fixture, 4))[0] == 0x31465843
        send(fixture, b"\1")
        assert read(fixture, 1) == b"\1"
        cursor = subprocess.Popen([str(worker), "cursor", screen["id"]], stdin=subprocess.PIPE,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE, bufsize=0)
        send(cursor, struct.pack("<iiIII", screen["x"], screen["y"], screen["desktop_width"], screen["desktop_height"], 0))
        assert struct.unpack("<II", read(cursor, 8)) == (0x31424150, 1)

        def update():
            nonlocal sequence, image_id, shape
            send(cursor, b"\1")
            op, version, seq, visible, x, y, next_id, fmt = struct.unpack("<BHQBiiQB", read(cursor, 29))
            assert op == version == 1 and seq == sequence+1 and visible in (0, 1) and fmt in (0, 1)
            sequence = seq
            size = 0
            if fmt:
                shape = struct.unpack("<IIII", read(cursor, 16))
                w, h, hx, hy = shape
                assert 0 < w <= 512 and 0 < h <= 512 and hx < w and hy < h and next_id > image_id
                pixels = read(cursor, w*h*4)
                size = len(pixels)
                assert all(pixels[k] <= pixels[k//4*4+3] for k in range(size) if k % 4 < 3)
            else:
                assert next_id == image_id
            image_id = next_id
            assert not visible or image_id
            result = {"sequence": seq, "visible": bool(visible), "position": [x, y],
                      "image_id": image_id, "shape": shape, "payload_bytes": size}
            print(json.dumps({"sample": result}), flush=True)
            return result

        first = update()
        assert first["visible"], "Expected an unlocked visible fixture cursor"
        reports.append({"phase": "arrow", **first})
        sx = screen["pixel_width"] / screen["desktop_width"]
        sy = screen["pixel_height"] / screen["desktop_height"]
        expected_shape = tuple(int(v+0.5) for v in (9*sx, 7*sy, 3*sx, 2*sy))
        for command, name in [(2, "custom"), (5, "move"), (6, "move_back"), (3, "hidden"), (2, "shown"), (4, "arrow_restored")]:
            send(fixture, bytes([command]))
            assert read(fixture, 1) == bytes([command])
            result = update()
            if command == 3:
                assert not result["visible"]
            else:
                assert result["visible"]
                if command in (2, 5, 6):
                    assert result["shape"] == expected_shape
            if command in (5, 6):
                assert result["payload_bytes"] == 0
            if command == 5:
                assert result["position"][0] == first["position"][0]+20
            if command == 6:
                assert result["position"] == first["position"]
            reports.append({"phase": name, **result})
    finally:
        if cursor is not None:
            stop(cursor)
        stop(fixture)
    assert fixture.returncode == 0 and cursor.returncode == 0
    print(json.dumps({"worker_sha256": hashlib.sha256(worker.read_bytes()).hexdigest(),
                      "fixture_sha256": hashlib.sha256(fixture_path.read_bytes()).hexdigest(),
                      "screen": screen, "phases": reports, "video_frames_requested": 0,
                      "fixture_exit": fixture.returncode, "cursor_exit": cursor.returncode}))


if __name__ == "__main__":
    main()
