"""Opt-in XFixes visibility acceptance using the production worker pipe.

Temporarily hides the cursor through owned X clients; disconnect always restores
their hides. Does not change root shapes, background, pointer or installed apps.
Run with --session-display only on an explicitly authorized X11 test desktop.
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

from check_x11_cursor_source import exact, quiet, send


def main():
    assert len(sys.argv) == 3 and sys.argv[1] in ("--isolated-display", "--session-display")
    assert os.environ.get("XDG_SESSION_TYPE") == "x11"
    worker = pathlib.Path(sys.argv[2]).resolve(strict=True)
    x = C.CDLL("libX11.so.6")
    fix = C.CDLL("libXfixes.so.3")
    D, U, I = C.c_void_p, C.c_ulong, C.c_int

    def bind(library, name, result, *arguments):
        function = getattr(library, name)
        function.restype, function.argtypes = result, arguments

    bind(x, "XOpenDisplay", D, C.c_char_p)
    bind(x, "XCloseDisplay", I, D)
    bind(x, "XScreenCount", I, D)
    bind(x, "XDefaultRootWindow", U, D)
    bind(x, "XSync", I, D, I)
    bind(fix, "XFixesQueryVersion", I, D, C.POINTER(I), C.POINTER(I))
    bind(fix, "XFixesHideCursor", I, D, U)
    bind(fix, "XFixesShowCursor", I, D, U)
    clients, processes, rows = [], [], []

    def connect():
        connection = x.XOpenDisplay(None)
        assert connection
        clients.append(connection)
        major, minor = I(5), I()
        assert fix.XFixesQueryVersion(connection, C.byref(major), C.byref(minor))
        return connection

    try:
        a, b = connect(), connect()
        assert x.XScreenCount(a) == 1
        root = x.XDefaultRootWindow(a)
        screens = json.loads(subprocess.check_output([str(worker), "inventory"], timeout=8))
        screen = next(s for s in screens if s["primary"])

        def start():
            process = subprocess.Popen([str(worker), "cursor", screen["id"]],
                                       stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                       stderr=subprocess.PIPE, bufsize=0)
            processes.append(process)
            send(process, struct.pack("<iiIII", screen["x"], screen["y"],
                                      screen["desktop_width"], screen["desktop_height"], 0))
            assert struct.unpack("<II", exact(process.stdout, 8)) == (0x31424150, 1)
            return process

        def read(process, name, visible, request=True):
            if request:
                send(process, b"\1")
            tag, version, sequence, value, px, py, image_id, changed = struct.unpack(
                "<BHQBiiQB", exact(process.stdout, 29))
            assert tag == version == 1 and value in (0, 1) and bool(value) == visible
            shape_bytes = 0
            if changed:
                width, height, hx, hy = struct.unpack("<IIII", exact(process.stdout, 16))
                assert 0 < width <= 512 and 0 < height <= 512 and hx < width and hy < height
                shape_bytes = width * height * 4
                exact(process.stdout, shape_bytes)
            row = dict(case=name, visible=bool(value), sequence=sequence, x=px, y=py,
                       image_id=image_id, shape_bytes=shape_bytes)
            rows.append(row)
            return row

        def hide(connection):
            fix.XFixesHideCursor(connection, root)
            x.XSync(connection, False)

        def show(connection):
            fix.XFixesShowCursor(connection, root)
            x.XSync(connection, False)

        process = start()
        initial = read(process, "initial", True)
        assert initial["shape_bytes"]
        hide(a)
        hidden = read(process, "system_hide", False)
        assert hidden["image_id"] == initial["image_id"] and not hidden["shape_bytes"]
        late = start()
        read(late, "startup_already_hidden", False)
        send(late, b"\0")
        assert late.wait(timeout=5) == 0
        hide(a)
        send(process, b"\1")
        quiet(process.stdout)
        show(a)
        quiet(process.stdout)
        hide(b)
        show(a)
        quiet(process.stdout)
        x.XCloseDisplay(b)
        clients.remove(b)
        restored = read(process, "last_hiding_client_exit", True, request=False)
        assert restored["image_id"] == initial["image_id"] and not restored["shape_bytes"]
        # An outstanding update owns no motion history and EOF still exits.
        send(process, b"\1")
        quiet(process.stdout)
        begin = time.monotonic()
        process.stdin.close()
        exit_code = process.wait(timeout=5)
        diagnostics = process.stderr.read().decode(errors="replace").strip()
        # Native parent_alive reports EOF through the worker's error exit;
        # it is not the graceful explicit stop command exercised above.
        assert exit_code == 0 or (exit_code == 1 and "Parent pipe ended" in diagnostics), diagnostics
        eof_ms = (time.monotonic() - begin) * 1000
        print(json.dumps(dict(worker_sha256=hashlib.sha256(worker.read_bytes()).hexdigest(),
                              display=screen, updates=rows, nested_hide_kept_hidden=True,
                              second_client_kept_hidden=True, shape_reused=True,
                              eof_exit_ms=round(eof_ms, 3), eof_exit_code=exit_code,
                              eof_diagnostics=diagnostics, passed=True), indent=2))
    finally:
        for connection in clients:
            x.XCloseDisplay(connection)
        for process in processes:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)


if __name__ == "__main__":
    main()
