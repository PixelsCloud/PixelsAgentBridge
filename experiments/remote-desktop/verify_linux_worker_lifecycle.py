"""Bounded capture worker cancellation/layout checks in an active GNOME session.

No installed services are changed. A temporary single-display 1080p mode change
is restored in finally; this is an explicit test, never part of the product.
"""
import json
import os
import pathlib
import select
import struct
import subprocess
import sys
import time

from gi.repository import Gio, GLib


def main():
    worker = pathlib.Path(sys.argv[1]).resolve(strict=True)
    screen = next(s for s in json.loads(subprocess.check_output([str(worker), "inventory"], timeout=8)) if s["primary"])
    checks = []
    children = []

    def spawn(screen_id=None):
        child = subprocess.Popen([str(worker), "capture", screen_id or screen["id"], "15", "desktop"],
                                 stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, bufsize=0)
        children.append(child)
        return child

    def read(child, size):
        data = bytearray()
        deadline = time.monotonic() + 8
        while len(data) < size:
            assert time.monotonic() < deadline, "Pipe read timed out"
            if select.select([child.stdout], [], [], .05)[0]:
                chunk = os.read(child.stdout.fileno(), size - len(data))
                assert chunk, "Capture pipe ended early: " + child.stderr.read(2000).decode(errors="replace")
                data.extend(chunk)
        return data

    def ready(child):
        assert struct.unpack("<III", read(child, 12)) == (0x31424150, screen["pixel_width"], screen["pixel_height"])

    def frame(child):
        child.stdin.write(b"\x01")
        size, _, _, _ = struct.unpack("<IQQI", read(child, 24))
        assert 0 < size <= 16 * 1024 * 1024
        read(child, size)

    try:
        bad = spawn("gnome:missing-test-connector")
        assert bad.wait(timeout=8) == 1
        assert b"Selected display is unavailable" in bad.stderr.read(2000)
        checks.append("missing_screen_rejected")

        invalid = spawn(); ready(invalid)
        invalid.stdin.write(b"\xff")
        assert invalid.wait(timeout=5) == 1
        assert b"Invalid capture command" in invalid.stderr.read(2000)
        checks.append("invalid_command_retires_worker")

        closed = spawn(); ready(closed); frame(closed)
        closed.stdin.write(b"\x01")
        closed.stdin.close()
        assert closed.wait(timeout=5) == 1
        checks.append("parent_eof_during_capture_retires_worker")

        blocked = spawn(); ready(blocked)
        # Stop consuming compressed output, then close the parent request pipe.
        blocked.stdin.write(b"\x01")
        time.sleep(.3)
        blocked.stdin.close()
        assert blocked.wait(timeout=6) == 1
        checks.append("parent_eof_without_consuming_video_retires_worker")

        for _ in range(3):
            normal = spawn(); ready(normal); frame(normal)
            normal.stdin.write(b"\x00")
            assert normal.wait(timeout=5) == 0
        checks.append("three_capture_reopen_cycles")

        bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
        def call(method, args=None):
            return bus.call_sync("org.gnome.Mutter.DisplayConfig", "/org/gnome/Mutter/DisplayConfig",
                "org.gnome.Mutter.DisplayConfig", method, args, None, Gio.DBusCallFlags.NONE, 5000, None).unpack()
        before = call("GetCurrentState")
        _, physical, logical, props = before
        assert len(physical) == len(logical) == 1 and len(logical[0][5]) == 1
        connector = physical[0][0][0]
        modes = physical[0][1]
        original = next(m for m in modes if m[6].get("is-current"))
        target = next(m for m in modes if list(m[1:3]) == [1920,1080] and m[0] != original[0])
        options = {}
        if props.get("supports-changing-layout-mode"):
            options["layout-mode"] = GLib.Variant("u", props["layout-mode"])
        monitor_options = {}
        if "is-underscanning" in physical[0][2]:
            monitor_options["enable_underscanning"] = GLib.Variant("b", physical[0][2]["is-underscanning"])
        def apply(mode):
            args = GLib.Variant("(uua(iiduba(ssa{sv}))a{sv})", (call("GetCurrentState")[0], 1,
                [tuple(logical[0][:5]) + ([(connector, mode, monitor_options)],)], options))
            call("ApplyMonitorsConfig", args)
        changed = spawn(); ready(changed); frame(changed)
        try:
            apply(target[0])
            time.sleep(.2)
            changed.stdin.write(b"\x01")
            assert changed.wait(timeout=6) == 1
            error = changed.stderr.read(3000)
            assert b"changed" in error or b"retired" in error
            checks.append("layout_change_retires_old_capture_generation")
        finally:
            apply(original[0])
            restored = call("GetCurrentState")
            assert restored[2] == before[2]
            assert next(m[0] for m in restored[1][0][1] if m[6].get("is-current")) == original[0]
            checks.append("original_monitor_mode_and_layout_restored")
        after = spawn(); ready(after); frame(after)
        after.stdin.write(b"\x00")
        assert after.wait(timeout=5) == 0
        checks.append("new_capture_after_restoration")
        print(json.dumps({"checks":checks, "passed":True, "spawned":len(children),
            "all_reaped":all(c.poll() is not None for c in children), "pixel_bytes_saved":0}))
    finally:
        for child in children:
            if child.poll() is None:
                child.kill()
            child.wait(timeout=5)


if __name__ == "__main__":
    main()
