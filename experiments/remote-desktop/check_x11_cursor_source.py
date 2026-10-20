"""Opt-in X11 worker acceptance on a DISPOSABLE two-screen X server.

Changes root cursor/background and pointer. Never run on a user's display.
Requires libX11, libXfixes and ffmpeg. No installed Pixels service is changed.
Usage: DISPLAY=:91 XDG_SESSION_TYPE=x11 python3 check_x11_cursor_source.py
       --isolated-display WORKER [--media-worker]
"""
import ctypes as C
import hashlib
import json
import os
import pathlib
import select
import struct
import subprocess
import sys
import time


def exact(stream, count, timeout=5):
    deadline = time.monotonic() + timeout
    result = bytearray()
    while len(result) < count:
        remaining = deadline - time.monotonic()
        if remaining <= 0 or not select.select([stream], [], [], remaining)[0]:
            raise TimeoutError(f"worker output {len(result)}/{count}")
        part = os.read(stream.fileno(), count - len(result))
        if not part:
            raise RuntimeError("worker pipe ended")
        result.extend(part)
    return bytes(result)


def quiet(stream, seconds=0.15):
    assert not select.select([stream], [], [], seconds)[0], "unsolicited or stale output"


def send(process, data):
    process.stdin.write(data)
    process.stdin.flush()


def update(process):
    send(process, b"\x01")
    tag, version, sequence, visible, x, y, image_id, changed = struct.unpack(
        "<BHQBiiQB", exact(process.stdout, 29))
    assert tag == version == 1 and visible in (0, 1) and changed in (0, 1)
    shape, pixels = None, b""
    if changed:
        shape = struct.unpack("<IIII", exact(process.stdout, 16))
        width, height, hx, hy = shape
        assert 0 < width <= 512 and 0 < height <= 512 and hx < width and hy < height
        pixels = exact(process.stdout, width * height * 4)
    return dict(sequence=sequence, visible=bool(visible), x=x, y=y, image_id=image_id,
                shape=shape, shape_bytes=len(pixels), shape_sha256=hashlib.sha256(pixels).hexdigest() if pixels else None)


