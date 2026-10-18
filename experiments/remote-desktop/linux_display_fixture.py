"""Bounded GNOME 1080p animation for measuring actual remote presentation.

Run as the active test user. Temporarily changes a single screen, restores the
original layout in finally, and saves only generated fixture metadata. A 12-bit
frame number and its complement are rendered in large black/white cells; a
viewer-side sampler can reject occlusion/tearing instead of counting stale frames.
"""
import argparse
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
    parser = argparse.ArgumentParser()
    parser.add_argument('root', type=pathlib.Path)
    parser.add_argument('--seconds', type=int, default=180)
    parser.add_argument('--inhibit-idle', action='store_true', help='Temporary session idle inhibitor for a bounded long-run fixture')
    args = parser.parse_args()
    assert 10 <= args.seconds <= 32400
    root = args.root.resolve(strict=True)
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
    targets = [m for m in modes if list(m[1:3]) == [1920, 1080]]
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
              'persistent': False, 'dimensions': [1920, 1080]}
    window = None
    idle_cookie = None
    try:
        if args.inhibit_idle:
            idle_cookie = bus.call_sync('org.gnome.SessionManager', '/org/gnome/SessionManager',
                'org.gnome.SessionManager', 'Inhibit',
                GLib.Variant('(susu)', ('pixels-remote-desktop-acceptance', 0,
                    'Bounded generated remote desktop measurement', 8)),
                GLib.VariantType('(u)'), Gio.DBusCallFlags.NONE, 5000, None).unpack()[0]
            report['temporary_idle_inhibitor'] = True
        apply(target[0])
        window = Gtk.Window(title='Pixels generated 1080p presentation fixture')
        area = Gtk.DrawingArea()
        counter = [0]
        start = time.monotonic()

        def draw(_, cr):
            number = counter[0]
            # Changing bars span the whole screen, beyond the frame-code ROI.
            for y in range(0, 1080, 90):
                for x in range(0, 1920, 120):
                    phase = (number * 3 + x // 12 + y // 9) % 128
                    cr.set_source_rgb(phase / 128, (128 - phase) / 128, .35)
                    cr.rectangle(x, y, 120, 90)
                    cr.fill()
            bits = [1, 0] + [(number >> i) & 1 for i in range(12)] + [0, 1]
            for row in range(2):
                for i, bit in enumerate(bits):
                    value = bit ^ row
                    cr.set_source_rgb(value, value, value)
                    cr.rectangle(192 + i * 96, 160 + row * 96, 96, 80)
                    cr.fill()
            return True

        def tick():
            counter[0] += 1
            area.queue_draw()
            if (root / 'animation-stop').exists() or time.monotonic() - start > args.seconds:
                Gtk.main_quit()
                return False
            return True

        def progress():
            state = {'generated_ticks': counter[0], 'elapsed': time.monotonic() - start,
                     'width': area.get_allocated_width(), 'height': area.get_allocated_height(),
                     'updated': time.time()}
            temp = root / 'animation-next.json'
            temp.write_text(json.dumps(state))
            temp.replace(root / 'animation.json')
            return True

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
        if idle_cookie is not None:
            bus.call_sync('org.gnome.SessionManager', '/org/gnome/SessionManager',
                'org.gnome.SessionManager', 'Uninhibit', GLib.Variant('(u)', (idle_cookie,)),
                None, Gio.DBusCallFlags.NONE, 5000, None)
    assert report['restored']


if __name__ == '__main__':
    main()
