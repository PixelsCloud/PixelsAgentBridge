"""Opt-in, two-minute generated animation in an owned Wayland test window.

No input injection or display-mode change. The caller creates the private
/tmp/pab-linux-viewer-* directory and supplies normal test service cleanup.
This simple animation tests healthy scheduling, not worst-case codec bitrate.
"""
import json
import os
import pathlib
import sys
import time

os.environ.update(DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/1000/bus",
                  XDG_RUNTIME_DIR="/run/user/1000", GDK_BACKEND="wayland")
import gi
gi.require_version("Gtk", "3.0")
from gi.repository import Gtk, GLib

root = pathlib.Path(sys.argv[1]).resolve(strict=True)
assert root.parent == pathlib.Path("/tmp") and root.name.startswith("pab-linux-viewer-")
assert root.stat().st_uid == os.getuid() == 1000
os.umask(0o077)
window = Gtk.Window(title="Pixels generated flow acceptance")
area = Gtk.DrawingArea()
ticks = 0
draws = 0
started = time.monotonic()


def draw(widget, cr):
    global draws
    draws += 1
    w, h = widget.get_allocated_width(), widget.get_allocated_height()
    cr.set_source_rgb(.06, .18, .16)
    cr.paint()
    for index in range(12):
        phase = (ticks * 3 + index * 17) % 256
        cr.set_source_rgb(phase / 255, (255 - phase) / 255, .35)
        cr.rectangle(index * w / 12, h / 3, w / 12, h / 3)
        cr.fill()
    cr.set_source_rgb(1, 1, 1)
    cr.set_font_size(26)
    cr.move_to(32, 72)
    cr.show_text("Pixels generated flow acceptance")
    return True


def tick():
    global ticks
    ticks += 1
    area.queue_draw()
    if time.monotonic() - started >= 120:
        window.destroy()
        return False
    return True


def report():
    data = dict(ticks=ticks, draws=draws, elapsed=time.monotonic() - started,
                width=area.get_allocated_width(), height=area.get_allocated_height())
    pending = root / "fixture-next.json"
    pending.write_text(json.dumps(data), encoding="utf-8")
    pending.replace(root / "fixture.json")
    return True


area.connect("draw", draw)
window.add(area)
window.show_all()
window.fullscreen()
window.connect("destroy", lambda _: Gtk.main_quit())
GLib.timeout_add(16, tick)
GLib.timeout_add(100, report)
Gtk.main()
