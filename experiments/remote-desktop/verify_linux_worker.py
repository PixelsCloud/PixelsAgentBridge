"""Check the production capture pipe and decoder without retaining screen pixels.

Uses an explicitly opened GTK animation window. Worker/decoder are isolated test
artifacts; no installed application, configuration or account is replaced.
"""
import ctypes
import argparse
import json
import os
import pathlib
import select
import struct
import subprocess
import sys
import time

import gi
gi.require_version("Gtk", "3.0")
from gi.repository import Gtk, GLib


class Bytes(ctypes.Structure):
    _fields_ = [("data", ctypes.POINTER(ctypes.c_ubyte)), ("size", ctypes.c_size_t)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("worker", type=pathlib.Path)
    parser.add_argument("codec_library", type=pathlib.Path)
    parser.add_argument("--embedded", action="store_true")
    parser.add_argument("--dma-fd", type=int)
    args = parser.parse_args()
    assert args.dma_fd is None or (args.dma_fd >= 3 and os.geteuid() != 0)
    worker = args.worker.resolve(strict=True)
    command = [str(worker)] + (["--media-worker"] if args.embedded else [])
    library = ctypes.CDLL(str(args.codec_library.resolve(strict=True)))
    library.pab_soft_decoder_new.argtypes = [ctypes.c_uint32, ctypes.c_uint32]
    library.pab_soft_decoder_new.restype = ctypes.c_void_p
    library.pab_soft_decoder_drop.argtypes = [ctypes.c_void_p]
    library.pab_soft_decode.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_size_t, ctypes.POINTER(Bytes)]
    library.pab_soft_decode.restype = ctypes.c_bool
    inventory = subprocess.run(command + ["inventory"], capture_output=True, text=True, timeout=8, check=True)
    screens = json.loads(inventory.stdout)
    screen = next(s for s in screens if s["primary"])
    width, height = screen["pixel_width"], screen["pixel_height"]
    decoder = library.pab_soft_decoder_new(width, height)
    assert decoder, "Decoder creation failed"
    window = Gtk.Window(title="Pixels bounded native worker test")
    window.set_default_size(800, 450)
    tick = [0]
    def draw(widget, cr):
        w, h = widget.get_allocated_width(), widget.get_allocated_height()
        for q in range(4):
            cr.set_source_rgb(*[(32 + (tick[0] * speed + q * offset) % 192) / 255 for speed, offset in [(3,43), (7,67), (11,97)]])
            cr.rectangle((q % 2) * w / 2, (q // 2) * h / 2, w / 2, h / 2)
            cr.fill()
        return True
    window.connect("draw", draw)
    window.show_all()
    window.fullscreen()
    def animate():
        tick[0] += 1
        window.queue_draw()
        return True
    timer = GLib.timeout_add(33, animate)
    context = GLib.MainContext.default()
    def pump():
        for _ in range(8):
            if not context.pending():
                break
            context.iteration(False)
    capture = ["capture-dma" if args.dma_fd is not None else "capture", screen["id"], "15", "desktop"]
    if args.dma_fd is not None:
        capture.append(str(args.dma_fd))
    source = subprocess.Popen(command + capture, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        stderr=subprocess.PIPE, bufsize=0, pass_fds=() if args.dma_fd is None else (args.dma_fd,))
    if args.dma_fd is not None:
        os.close(args.dma_fd)  # Only the capture child retains the channel.
    total_bytes = 0
    samples = []
    times = []
    previous = -1
    deadline = time.monotonic() + 25
    def read(size):
        data = bytearray()
        while len(data) < size:
            pump()
            if time.monotonic() > deadline:
                raise TimeoutError("Native capture pipe timed out")
            ready, _, _ = select.select([source.stdout], [], [], .002)
            if ready:
                chunk = os.read(source.stdout.fileno(), size - len(data))
                if not chunk:
                    raise RuntimeError("Capture pipe closed: " + source.stderr.read(2000).decode(errors="replace"))
                data.extend(chunk)
        return bytes(data)
    try:
        assert struct.unpack("<III", read(12)) == (0x31424150, width, height)
        for i in range(90):
            source.stdin.write(b"\x01")
            size, index, captured, key = struct.unpack("<IQQI", read(24))
            assert 0 < size <= 16 * 1024 * 1024 and index == i
            assert key in (0,1) and (i or key == 1) and captured >= previous
            previous = captured
            packet = read(size)
            pixels = Bytes()
            assert library.pab_soft_decode(decoder, packet, len(packet), ctypes.byref(pixels))
            assert pixels.size == width * height * 3 // 2
            samples.append(tuple(pixels.data[y * width + x] for y in (height//4, height*3//4) for x in (width//4, width*3//4)))
            times.append(time.monotonic())
            total_bytes += size
        source.stdin.write(b"\x00")
        assert source.wait(timeout=5) == 0
        stderr = source.stderr.read(4000).decode(errors="replace")
        changed = sum(a != b for a,b in zip(samples, samples[1:]))
        # Exclude a one-second warmup and count actual decoded changing frames.
        elapsed = times[-1] - times[15]
        print(json.dumps({"dimensions":[width,height], "inventory":screens, "frames":len(samples),
            "changed_samples":changed, "decoded_fps_after_warmup":74/elapsed,
            "changed_fps_after_warmup":sum(a != b for a,b in zip(samples[15:],samples[16:]))/elapsed,
            "encoded_bytes":total_bytes, "fixture_ticks":tick[0], "stderr":stderr,
            "pixel_bytes_saved":0, "worker_exit":source.returncode, "consumer_uid":os.geteuid(),
            "source":"dma" if args.dma_fd is not None else "pipewire"}))
        assert changed >= 85, "Animation did not produce distinct desktop frames"
    finally:
        if source.poll() is None:
            source.kill()
        source.wait(timeout=5)
        GLib.source_remove(timer)
        window.destroy()
        library.pab_soft_decoder_drop(decoder)


if __name__ == "__main__":
    main()
