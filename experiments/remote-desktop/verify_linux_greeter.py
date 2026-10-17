"""Opt-in capture check inside a supervised GNOME Wayland greeter.

Use linux_dma_broker_check with an independently armed caller recovery timer.
Does not type a password, create windows or save screen pixels. A nonblank frame
alone does not prove correct login UI; formal-window/console review is separate.
"""
import argparse
import ctypes as C
import json
import os
import select
import struct
import subprocess
import time

from gi.repository import Gio


class Bytes(C.Structure):
    _fields_ = [('data', C.POINTER(C.c_ubyte)), ('size', C.c_size_t)]


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('worker')
    p.add_argument('codec_library')
    p.add_argument('--embedded', action='store_true')
    p.add_argument('--dma-broker', action='store_true')
    args = p.parse_args()
    assert os.geteuid() != 0 and os.environ['XDG_SESSION_CLASS'] == 'greeter'
    assert os.environ['XDG_SESSION_TYPE'] == 'wayland'
    bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)

    def inhibitors():
        def call(path, interface, method):
            return bus.call_sync('org.gnome.SessionManager', path, interface, method,
                                 None, None, Gio.DBusCallFlags.NO_AUTO_START, 3000, None).unpack()
        return [path for path in call('/org/gnome/SessionManager', 'org.gnome.SessionManager', 'GetInhibitors')[0]
                if call(path, 'org.gnome.SessionManager.Inhibitor', 'GetAppId')[0] == 'Pixels Agent Bridge']

    assert not inhibitors()
    command = [args.worker] + (['--media-worker'] if args.embedded else [])
    screens = json.loads(subprocess.check_output(command + ['inventory'], timeout=8))
    screen = next(s for s in screens if s['primary'])
    width, height = screen['pixel_width'], screen['pixel_height']
    lib = C.CDLL(args.codec_library)
    lib.pab_soft_decoder_new.argtypes = [C.c_uint32, C.c_uint32]
    lib.pab_soft_decoder_new.restype = C.c_void_p
    lib.pab_soft_decoder_drop.argtypes = [C.c_void_p]
    lib.pab_soft_decode.argtypes = [C.c_void_p, C.c_void_p, C.c_size_t, C.POINTER(Bytes)]
    lib.pab_soft_decode.restype = C.c_bool
    decoder = lib.pab_soft_decoder_new(width, height)
    assert decoder
    capture = ['capture-dma' if args.dma_broker else 'capture', screen['id'], '15', 'desktop']
    if args.dma_broker: capture.append('broker')
    child = subprocess.Popen(command + capture, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, bufsize=0)
    deadline = time.monotonic() + 15

    def read(size):
        data = bytearray()
        while len(data) < size:
            assert time.monotonic() < deadline, 'Greeter capture timed out'
            if select.select([child.stdout], [], [], .05)[0]:
                part = os.read(child.stdout.fileno(), size - len(data))
                assert part, 'Capture closed: ' + child.stderr.read().decode(errors='replace')
                data.extend(part)
        return bytes(data)

    try:
        assert struct.unpack('<III', read(12)) == (0x31424150, width, height)
        assert len(inhibitors()) == 1
        ranges = []
        for index in range(30):
            child.stdin.write(b'\1')
            size, seq, stamp, key = struct.unpack('<IQQI', read(24))
            assert 0 < size <= 16 * 1024 * 1024 and seq == index
            pixels = Bytes(); packet = read(size)
            assert lib.pab_soft_decode(decoder, packet, size, C.byref(pixels))
            assert pixels.size == width * height * 3 // 2
            values = [pixels.data[y * width + x] for y in range(0, height, 16) for x in range(0, width, 16)]
            ranges.append([min(values), max(values)])
        assert ranges[-1][1] - ranges[-1][0] > 60, 'Blank greeter frame'
        child.stdin.write(b'\0'); assert child.wait(timeout=5) == 0
        diagnostic = child.stderr.read().decode(errors='replace')
        assert 'source=drm_dma' in diagnostic
        end = time.monotonic() + 3
        while inhibitors() and time.monotonic() < end: time.sleep(.05)
        assert not inhibitors(), 'Greeter idle inhibition leaked'
        print(json.dumps({'passed': True, 'inventory': screens, 'frames': len(ranges),
                          'last_luma_range': ranges[-1], 'uid': os.geteuid(),
                          'entry': 'explicit_dma' if args.dma_broker else 'normal_capture',
                          'inhibitor_released': True, 'stderr': diagnostic, 'saved_pixels': 0}))
    finally:
        if child.poll() is None: child.kill()
        child.wait(timeout=5)
        lib.pab_soft_decoder_drop(decoder)


if __name__ == '__main__': main()
