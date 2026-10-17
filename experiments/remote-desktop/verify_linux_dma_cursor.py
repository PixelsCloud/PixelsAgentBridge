"""Opt-in supervised capture/cursor check, without saving screen or cursor pixels.

Use as VERIFY_SCRIPT of linux_dma_broker_check in an isolated runtime. The caller
arranges any authorized lock and restoration. This driver never locks or sends
input, and deliberately uses the normal capture entry rather than capture-dma.
"""
import argparse
import ctypes
import json
import os
import pathlib
import select
import socket
import struct
import subprocess
import tempfile
import time


class Bytes(ctypes.Structure):
    _fields_ = [("data", ctypes.POINTER(ctypes.c_ubyte)), ("size", ctypes.c_size_t)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("worker", type=pathlib.Path)
    parser.add_argument("codec_library", type=pathlib.Path)
    parser.add_argument("--embedded", action="store_true")
    parser.add_argument("--dma-broker", action="store_true")
    args = parser.parse_args()
    assert os.geteuid() != 0
    command = [str(args.worker.resolve(strict=True))] + (["--media-worker"] if args.embedded else [])
    screens = json.loads(subprocess.check_output(command + ["inventory"], timeout=8))
    screen = next(s for s in screens if s["primary"])
    assert screen["id"].startswith("gnome:") and screen["rotation"] == "identity"
    width, height = screen["pixel_width"], screen["pixel_height"]
    library = ctypes.CDLL(str(args.codec_library.resolve(strict=True)))
    library.pab_soft_decoder_new.argtypes = [ctypes.c_uint32, ctypes.c_uint32]
    library.pab_soft_decoder_new.restype = ctypes.c_void_p
    library.pab_soft_decoder_drop.argtypes = [ctypes.c_void_p]
    library.pab_soft_decode.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_size_t, ctypes.POINTER(Bytes)]
    library.pab_soft_decode.restype = ctypes.c_bool
    decoder = library.pab_soft_decoder_new(width, height)
    assert decoder
    # PipeWire intentionally sends no duplicate video for an unchanged screen.
    # Animate our own small window only in an unlocked session; never mistake
    # that normal idle behavior for a transport stall.
    import gi
    gi.require_version("Gtk", "3.0")
    from gi.repository import Gtk, GLib
    context = GLib.MainContext.default()
    window = timer = None
    lock_state = subprocess.check_output(["gdbus", "call", "--session", "--dest", "org.gnome.ScreenSaver",
        "--object-path", "/org/gnome/ScreenSaver", "--method", "org.gnome.ScreenSaver.GetActive"], text=True, timeout=5)
    if lock_state.strip() == "(false,)":
        window = Gtk.Window(title="Pixels DMA cursor stream check")
        window.set_default_size(320, 180)
        tick = [0]
        def draw(widget, cr):
            cr.set_source_rgb((tick[0] % 17) / 16, .2, .7)
            cr.paint()
            return True
        window.connect("draw", draw)
        window.show_all()
        def animate():
            tick[0] += 1
            window.queue_draw()
            return True
        timer = GLib.timeout_add(50, animate)

    def read(pipe, size, timeout=6):
        data = bytearray()
        deadline = time.monotonic() + timeout
        while len(data) < size:
            for _ in range(8):
                if not context.pending():
                    break
                context.iteration(False)
            assert time.monotonic() < deadline, "capture/cursor response timed out"
            if not select.select([pipe], [], [], .02)[0]:
                continue
            part = os.read(pipe.fileno(), size - len(data))
            assert part, "capture/cursor closed before response"
            data.extend(part)
        return bytes(data)

    source = peer = None
    try:
        with tempfile.TemporaryDirectory(prefix="pab-dma-cursor-", dir=os.environ["XDG_RUNTIME_DIR"]) as temporary:
            endpoint = pathlib.Path(temporary) / "cursor.sock"
            source = subprocess.Popen(command + ["capture", screen["id"], "15", "desktop-separate-cursor", str(endpoint)],
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, bufsize=0)
            try:
                assert struct.unpack("<III", read(source.stdout, 12)) == (0x31424150, width, height)

                def connect():
                    connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                    connection.settimeout(6)
                    connection.connect(str(endpoint))
                    connection.sendall(struct.pack("<iiIII", screen["x"], screen["y"], screen["desktop_width"], screen["desktop_height"], 0))
                    assert struct.unpack("<II", read(connection, 8)) == (0x31424150, 1)
                    return connection

                def cursor(connection):
                    connection.sendall(b"\x01")
                    tag, version, sequence, visible, x, y, image_id, image = struct.unpack("<BHQBiiQB", read(connection, 29))
                    assert (tag, version, sequence) == (1, 1, 1) and visible in (0, 1) and image in (0, 1)
                    shape = None
                    if image:
                        w, h, hx, hy = struct.unpack("<IIII", read(connection, 16))
                        assert 0 < w <= 512 and 0 < h <= 512 and hx < w and hy < h and image_id
                        pixels = read(connection, w * h * 4)
                        assert all(max(pixels[i:i+3]) <= pixels[i+3] for i in range(0, len(pixels), 4))
                        shape = [w, h, hx, hy]
                    assert not visible or shape
                    return dict(visible=bool(visible), position=[x, y], image_id=image_id, shape=shape)

                peer = connect()
                initial = cursor(peer)  # No encoded frame has been requested.
                assert not select.select([source.stdout], [], [], .1)[0]
                frame_count = 0

                def video():
                    nonlocal frame_count
                    source.stdin.write(b"\x01")
                    size, sequence, _, key = struct.unpack("<IQQI", read(source.stdout, 24))
                    assert 0 < size <= 16 * 1024 * 1024 and sequence == frame_count and (frame_count or key == 1)
                    packet = read(source.stdout, size)
                    pixels = Bytes()
                    assert library.pab_soft_decode(decoder, packet, len(packet), ctypes.byref(pixels))
                    assert pixels.size == width * height * 3 // 2
                    frame_count += 1

                for _ in range(3):
                    video()
                # A malformed cursor client is retired, with video kept alive.
                peer.sendall(b"\x02")
                assert peer.recv(1) == b""
                peer.close(); peer = None
                video()
                peer = connect()
                recovered = cursor(peer)
                peer.close(); peer = None
                # Cursor production continues while the consumer requests no
                # video and no cursor updates. Reconnect reads latest state.
                fd_before = len(list(pathlib.Path(f"/proc/{source.pid}/fd").iterdir()))
                time.sleep(1)
                peer = connect()
                latest = cursor(peer)
                video()
                peer.close(); peer = None
                time.sleep(.05)
                fd_after = len(list(pathlib.Path(f"/proc/{source.pid}/fd").iterdir()))
                assert fd_after <= fd_before, (fd_before, fd_after)
                started = time.monotonic()
                source.stdin.write(b"\x00")
                assert source.wait(timeout=5) == 0
                elapsed = time.monotonic() - started
                stderr = source.stderr.read().decode(errors="replace")
                assert not endpoint.exists()
                print(json.dumps(dict(screen=screen, initial_before_video=initial, recovered=recovered, latest=latest,
                    decoded_frames=frame_count, fd_before=fd_before, fd_after=fd_after,
                    stop_seconds=elapsed, cursor_socket_removed=True, worker_exit=source.returncode,
                    stderr=stderr, pixel_bytes_saved=0)), flush=True)
            finally:
                if peer:
                    peer.close()
                if source.poll() is None:
                    source.kill()
                source.wait(timeout=5)
                if source.returncode:
                    print(source.stderr.read().decode(errors="replace"), flush=True)
                # SIGKILL cannot run native destructors; this directory belongs
                # only to this test, and TemporaryDirectory removes it.
    finally:
        if timer is not None:
            GLib.source_remove(timer)
        if window is not None:
            window.destroy()
        library.pab_soft_decoder_drop(decoder)


if __name__ == "__main__":
    main()
