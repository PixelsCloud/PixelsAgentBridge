"""Generated fullscreen edges and bounded local click coordinates; no mode change.

Run as the active test user in an owned /tmp/pab-linux-viewer-* directory.
One-pixel white borders detect cropping of the last visible row/column. Only
events received by this window are counted; no global input hook or user text.
"""
import json
import os
import pathlib
import sys
import time

os.environ.update(DBUS_SESSION_BUS_ADDRESS='unix:path=/run/user/1000/bus',
                  XDG_RUNTIME_DIR='/run/user/1000', GDK_BACKEND='wayland')
import gi
gi.require_version('Gtk', '3.0')
from gi.repository import Gtk, Gdk, GLib

root = pathlib.Path(sys.argv[1]).resolve(strict=True)
assert root.parent == pathlib.Path('/tmp') and root.name.startswith('pab-linux-viewer-')
assert root.stat().st_uid == os.getuid() == 1000
os.umask(0o077)
counts = {'clicks': 0, 'releases': 0, 'left_held': False, 'points': []}
window = Gtk.Window(title='Pixels generated edge acceptance')
area = Gtk.DrawingArea()
area.add_events(Gdk.EventMask.BUTTON_PRESS_MASK | Gdk.EventMask.BUTTON_RELEASE_MASK)

def pointer(_, event):
    if event.button == 1:
        down = event.type == Gdk.EventType.BUTTON_PRESS
        counts['clicks' if down else 'releases'] += 1
        counts['left_held'] = down
        if down:
            counts['points'] = (counts['points'] + [[event.x, event.y]])[-16:]
    return True

def draw(widget, cr):
    w, h = widget.get_allocated_width(), widget.get_allocated_height()
    cr.set_source_rgb(20/255, 81/255, 74/255)
    cr.paint()
    cr.set_source_rgb(1, 1, 1)
    for x, y, width, height in [(0,0,w,1), (0,h-1,w,1), (0,0,1,h), (w-1,0,1,h)]:
        cr.rectangle(x, y, width, height)
        cr.fill()
    cr.set_font_size(24)
    cr.move_to(36, 60)
    cr.show_text(f'Pixels edge acceptance: {w} x {h}')
    return True

def hide_pointer(widget):
    widget.get_window().set_cursor(Gdk.Cursor.new_for_display(widget.get_display(), Gdk.CursorType.BLANK_CURSOR))

def report():
    value = {**counts, 'width': area.get_allocated_width(), 'height': area.get_allocated_height(),
             'focused': window.is_active(), 'updated': time.time()}
    temporary = root/'fixture-next.json'
    temporary.write_text(json.dumps(value))
    temporary.replace(root/'fixture.json')
    return True

area.connect('draw', draw)
area.connect('realize', hide_pointer)
for name in ('button-press-event', 'button-release-event'):
    area.connect(name, pointer)
window.add(area)
window.show_all()
window.fullscreen()
window.present()
GLib.timeout_add(100, report)
GLib.timeout_add(420000, lambda: (window.destroy(), False)[-1])
window.connect('destroy', lambda _: Gtk.main_quit())
Gtk.main()
