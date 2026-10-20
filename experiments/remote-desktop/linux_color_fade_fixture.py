"""Bounded GNOME 1280x800 color fixture for testing settled remote colors.

Run as the active test user. Temporarily changes a single screen, restores the
original layout in finally, and saves only generated fixture metadata. Creating
color-fade inside the owned runtime directory triggers one spatial checker fade.
The steady RGB is [20,81,74]. Observe the actual viewer before/after the fade;
this is not a frame-rate test. SIGTERM or the bounded deadline restores layout.
"""
import json
import os
import pathlib
import signal
import sys
import time

os.environ.update(DBUS_SESSION_BUS_ADDRESS='unix:path=/run/user/1000/bus',
                  XDG_RUNTIME_DIR='/run/user/1000', GDK_BACKEND='wayland')
import gi
gi.require_version('Gtk', '3.0')
from gi.repository import Gio, GLib, Gtk


def main():
    root = pathlib.Path(sys.argv[1]).resolve(strict=True)
    assert root.parent == pathlib.Path('/tmp') and root.name.startswith('pab-linux-viewer-')
    assert root.stat().st_uid == os.getuid() and os.getuid() == 1000
    os.umask(0o077)
    bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)

    def call(method, args=None):
        return bus.call_sync('org.gnome.Mutter.DisplayConfig',
            '/org/gnome/Mutter/DisplayConfig', 'org.gnome.Mutter.DisplayConfig',
            method, args, None, Gio.DBusCallFlags.NONE, 5000, None).unpack()

    before = call('GetCurrentState')
    _, monitors, logical, properties = before
    assert len(monitors) == len(logical) == len(logical[0][5]) == 1
    spec, modes, monitor_properties = monitors[0]
    current = [m for m in modes if m[6].get('is-current')]
    targets = [m for m in modes if list(m[1:3]) == [1280, 800]]
    assert len(current) == 1 and targets
    target = min(targets, key=lambda m: abs(m[3] - 60))
    layout = logical[0]
    assert layout[2] == 1 and layout[3] == 0 and layout[2] in target[5]
    options = {}
    if 'is-underscanning' in monitor_properties:
        options['enable_underscanning'] = GLib.Variant('b', monitor_properties['is-underscanning'])
    global_options = {}
    if properties.get('supports-changing-layout-mode') and 'layout-mode' in properties:
        global_options['layout-mode'] = GLib.Variant('u', properties['layout-mode'])

    def apply(mode):
        serial = call('GetCurrentState')[0]
        config = [tuple(layout[:5]) + ([(spec[0], mode, options)],)]
        call('ApplyMonitorsConfig', GLib.Variant('(uua(iiduba(ssa{sv}))a{sv})',
             (serial, 1, config, global_options)))

    def stop(*_):
        Gtk.main_quit()

    report = {'before_mode': current[0][0], 'test_mode': target[0],
              'persistent': False, 'dimensions': [1280, 800]}
    window = None
    try:
        apply(target[0])
        window = Gtk.Window(title='Pixels generated color transition fixture')
        area = Gtk.DrawingArea()
        counter = [0]
        start = time.monotonic(); fade_start = [None]

        def draw(_, cr):
            phase = 20 if fade_start[0] is None else min(20, counter[0] - fade_start[0])
            if phase == 20:
                cr.set_source_rgb(20/255,81/255,74/255);cr.paint()
            else:
                for y in range(0,800,8):
                    for x in range(0,1280,8):
                        col,row=x//8,y//8
                        pattern=[((col*17+row*31)&1)*255,((col*3^row*13)&2)//2*255,((col*7+row*11)&4)//4*255]
                        color=[((value*(20-phase)+target*phase)//20)/255 for value,target in zip(pattern,[20,81,74])]
                        cr.set_source_rgb(*color);cr.rectangle(x,y,8,8);cr.fill()
            return True

        def tick():
            counter[0] += 1
            if fade_start[0] is None and (root/'color-fade').exists(): fade_start[0]=counter[0]
            area.queue_draw()
            if (root / 'animation-stop').exists() or time.monotonic() - start > 180:
                Gtk.main_quit()
                return False
            return True

        def progress():
            state = {'generated_ticks': counter[0], 'elapsed': time.monotonic() - start,
                     'width': area.get_allocated_width(), 'height': area.get_allocated_height(),
                     'fade_start': fade_start[0], 'fade_settled': fade_start[0] is not None and counter[0]-fade_start[0]>=20, 'updated': time.time()}
            temp = root / 'animation-next.json'
            temp.write_text(json.dumps(state))
            temp.replace(root / 'animation.json')
            return True

        from gi.repository import Gdk
        def hide_pointer(widget):
            widget.get_window().set_cursor(Gdk.Cursor.new_for_display(widget.get_display(),Gdk.CursorType.BLANK_CURSOR))
        area.connect('realize',hide_pointer)
        area.connect('draw', draw)
        window.add(area)
        window.fullscreen()
        window.show_all()
        window.present()
        GLib.timeout_add(33, tick)
        GLib.timeout_add(250, progress)
        # GLib signal sources leave monitor restoration outside the GUI callback.
        GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGTERM, lambda: (stop(), False)[1])
        GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGINT, lambda: (stop(), False)[1])
        Gtk.main()
        progress()
        report['generated_ticks'] = counter[0]
    finally:
        if window is not None:
            window.destroy()
        apply(current[0][0])
        restored = call('GetCurrentState')
        report['restored'] = restored[2] == before[2] and [m[0] for m in restored[1][0][1]
            if m[6].get('is-current')] == [current[0][0]]
        (root / 'animation-result.json').write_text(json.dumps(report))
        print(json.dumps(report), flush=True)
    assert report['restored']


if __name__ == '__main__':
    main()