def main():
    assert len(sys.argv) >= 3 and sys.argv[1] == "--isolated-display"
    assert os.environ.get("XDG_SESSION_TYPE") == "x11"
    command = sys.argv[2:]
    x = C.CDLL("libX11.so.6")
    fix = C.CDLL("libXfixes.so.3")
    D, U, I = C.c_void_p, C.c_ulong, C.c_int

    class Color(C.Structure):
        _fields_ = [("pixel", U), ("red", C.c_ushort), ("green", C.c_ushort),
                    ("blue", C.c_ushort), ("flags", C.c_char), ("pad", C.c_char)]

    def bind(lib, name, result, *args):
        function = getattr(lib, name)
        function.restype, function.argtypes = result, args
        return function

    bind(x, "XOpenDisplay", D, C.c_char_p)
    bind(x, "XCloseDisplay", I, D)
    bind(x, "XScreenCount", I, D)
    bind(x, "XRootWindow", U, D, I)
    bind(x, "XCreateFontCursor", U, D, C.c_uint)
    bind(x, "XDefineCursor", I, D, U, U)
    bind(x, "XUndefineCursor", I, D, U)
    bind(x, "XFreeCursor", I, D, U)
    bind(x, "XFreePixmap", I, D, U)
    bind(x, "XCreateBitmapFromData", U, D, U, C.c_char_p, C.c_uint, C.c_uint)
    bind(x, "XCreatePixmapCursor", U, D, U, U, C.POINTER(Color), C.POINTER(Color), C.c_uint, C.c_uint)
    bind(x, "XWarpPointer", I, D, U, U, I, I, C.c_uint, C.c_uint, I, I)
    bind(x, "XSetWindowBackground", I, D, U, U)
    bind(x, "XClearWindow", I, D, U)
    bind(x, "XSync", I, D, I)
    bind(fix, "XFixesHideCursor", I, D, U)
    bind(fix, "XFixesShowCursor", I, D, U)
    d = x.XOpenDisplay(None)
    assert d and x.XScreenCount(d) == 2, "requires the dedicated two-screen server"
    root, other = x.XRootWindow(d, 0), x.XRootWindow(d, 1)
    arrow, cross = x.XCreateFontCursor(d, 68), x.XCreateFontCursor(d, 34)
    bitmap = x.XCreateBitmapFromData(d, root, b"\0", 1, 1)
    black = Color()
    transparent = x.XCreatePixmapCursor(d, bitmap, bitmap, C.byref(black), C.byref(black), 0, 0)
    children = []
    results = {}
    compressed_streams = {}

    def sync():
        x.XSync(d, 0)
        time.sleep(0.06)

    def cursor(shape):
        x.XDefineCursor(d, root, shape)
        sync()

    def move(px, py, destination=root):
        x.XWarpPointer(d, 0, destination, 0, 0, 0, 0, px, py)
        sync()

    def start(*args):
        p = subprocess.Popen(command + list(args), stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, bufsize=0)
        children.append(p)
        return p

    def stop(p):
        send(p, b"\0")
        assert p.wait(timeout=5) == 0, p.stderr.read().decode()

    def start_cursor(screen, stale=False):
        p = start("cursor", screen["id"])
        send(p, struct.pack("<iiIII", screen["x"] + int(stale), screen["y"],
                            screen["desktop_width"], screen["desktop_height"], 0))
        if stale:
            assert p.wait(timeout=5) != 0 and p.stdout.read() == b""
            return p
        assert struct.unpack("<II", exact(p.stdout, 8)) == (0x31424150, 1)
        return p

    def capture(screen, mode):
        p = start("capture", screen["id"], "15", mode)
        assert struct.unpack("<III", exact(p.stdout, 12)) == (0x31424150, screen["pixel_width"], screen["pixel_height"])
        return p

    def frame(p, expected_index=0, request=True):
        if request:
            send(p, b"\1")
        size, index, timestamp, key = struct.unpack("<IQQI", exact(p.stdout, 24))
        assert 0 < size <= 16 * 1024 * 1024 and index == expected_index and key in (0, 1)
        assert index != 0 or key == 1
        compressed = compressed_streams.setdefault(p, bytearray())
        compressed.extend(exact(p.stdout, size))
        decoded = subprocess.run(["ffmpeg", "-v", "error", "-threads", "1", "-f", "h264", "-i", "pipe:0",
                                  "-threads", "1", "-frames:v", str(index + 1), "-pix_fmt", "bgr24", "-f", "rawvideo", "pipe:1"],
                                 input=compressed, capture_output=True, timeout=10, check=True).stdout
        length = screen["pixel_width"] * screen["pixel_height"] * 3
        assert len(decoded) == length * (index + 1)
        return decoded[-length:]

    try:
        x.XSetWindowBackground(d, root, 0x204060)
        x.XClearWindow(d, root)
        cursor(arrow)
        move(100, 100)
        screens = json.loads(subprocess.check_output(command + ["inventory"], timeout=8))
        screen = next(s for s in screens if s["primary"])
        assert screen["id"].startswith("x11:0:")
        results["screens"] = screens
        start_cursor(screen, stale=True)
        results["stale_geometry_rejected"] = True
        p = start_cursor(screen)
        initial = update(p)
        assert initial["visible"] and initial["x"] == initial["y"] == 100 and initial["shape_bytes"]
        results["initial"] = initial
        for position in (110, 120, 130):
            move(position, position)
        quiet(p.stdout)
        moved = update(p)
        assert moved["sequence"] == 2 and moved["x"] == moved["y"] == 130 and moved["shape_bytes"] == 0
        results["motion_latest_only"] = moved
        cursor(cross)
        changed = update(p)
        assert changed["visible"] and changed["image_id"] > initial["image_id"] and changed["shape_bytes"]
        results["changed"] = changed
        cursor(transparent)
        hidden = update(p)
        assert not hidden["visible"]
        move(140, 140)
        quiet(p.stdout)
        cursor(arrow)
        assert update(p)["visible"]
        move(30, 30, other)
        assert not update(p)["visible"]
        move(150, 150)
        assert update(p)["visible"]
        results["transparent_and_other_screen_restore"] = True
        stop(p)

        # Pixel evidence: no cursor baked into the new video; legacy embedded
        # capture is the positive control on exactly the same static background.
        separate = capture(screen, "desktop-separate-cursor")
        pixels = frame(separate)
        assert len(pixels) == screen["pixel_width"] * screen["pixel_height"] * 3
        expected = (96, 64, 32)
        errors = [abs(value - expected[i % 3]) for i, value in enumerate(pixels)]
        assert max(errors) <= 4, max(errors)
        embedded = capture(screen, "desktop")
        embedded_pixels = frame(embedded)
        mismatches = sum(abs(value - expected[i % 3]) > 20 for i, value in enumerate(embedded_pixels))
        assert mismatches > 20, "positive control must contain the cursor"
        stop(embedded)
        send(separate, b"\1")  # Pending video credit on an unchanged desktop.
        cursor_reader = start_cursor(screen)
        update(cursor_reader)
        move(170, 170)
        assert update(cursor_reader)["x"] == 170
        cursor(cross)
        assert update(cursor_reader)["shape_bytes"]
        quiet(separate.stdout, 0.3)
        results["video"] = dict(no_cursor_max_channel_error=max(errors), compared_channels=len(pixels),
                                embedded_positive_control_channels=mismatches, motion_generated_no_video=True)
        # A malformed cursor operation must not kill the capture process.
        send(cursor_reader, b"\x7f")
        assert cursor_reader.wait(timeout=5) != 0 and separate.poll() is None
        x.XSetWindowBackground(d, root, 0x605040)
        x.XClearWindow(d, root)
        sync()
        recovered = frame(separate, expected_index=1, request=False)
        after_error = max(abs(value - (64, 80, 96)[i % 3]) for i, value in enumerate(recovered))
        assert after_error <= 4, after_error
        stop(separate)
        results["cursor_failure_video_continued"] = dict(frame_index=1, max_channel_error=after_error)
        eof = start_cursor(screen)
        update(eof)
        began = time.monotonic()
        eof.stdin.close()
        assert eof.wait(timeout=5) != 0
        results["parent_eof_ms"] = round((time.monotonic() - began) * 1000, 2)
        results["worker_sha256"] = hashlib.sha256(pathlib.Path(command[0]).read_bytes()).hexdigest()
    finally:
        for p in children:
            if p.poll() is None:
                p.kill()
                p.wait(timeout=5)
            for stream in (p.stdin, p.stdout, p.stderr):
                if not stream.closed:
                    stream.close()
        x.XUndefineCursor(d, root)
        for shape in (arrow, cross, transparent):
            x.XFreeCursor(d, shape)
        x.XFreePixmap(d, bitmap)
        x.XCloseDisplay(d)
    results["children_reaped"] = all(p.poll() is not None for p in children)
    print(json.dumps(results, indent=2))


if __name__ == "__main__":
    main()
