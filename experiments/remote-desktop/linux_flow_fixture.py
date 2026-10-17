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
from gi.repository import Gtk, Gdk, GLib

root = pathlib.Path(sys.argv[1]).resolve(strict=True)
assert root.parent == pathlib.Path("/tmp") and root.name.startswith("pab-linux-viewer-")
assert root.stat().st_uid == os.getuid() == 1000
os.umask(0o077)
window = Gtk.Window(title="Pixels generated flow acceptance")
area = Gtk.DrawingArea()
area.set_can_focus(True)
area.add_events(Gdk.EventMask.BUTTON_PRESS_MASK | Gdk.EventMask.BUTTON_RELEASE_MASK |
                Gdk.EventMask.KEY_PRESS_MASK | Gdk.EventMask.KEY_RELEASE_MASK)
ticks = 0
draws = 0
started = time.monotonic()
counts = dict(clicks=0, releases=0, left_held=False, ctrl_down=0, ctrl_up=0, ctrl_held=False)


def pointer(_, event):
    if event.button == 1 and event.type in (Gdk.EventType.BUTTON_PRESS, Gdk.EventType.BUTTON_RELEASE):
        down = event.type == Gdk.EventType.BUTTON_PRESS
        counts["clicks" if down else "releases"] += 1
        counts["left_held"] = down
        if down:
            area.grab_focus()
    return True


def key(_, event):
    # Only aggregate the fixture's Ctrl transitions; never record typed text.
    if event.hardware_keycode in (37, 105):
        down = event.type == Gdk.EventType.KEY_PRESS
        counts["ctrl_down" if down else "ctrl_up"] += 1
        counts["ctrl_held"] = down
    return True


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
    if not (root / "pause-animation").exists():
        ticks += 1
        area.queue_draw()
    if time.monotonic() - started >= 120:
        window.destroy()
        return False
    return True


def report():
    data = dict(ticks=ticks, draws=draws, elapsed=time.monotonic() - started,
                width=area.get_allocated_width(), height=area.get_allocated_height(),
                focused=window.is_active(), **counts)
    pending = root / "fixture-next.json"
    pending.write_text(json.dumps(data), encoding="utf-8")
    pending.replace(root / "fixture.json")
    return True


area.connect("draw", draw)
for name in ("button-press-event", "button-release-event"):
    area.connect(name, pointer)
for name in ("key-press-event", "key-release-event"):
    area.connect(name, key)
window.add(area)
window.show_all()
window.fullscreen()
window.present()
area.grab_focus()
window.connect("destroy", lambda _: Gtk.main_quit())
GLib.timeout_add(16, tick)
GLib.timeout_add(100, report)
Gtk.main()
