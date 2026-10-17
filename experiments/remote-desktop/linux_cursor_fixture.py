"""Opt-in generated GNOME cursor fixture, bounded to seven minutes.

Records pointer coordinates and aggregate click counts only. It never injects
input. The caller creates an owned /tmp/pab-linux-viewer-* output directory.
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
gi.require_version("GdkPixbuf", "2.0")
from gi.repository import Gtk, Gdk, GdkPixbuf, GLib

root = pathlib.Path(sys.argv[1]).resolve(strict=True)
assert root.parent == pathlib.Path("/tmp") and root.name.startswith("pab-linux-viewer-")
assert root.stat().st_uid == os.getuid() == 1000
os.umask(0o077)
window = Gtk.Window(title="Pixels isolated Linux cursor acceptance")
area = Gtk.DrawingArea()
area.set_can_focus(True)
area.add_events(Gdk.EventMask.POINTER_MOTION_MASK | Gdk.EventMask.BUTTON_PRESS_MASK | Gdk.EventMask.BUTTON_RELEASE_MASK)
state = dict(x=None, y=None, shape=None, clicks=0, releases=0, held=False)
display = Gdk.Display.get_default()
rgba = bytes([0, 240, 220, 255] * (24 * 24))
pixbuf = GdkPixbuf.Pixbuf.new_from_bytes(GLib.Bytes.new(rgba), GdkPixbuf.Colorspace.RGB, True, 8, 24, 24, 96)
cursors = [Gdk.Cursor.new_for_display(display, Gdk.CursorType.LEFT_PTR),
    Gdk.Cursor.new_from_pixbuf(display, pixbuf, 5, 7),
    Gdk.Cursor.new_for_display(display, Gdk.CursorType.XTERM),
    Gdk.Cursor.new_for_display(display, Gdk.CursorType.BLANK_CURSOR)]
names = ["arrow", "cyan", "text", "hidden"]


def motion(widget, event):
    zone = min(3, max(0, int(event.x * 4 / widget.get_allocated_width())))
    widget.get_window().set_cursor(cursors[zone])
    state.update(x=event.x, y=event.y, shape=names[zone])
    return True


def button(_, event):
    if event.button == 1 and event.type in (Gdk.EventType.BUTTON_PRESS, Gdk.EventType.BUTTON_RELEASE):
        down = event.type == Gdk.EventType.BUTTON_PRESS
        state["clicks" if down else "releases"] += 1
        state["held"] = down
    return True


def draw(widget, cr):
    w, h = widget.get_allocated_width(), widget.get_allocated_height()
    for index in range(4):
        cr.set_source_rgb(.08 + index * .03, .24, .21)
        cr.rectangle(index * w / 4, 0, w / 4, h)
        cr.fill()
        cr.set_source_rgb(1, 1, 1)
        cr.set_font_size(24)
        cr.move_to(index * w / 4 + 35, 70)
        cr.show_text(names[index])
    return True


def report():
    data = dict(state, updated=time.time(), width=area.get_allocated_width(),
        height=area.get_allocated_height(), focused=window.is_active())
    temporary = root / "fixture-next.json"
    temporary.write_text(json.dumps(data), encoding="utf-8")
    temporary.replace(root / "fixture.json")
    return True


area.connect("motion-notify-event", motion)
area.connect("button-press-event", button)
area.connect("button-release-event", button)
area.connect("draw", draw)
window.add(area)
window.show_all()
window.fullscreen()
window.present()
area.grab_focus()
GLib.timeout_add(100, report)
GLib.timeout_add(420000, lambda: (window.destroy(), False)[-1])
window.connect("destroy", lambda _: Gtk.main_quit())
Gtk.main()
