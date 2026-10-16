"""Bounded, opt-in GNOME test: temporary 1080p, capture probe, restore in finally.

Run as the active desktop user, with its session bus. No persistent configuration
is written. Restricted to one monitor to avoid changing a multi-monitor layout.
Reference: GNOME Mutter 42.9 DisplayConfig D-Bus interface.
"""
import json
import pathlib
import signal
import subprocess
import sys
import time

from gi.repository import Gio, GLib


def main():
    probe = pathlib.Path(sys.argv[1]).resolve(strict=True)
    if len(sys.argv) != 3 or sys.argv[2] not in ("--animate", "--fullscreen", "--native-worker", "--embedded-worker"):
        raise SystemExit("usage: linux_capture_1080.py PROBE --animate|--fullscreen|--native-worker|--embedded-worker")
    bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)

    def call(method, args=None):
        return bus.call_sync(
            "org.gnome.Mutter.DisplayConfig", "/org/gnome/Mutter/DisplayConfig",
            "org.gnome.Mutter.DisplayConfig", method, args, None,
            Gio.DBusCallFlags.NONE, 5000, None,
        ).unpack()

    before = call("GetCurrentState")
    serial, monitors, logical, properties = before
    if len(monitors) != 1 or len(logical) != 1 or len(logical[0][5]) != 1:
        raise RuntimeError("Test requires exactly one physical and logical monitor")
    spec, modes, monitor_properties = monitors[0]
    connector = spec[0]
    current = [m for m in modes if m[6].get("is-current")]
    targets = [m for m in modes if m[1:3] == (1920, 1080) or list(m[1:3]) == [1920, 1080]]
    if len(current) != 1 or not targets:
        raise RuntimeError("Current mode or 1080p target unavailable")
    target = min(targets, key=lambda m: abs(m[3] - 60))
    layout = logical[0]
    if layout[2] not in target[5]:
        raise RuntimeError("1080p does not support the existing scale")
    monitor_options = {}
    if "is-underscanning" in monitor_properties:
        monitor_options["enable_underscanning"] = GLib.Variant("b", monitor_properties["is-underscanning"])
    global_options = {}
    if properties.get("supports-changing-layout-mode") and "layout-mode" in properties:
        global_options["layout-mode"] = GLib.Variant("u", properties["layout-mode"])

    def apply(mode):
        fresh_serial = call("GetCurrentState")[0]
        config = [tuple(layout[:5]) + ([(connector, mode, monitor_options)],)]
        args = GLib.Variant("(uua(iiduba(ssa{sv}))a{sv})", (fresh_serial, 1, config, global_options))
        call("ApplyMonitorsConfig", args)

    def stop(signum, frame):
        raise InterruptedError("Interrupted; restoring monitor mode")

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    report = {"before_mode": current[0][0], "test_mode": target[0], "persistent": False}
    try:
        apply(target[0])
        time.sleep(1)
        command = ([sys.executable, str(probe.parent / "verify_linux_worker.py"), str(probe),
                    str(probe.parent / "libpab-codec-check.so")]
                   if sys.argv[2] in ("--native-worker", "--embedded-worker") else [str(probe), connector, sys.argv[2]])
        if sys.argv[2] == "--embedded-worker":
            command.append("--embedded")
        result = subprocess.run(command, capture_output=True, text=True, timeout=30)
        report["probe_exit"] = result.returncode
        report["probe_stderr"] = result.stderr[-2000:]
        report["capture"] = json.loads(result.stdout)
    finally:
        apply(current[0][0])
        restored = call("GetCurrentState")
        restored_modes = [m[0] for m in restored[1][0][1] if m[6].get("is-current")]
        report["restored_mode"] = restored_modes
        report["restored"] = restored_modes == [current[0][0]] and restored[2] == before[2]
        print(json.dumps(report), flush=True)
    assert report["restored"], "Monitor layout restoration failed"
    assert report["probe_exit"] == 0, "Capture probe failed"
    capture = report["capture"]
    dimensions = capture.get("dimensions", [capture.get("width"), capture.get("height")])
    assert dimensions == [1920, 1080], "Capture was not 1080p"


if __name__ == "__main__":
    main()
