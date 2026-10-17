"""Opt-in GNOME lock/wake/login check through the normal native entry points.

Requires a caller-created recovery timer, private observation directory and
credential file. Only pre-login lock/prompt luminance is saved, never typed
contents. The caller independently compares those frames with the VM console
before allowing one password submission. No installed service is replaced.
"""
import argparse
import ctypes
import json
import os
import pathlib
import select
import struct
import subprocess
import time

import gi
gi.require_version('Gtk', '3.0')
from gi.repository import Gio, GLib, Gtk


class Bytes(ctypes.Structure):
    _fields_ = [('data', ctypes.POINTER(ctypes.c_ubyte)), ('size', ctypes.c_size_t)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('worker', type=pathlib.Path)
    parser.add_argument('--embedded', action='store_true')
    parser.add_argument('--uinput', action='store_true')  # Shared isolated wrapper.
    args = parser.parse_args()
    worker = args.worker.resolve(strict=True)
    assert worker.parent.parent == pathlib.Path('/tmp') and worker.parent.name.startswith('pab-uinput-')
    stage = worker.parent
    observation = stage / 'observation'
    recovery = json.loads((stage / 'recovery.json').read_text())
    assert subprocess.check_output(['systemctl', 'is-active', recovery['unit'] + '.timer'], text=True).strip() == 'active'
    session = recovery['session']
    command = [str(worker)] + (['--media-worker'] if args.embedded else [])
    screens = json.loads(subprocess.check_output(command + ['inventory'], timeout=8))
    assert len(screens) == 1
    screen = screens[0]
    width, height = screen['pixel_width'], screen['pixel_height']
    bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    children = []
    report = {'passed': False, 'normal_entry_points': True, 'login_submissions': 0}
    def locked():
        return subprocess.check_output(['loginctl', 'show-session', session, '-p', 'LockedHint', '--value'], text=True, timeout=3).strip() == 'yes'
    def call(name, path, interface, method, params=None):
        return bus.call_sync(name, path, interface, method, params, None, Gio.DBusCallFlags.NONE, 3000, None).unpack()
    def power():
        return call('org.gnome.Mutter.DisplayConfig', '/org/gnome/Mutter/DisplayConfig',
                    'org.freedesktop.DBus.Properties', 'Get',
                    GLib.Variant('(ss)', ('org.gnome.Mutter.DisplayConfig', 'PowerSaveMode')))[0]
    def pump(seconds=.02):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            for _ in range(8):
                if not GLib.MainContext.default().pending(): break
                GLib.MainContext.default().iteration(False)
            time.sleep(.002)
    def until(condition, seconds, message):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            if condition(): return
            pump()
        raise AssertionError(message)
    def read(child, size):
        result = bytearray()
        end = time.monotonic() + 8
        while len(result) < size:
            pump(.005)
            assert time.monotonic() < end, 'Native response timeout'
            if select.select([child.stdout], [], [], .01)[0]:
                chunk = os.read(child.stdout.fileno(), size - len(result))
                assert chunk, 'Native response ended: ' + child.stderr.read(1200).decode(errors='replace')
                result.extend(chunk)
        return bytes(result)
    def spawn(operation):
        argv = command + [operation, screen['id']]
        if operation == 'capture': argv += ['15', 'desktop']
        child = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, bufsize=0)
        children.append(child)
        if operation == 'input':
            child.stdin.write(struct.pack('<iiII', screen['x'], screen['y'], screen['desktop_width'], screen['desktop_height']))
            assert read(child, 4) == struct.pack('<I', 0x31424150)
        else:
            assert read(child, 12) == struct.pack('<III', 0x31424150, width, height)
        return child
    def key(child, vk, down):
        child.stdin.write(struct.pack('<BHHHB', 4, 0, 0, vk, down))
        assert read(child, 1) == b'\x01'
        pump(.025)
    library = ctypes.CDLL(str(stage / 'codec.so'))
    library.pab_soft_decoder_new.argtypes = [ctypes.c_uint32, ctypes.c_uint32]
    library.pab_soft_decoder_new.restype = ctypes.c_void_p
    library.pab_soft_decoder_drop.argtypes = [ctypes.c_void_p]
    library.pab_soft_decode.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_size_t, ctypes.POINTER(Bytes)]
    library.pab_soft_decode.restype = ctypes.c_bool
    decoder = library.pab_soft_decoder_new(width, height)
    assert decoder
    def frame(child):
        child.stdin.write(b'\x01')
        size, sequence, captured, keyframe = struct.unpack('<IQQI', read(child, 24))
        assert 0 < size <= 16 * 1024 * 1024 and keyframe in (0, 1)
        packet = read(child, size)
        pixels = Bytes()
        assert library.pab_soft_decode(decoder, packet, len(packet), ctypes.byref(pixels))
        assert pixels.size == width * height * 3 // 2
        return ctypes.string_at(pixels.data, width * height)
    def observe(child, name):
        assert locked() and power() == 0
        for _ in range(6): luminance = frame(child)
        assert max(luminance) - min(luminance) > 60, 'Lock frame is blank'
        (observation / (name + '.pgm')).write_bytes(f'P5\n{width} {height}\n255\n'.encode() + luminance)
        (observation / 'phase.json').write_text(json.dumps({'phase': name, 'locked': True, 'power': 0}))
        until(lambda: (observation / (name + '.verified')).exists(), 22, 'Independent console verification missing')
    # A generated fullscreen fixture makes stale pre-lock scanout distinguishable.
    window = Gtk.Window(title='Pixels authorized lock/login check')
    def draw(widget, cr):
        cr.set_source_rgb(.04, .65, .14); cr.paint()
        cr.set_source_rgb(1, 1, 1); cr.set_font_size(32); cr.move_to(40, 90)
        cr.show_text('Pixels lock/login verification fixture')
        return True
    window.connect('draw', draw); window.show_all(); window.fullscreen(); window.present()
    try:
        assert not locked()
        pump(.5)
        call('org.gnome.ScreenSaver', '/org/gnome/ScreenSaver', 'org.gnome.ScreenSaver', 'Lock')
        until(lambda: locked() and power() in (1, 2, 3), 10, 'Session did not lock and power off')
        report['locked_power'] = power()
        capture = spawn('capture')  # Automatic wake + DMA, no explicit DMA switch.
        observe(capture, 'lock')
        report['automatic_wake'] = power() == 0 and locked()
        input_child = spawn('input')  # Automatic locked-session input backend.
        key(input_child, 0x0D, True); key(input_child, 0x0D, False)
        pump(.5)
        observe(capture, 'prompt')
        # One submission only. The credential never enters argv, environment,
        # reports, screenshots or assertion messages.
        credential = (observation / 'credential').read_text()
        (observation / 'credential').unlink()
        shifted = '~!@#$%^&*()_+{}|:"<>?'
        plain = '`1234567890-=[]\\;\',./'
        punctuation = {'`':0xC0, '-':0xBD, '=':0xBB, '[':0xDB, ']':0xDD,
                       '\\':0xDC, ';':0xBA, "'":0xDE, ',':0xBC, '.':0xBE, '/':0xBF, ' ':0x20}
        keys = []
        for char in credential:
            shift = char in shifted or ('A' <= char <= 'Z')
            base = plain[shifted.index(char)] if char in shifted else char.lower()
            if 'a' <= base <= 'z': vk = ord(base.upper())
            elif '0' <= base <= '9': vk = ord(base)
            else:
                assert base in punctuation, 'Login test needs a supported keyboard layout'
                vk = punctuation[base]
            keys.append((vk, shift))
        del credential
        assert keys and locked()
        (observation / 'phase.json').write_text(json.dumps({'phase': 'submitting'}))
        for vk, shift in keys:
            if shift: key(input_child, 0x10, True)
            key(input_child, vk, True); key(input_child, vk, False)
            if shift: key(input_child, 0x10, False)
        del keys
        report['login_submissions'] = 1
        key(input_child, 0x0D, True)
        try: key(input_child, 0x0D, False)
        except (AssertionError, BrokenPipeError):
            # Unlock retires this generation; do not replay an uncertain input.
            pass
        until(lambda: not locked(), 10, 'Single login submission did not unlock')
        report['password_login_unlocked'] = True
        until(lambda: input_child.poll() is not None, 5, 'Old input survived unlock')
        capture.stdin.write(b'\x01')
        tail = capture.communicate(timeout=8)
        assert tail[0] == b'', 'Old locked generation emitted data after unlock'
        assert b'source=drm_dma' in tail[1], 'Automatic lock capture did not use supervised DMA'
        report['locked_capture_source'] = tail[1].decode(errors='replace')
        report['old_generation_retired'] = True
        fresh = spawn('capture')
        assert max(frame(fresh)) - min(frame(fresh)) > 60
        fresh.stdin.write(b'\x00'); fresh.wait(timeout=5)
        diagnostic = fresh.stderr.read().decode(errors='replace')
        assert fresh.returncode == 0 and 'source=gnome_pipewire' in diagnostic
        report['unlocked_capture_source'] = diagnostic
        report['passed'] = True
    finally:
        for child in children:
            if child.poll() is None: child.kill()
            child.wait(timeout=5)
        library.pab_soft_decoder_drop(decoder)
        window.destroy(); pump(.1)
        report['final_locked'] = locked()
        report['input_contents_saved'] = False
        print(json.dumps(report), flush=True)


if __name__ == '__main__':
    main()
