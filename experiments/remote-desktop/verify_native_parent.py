"""Verify only owned native windows: parent embedding, aspect, resize and restore.

Uses synthetic H.264 and the worker's four-pixel GPU checks. No desktop image,
password, input injection, installed process or service is involved.
"""
import argparse
import ctypes
import faulthandler
from ctypes import wintypes
from concurrent.futures import ThreadPoolExecutor
import json
import struct
import subprocess
import time


def main():
    faulthandler.dump_traceback_later(20, repeat=False)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--helper', required=True)
    parser.add_argument('--prefix', action='append', default=[], help='For Desktop use --prefix=--media-worker')
    parser.add_argument('--rotation', choices=['0', '90', '180', '270'], default='0')
    args = parser.parse_args()
    user = ctypes.WinDLL('user32', use_last_error=True)
    user.OpenInputDesktop.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    user.OpenInputDesktop.restype = wintypes.HANDLE
    user.CloseDesktop.argtypes = [wintypes.HANDLE]
    active_desktop = user.OpenInputDesktop(0, False, 1)
    if not active_desktop:
        raise RuntimeError('Native presentation acceptance needs an unlocked interactive desktop '
                           f'(OpenInputDesktop error {ctypes.get_last_error()})')
    user.CloseDesktop(active_desktop)
    user.SetProcessDpiAwarenessContext.argtypes = [wintypes.HANDLE]
    user.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
    user.CreateWindowExW.argtypes = [wintypes.DWORD, wintypes.LPCWSTR, wintypes.LPCWSTR, wintypes.DWORD,
        ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, wintypes.HWND, wintypes.HMENU, wintypes.HINSTANCE, wintypes.LPVOID]
    user.CreateWindowExW.restype = wintypes.HWND
    user.ShowWindow.argtypes = [wintypes.HWND, ctypes.c_int]
    user.DestroyWindow.argtypes = [wintypes.HWND]
    user.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
    user.GetClientRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
    user.GetDpiForWindow.argtypes = [wintypes.HWND]
    user.ScreenToClient.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.POINT)]
    user.SetWindowPos.argtypes = [wintypes.HWND, wintypes.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, wintypes.UINT]
    user.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
    callback_type = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    user.EnumChildWindows.argtypes = [wintypes.HWND, callback_type, wintypes.LPARAM]
    user.PeekMessageW.argtypes = [ctypes.POINTER(wintypes.MSG), wintypes.HWND, wintypes.UINT, wintypes.UINT, wintypes.UINT]
    user.TranslateMessage.argtypes = [ctypes.POINTER(wintypes.MSG)]
    user.DispatchMessageW.argtypes = [ctypes.POINTER(wintypes.MSG)]
    user.DispatchMessageW.restype = ctypes.c_ssize_t

    def pump():
        message = wintypes.MSG()
        while user.PeekMessageW(ctypes.byref(message), None, 0, 0, 1):
            user.TranslateMessage(ctypes.byref(message))
            user.DispatchMessageW(ctypes.byref(message))

    def pause(seconds):
        until = time.monotonic() + seconds
        while time.monotonic() < until:
            pump()
            time.sleep(.005)

    processes = []
    parent = user.CreateWindowExW(0, 'STATIC', 'Pixels isolated native viewer acceptance',
        0x00CF0000, 60, 60, 1100, 760, None, None, None, None)
    if not parent:
        raise ctypes.WinError(ctypes.get_last_error())
    user.ShowWindow(parent, 4)
    pool = ThreadPoolExecutor(max_workers=2)

    def spawn(arguments):
        child = subprocess.Popen([args.helper, *args.prefix, *arguments], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                 stderr=subprocess.PIPE, creationflags=subprocess.CREATE_NO_WINDOW)
        processes.append(child)
        return child

    def await_io(pending):
        deadline = time.monotonic()+8
        while not pending.done():
            if time.monotonic() > deadline:
                raise TimeoutError('Native parent/pipe operation stalled')
            pause(.005)
        return pending.result()

    def read(child, count):
        def exact():
            result = bytearray()
            while len(result) < count:
                chunk = child.stdout.read(count-len(result))
                if not chunk:
                    raise RuntimeError('Native worker ended: ' + child.stderr.read().decode('utf-8', 'replace'))
                result.extend(chunk)
            return bytes(result)
        return await_io(pool.submit(exact))

    def send(child, value):
        # A large H.264 packet can fill the pipe while the native child sends
        # a synchronous window message to this owner. Both reads AND writes
        # must keep the owning UI thread pumping, as the product's async IPC does.
        def write():
            child.stdin.write(value)
            child.stdin.flush()
        await_io(pool.submit(write))

    try:
        screens = json.loads(subprocess.check_output([args.helper, *args.prefix, 'inventory'], timeout=8))
        screen = next((s for s in screens if s['primary']), screens[0])
        capture = spawn(['capture', screen['id'], '15', 'synthetic'])
        magic, width, height = struct.unpack('<III', read(capture, 12))
        assert magic == 0x31424150
        viewer = spawn(['render', str(width), str(height), '15', 'verify', str(parent), args.rotation])
        assert struct.unpack('<I', read(viewer, 4))[0] == magic
        child_windows = []

        @callback_type
        def visit(window, _):
            pid = wintypes.DWORD()
            user.GetWindowThreadProcessId(window, ctypes.byref(pid))
            if pid.value == viewer.pid:
                child_windows.append(window)
            return True

        def geometry():
            child_windows.clear()
            user.EnumChildWindows(parent, visit, 0)
            assert len(child_windows) == 1, 'Renderer is not embedded in its owner'
            client, child = wintypes.RECT(), wintypes.RECT()
            user.GetClientRect(parent, ctypes.byref(client))
            user.GetWindowRect(child_windows[0], ctypes.byref(child))
            origin = wintypes.POINT(child.left, child.top)
            user.ScreenToClient(parent, ctypes.byref(origin))
            expected_top = round(72 * user.GetDpiForWindow(parent) / 96)
            assert origin.x == 0 and origin.y == expected_top, (origin.x, origin.y, expected_top)
            assert child.right-child.left == client.right
            assert child.bottom-child.top == client.bottom-expected_top

        for index in range(26):
            if index == 5:
                assert user.SetWindowPos(parent, None, 60, 60, 800, 1100, 0x14)
                pause(.15)
                geometry()  # Static repaint, before supplying another frame.
            if index == 10:
                user.ShowWindow(parent, 7)
                pause(.15)
            if index == 14:
                user.ShowWindow(parent, 9)
                assert user.SetWindowPos(parent, None, 60, 60, 1200, 650, 0x14)
                pause(.15)
                geometry()
            send(capture, b'\x01')
            length, sequence, _, _ = struct.unpack('<IQQI', read(capture, 24))
            assert sequence == index and 0 < length <= 16*1024*1024
            send(viewer, struct.pack('<IQ', length, sequence) + read(capture, length))
            assert struct.unpack('<Q', read(viewer, 8))[0] == index
        send(viewer, struct.pack('<I', 0))
        displayed, checks, error = struct.unpack('<QQI', read(viewer, 20))
        assert 12 <= displayed < 26 and checks >= displayed*4 and error <= 12, (displayed, checks, error)
        send(capture, b'\x00')
        # DestroyWindow on a cross-process child can synchronously notify its
        # owner. Keep that owner's thread pumping through worker teardown too.
        deadline = time.monotonic() + 5
        while viewer.poll() is None or capture.poll() is None:
            if time.monotonic() > deadline:
                raise TimeoutError('Native workers did not exit after finish')
            pause(.005)
        assert viewer.returncode == capture.returncode == 0
        print(json.dumps({'embedded': True, 'static_resize': True, 'minimize_restore': True, 'rotation': int(args.rotation),
                          'decoded': 26, 'confirmed_displayed': displayed, 'pixel_checks': checks, 'max_rgb_error': error}))
    finally:
        for child in processes:
            if child.poll() is None:
                child.kill()
            child.communicate(timeout=5)
        user.DestroyWindow(parent)
        pool.shutdown(wait=True, cancel_futures=True)
        faulthandler.cancel_dump_traceback_later()


if __name__ == '__main__':
    main()
