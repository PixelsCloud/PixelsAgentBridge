"""Real GNOME input into an owned, bounded GTK fixture; no installed app changes.

Checks absolute positioning, drag, wheel, chords and release after EOF, malformed
input and worker death. No keystroke contents or screen pixels are persisted.
"""
import json
import argparse
import os
import pathlib
import select
import struct
import subprocess
import sys
import time

import gi
gi.require_version("Gtk", "3.0")
from gi.repository import Gtk, Gdk, GLib, Gio


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('worker', type=pathlib.Path)
    parser.add_argument('--embedded', action='store_true')
    parser.add_argument('--uinput', action='store_true')
    args = parser.parse_args()
    worker = args.worker.resolve(strict=True)
    assert worker.parent.parent == pathlib.Path('/tmp') and worker.parent.name.startswith(('pab-linux-media-', 'pab-uinput-'))
    command = [str(worker)] + (["--media-worker"] if args.embedded else [])
    screens = json.loads(subprocess.check_output(command + ["inventory"], timeout=8))
    assert len(screens) == 1, "Fixture currently requires the single-monitor Linux VM"
    screen = screens[0]
    events, children, checks = [], [], []
    window = Gtk.Window(title="Pixels bounded remote input test")
    area = Gtk.DrawingArea()
    area.set_can_focus(True)
    area.add_events(Gdk.EventMask.POINTER_MOTION_MASK | Gdk.EventMask.BUTTON_PRESS_MASK |
                    Gdk.EventMask.BUTTON_RELEASE_MASK | Gdk.EventMask.KEY_PRESS_MASK |
                    Gdk.EventMask.KEY_RELEASE_MASK | Gdk.EventMask.SCROLL_MASK |
                    Gdk.EventMask.SMOOTH_SCROLL_MASK)
    def pointer(widget, event):
        events.append((event.type, event.x, event.y, int(event.state), getattr(event, 'button', 0)))
        return True
    def key(widget, event):
        events.append((event.type, event.keyval, int(event.state)))
        return True
    def scroll(widget, event):
        values = event.get_scroll_deltas()
        dy = values[2] if values[0] else (-1 if event.direction == Gdk.ScrollDirection.UP else 1)
        events.append((event.type, dy))
        return True
    for name in ['motion-notify-event', 'button-press-event', 'button-release-event']:
        area.connect(name, pointer)
    area.connect('key-press-event', key)
    area.connect('key-release-event', key)
    area.connect('scroll-event', scroll)
    def draw(widget, cr):
        cr.set_source_rgb(.08, .32, .29); cr.paint()
        cr.set_source_rgb(1, 1, 1); cr.set_font_size(30)
        cr.move_to(40, 80); cr.show_text('Pixels remote input verification')
        return True
    area.connect('draw', draw)
    window.add(area); window.show_all(); window.fullscreen(); window.present(); area.grab_focus()
    def pump(seconds=.1):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            while GLib.MainContext.default().pending(): GLib.MainContext.default().iteration(False)
            time.sleep(.002)
    def read(child, size):
        result = bytearray(); end = time.monotonic() + 8
        while len(result) < size:
            pump(.005)
            assert time.monotonic() < end, 'Input acknowledgement timed out'
            if select.select([child.stdout], [], [], .01)[0]:
                part = os.read(child.stdout.fileno(), size-len(result))
                assert part, 'Input ended: ' + child.stderr.read(2000).decode(errors='replace')
                result.extend(part)
        return bytes(result)
    def spawn(offset=0):
        child = subprocess.Popen(command + ['input-uinput' if args.uinput else 'input', screen['id']], stdin=subprocess.PIPE,
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE, bufsize=0)
        children.append(child)
        child.stdin.write(struct.pack('<iiII', screen['x']+offset, screen['y'], screen['desktop_width'], screen['desktop_height']))
        if not offset: assert read(child, 4) == struct.pack('<I', 0x31424150)
        return child
    def send(child, kind, x=0, y=0, value=0, down=False):
        child.stdin.write(struct.pack('<BHHHB', kind, round(x*65535), round(y*65535), value & 65535, down))
        assert read(child, 1) == b'\x01'
        pump()
    def wait(child, success=True):
        end = time.monotonic()+8
        while child.poll() is None and time.monotonic()<end: pump(.02)
        assert child.poll() is not None, 'Input child not reaped'
        if success: assert child.returncode == 0, child.stderr.read(2000).decode(errors='replace')
        pump(.2)
    def count(kind, value=None):
        return sum(e[0] == kind and (value is None or e[1] == value) for e in events)
    def normal_a(child):
        events.clear(); send(child, 4, value=0x41, down=True); send(child, 4, value=0x41)
        assert count(Gdk.EventType.KEY_PRESS, Gdk.KEY_a) == 1
        for e in events:
            if e[0] == Gdk.EventType.KEY_PRESS:
                assert not e[2] & (Gdk.ModifierType.CONTROL_MASK | Gdk.ModifierType.SHIFT_MASK | Gdk.ModifierType.BUTTON1_MASK)
    try:
        # Mapping/fullscreen is asynchronous on Wayland. Check the actual
        # allocation before interpreting absolute input coordinates.
        expected = (screen['desktop_width'], screen['desktop_height'])
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            pump(.05)
            if (area.get_allocated_width(), area.get_allocated_height()) == expected:
                break
        allocated = (area.get_allocated_width(), area.get_allocated_height())
        assert allocated == expected, {'fixture_allocation': allocated, 'screen_geometry': expected,
                                       'scale': area.get_scale_factor()}
        child = spawn()
        send(child, 2, .25, .35, down=True); send(child, 2, .25, .35)
        presses = [e for e in events if e[0] == Gdk.EventType.BUTTON_PRESS]
        assert len(presses) == 1
        assert abs(presses[0][1] - .25*(screen['desktop_width']-1)) < 2
        assert abs(presses[0][2] - .35*(screen['desktop_height']-1)) < 2
        checks.append('selected_screen_absolute_click')
        normal_a(child)
        send(child, 4, value=0x10, down=True); send(child, 4, value=0x42, down=True)
        send(child, 4, value=0x42); send(child, 4, value=0x10)
        assert count(Gdk.EventType.KEY_PRESS, Gdk.KEY_B) == 1
        send(child, 4, value=0x11, down=True); send(child, 4, value=0x43, down=True)
        send(child, 4, value=0x43); send(child, 4, value=0x11)
        assert any(e[0] == Gdk.EventType.KEY_PRESS and e[1] == Gdk.KEY_c and e[2] & Gdk.ModifierType.CONTROL_MASK for e in events)
        checks.append('text_shift_and_control_chord')
        events.clear(); send(child, 2, .2, .2, down=True); send(child, 1, .65, .6); send(child, 2, .65, .6)
        assert any(e[0] == Gdk.EventType.MOTION_NOTIFY and e[3] & Gdk.ModifierType.BUTTON1_MASK and e[1] > .6*screen['desktop_width'] for e in events)
        checks.append('drag_with_button_held')
        events.clear(); send(child, 3, value=120); send(child, 3, value=-240)
        wheel = [e[1] for e in events if e[0] == Gdk.EventType.SCROLL]
        assert any(v < 0 for v in wheel) and any(v > 0 for v in wheel)
        checks.append('wheel_both_directions')
        send(child, 4, value=0x10, down=True); send(child, 2, .5, .5, down=True)
        send(child, 5); normal_a(child); checks.append('explicit_release_all')
        send(child, 4, value=0x11, down=True); send(child, 2, .5, .5, down=True)
        child.stdin.write(b'\x04\x00'); child.stdin.close(); wait(child)
        child = spawn(); normal_a(child); checks.append('partial_packet_eof_releases_state')
        send(child, 4, value=0x10, down=True); send(child, 2, .5, .5, down=True)
        child.kill(); wait(child, False)
        child = spawn(); normal_a(child); checks.append('worker_death_releases_state')
        if args.uinput:
            send(child, 4, value=0x10, down=True); send(child, 2, .5, .5, down=True)
            data = pathlib.Path(os.environ['PAB_DATA_DIR'])
            (data/'root-crash-request').write_text('terminate isolated root input child')
            wait(child, False)
            assert (data/'root-crash-done').exists(), 'Root crash was not performed by the isolated supervisor'
            child = spawn(); normal_a(child); checks.append('root_worker_sigkill_releases_state')
        send(child, 4, value=0x11, down=True); child.stdin.write(bytes([99])+bytes(7)); wait(child, False)
        assert child.returncode != 0
        child = spawn(); normal_a(child); child.stdin.close(); wait(child)
        checks.append('malformed_packet_releases_state')
        child = spawn(offset=1); wait(child, False); assert child.returncode != 0
        checks.append('stale_geometry_rejected')
        bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
        def display_call(method, args=None):
            return bus.call_sync('org.gnome.Mutter.DisplayConfig', '/org/gnome/Mutter/DisplayConfig',
                'org.gnome.Mutter.DisplayConfig', method, args, None, Gio.DBusCallFlags.NONE, 5000, None).unpack()
        before = display_call('GetCurrentState')
        assert len(before[1]) == len(before[2]) == 1
        spec, modes, props = before[1][0]
        original = next(m[0] for m in modes if m[6].get('is-current'))
        target = next(m[0] for m in modes if m[0] != original and m[1] == 1920 and m[2] == 1080 and before[2][0][2] in m[5])
        def mode(value):
            options = {}
            if 'is-underscanning' in props: options['enable_underscanning'] = GLib.Variant('b', props['is-underscanning'])
            global_options = {}
            if before[3].get('supports-changing-layout-mode'):
                global_options['layout-mode'] = GLib.Variant('u', before[3]['layout-mode'])
            config = [tuple(before[2][0][:5]) + ([(spec[0], value, options)],)]
            display_call('ApplyMonitorsConfig', GLib.Variant('(uua(iiduba(ssa{sv}))a{sv})',
                (display_call('GetCurrentState')[0], 1, config, global_options)))
        child = spawn(); send(child, 4, value=0x11, down=True); send(child, 2, .5, .5, down=True)
        try:
            mode(target); wait(child, False)
            assert child.returncode != 0, 'Idle old input survived a layout change'
        finally:
            mode(original); pump(.5)
            restored = display_call('GetCurrentState')
            assert restored[2] == before[2]
            assert [m[0] for m in restored[1][0][1] if m[6].get('is-current')] == [original]
        child = spawn(); normal_a(child); child.stdin.close(); wait(child)
        checks.append('idle_layout_change_releases_state_and_reopens')
        print(json.dumps({'passed':True,'checks':checks,'worker_processes_reaped':len(children),
                          'screen':screen['id'],'dimensions':[screen['pixel_width'],screen['pixel_height']],
                          'layout_restored':True,
                          'input_backend':'supervised_uinput' if args.uinput else 'gnome',
                          'input_contents_saved':False,'installed_application_modified':False}), flush=True)
    finally:
        for child in children:
            if child.poll() is None: child.kill()
            child.wait(timeout=5)
        window.destroy(); pump(.2)


if __name__ == '__main__':
    main()
