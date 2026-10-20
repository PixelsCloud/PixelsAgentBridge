"""Native viewer events on an owned window; never calls SendInput.

Exercises view-only, letterboxing, rotated surface coordinates, drag clamping,
right modifiers, focus/capture loss and mode barriers without a remote target.
"""
import argparse
import ctypes
from ctypes import wintypes
from concurrent.futures import ThreadPoolExecutor
import json
import struct
import subprocess
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--helper', required=True)
    parser.add_argument('--prefix', action='append', default=[])
    parser.add_argument('--rotation', choices=['0', '90', '180', '270'], default='90')
    args = parser.parse_args()
    user = ctypes.WinDLL('user32', use_last_error=True)
    user.SetProcessDpiAwarenessContext.argtypes = [wintypes.HANDLE]
    user.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
    user.CreateWindowExW.argtypes = [wintypes.DWORD, wintypes.LPCWSTR, wintypes.LPCWSTR, wintypes.DWORD,
        ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, wintypes.HWND, wintypes.HMENU, wintypes.HINSTANCE, wintypes.LPVOID]
    user.CreateWindowExW.restype = wintypes.HWND
    user.ShowWindow.argtypes = [wintypes.HWND, ctypes.c_int]
    user.DestroyWindow.argtypes = [wintypes.HWND]
    user.GetClientRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
    user.ClientToScreen.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.POINT)]
    user.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
    callback = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    user.EnumChildWindows.argtypes = [wintypes.HWND, callback, wintypes.LPARAM]
    user.PeekMessageW.argtypes = [ctypes.POINTER(wintypes.MSG), wintypes.HWND, wintypes.UINT, wintypes.UINT, wintypes.UINT]
    user.TranslateMessage.argtypes = [ctypes.POINTER(wintypes.MSG)]
    user.DispatchMessageW.argtypes = [ctypes.POINTER(wintypes.MSG)]
    user.PostMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
    parent = user.CreateWindowExW(0, 'STATIC', 'Pixels isolated input event acceptance',
        0x00CF0000, 60, 60, 1100, 760, None, None, None, None)
    if not parent:
        raise ctypes.WinError(ctypes.get_last_error())
    user.ShowWindow(parent, 4)
    pool = ThreadPoolExecutor(max_workers=2)
    child = subprocess.Popen([args.helper, *args.prefix, 'render', '1920', '1080', '15', 'interactive', str(parent), args.rotation],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, creationflags=subprocess.CREATE_NO_WINDOW)

    def pause():
        message = wintypes.MSG()
        while user.PeekMessageW(ctypes.byref(message), None, 0, 0, 1):
            user.TranslateMessage(ctypes.byref(message))
            user.DispatchMessageW(ctypes.byref(message))
        time.sleep(.002)

    def wait(future):
        until = time.monotonic() + 8
        while not future.done():
            if time.monotonic() > until:
                raise TimeoutError('Native event/owner operation stalled')
            pause()
        return future.result()

    def read(pipe, count):
        def exact():
            result = bytearray()
            while len(result) < count:
                data = pipe.read(count-len(result))
                if not data:
                    raise RuntimeError('Native event worker ended')
                result.extend(data)
            return result
        return wait(pool.submit(exact))

    def send(data):
        def write():
            child.stdin.write(data)
            child.stdin.flush()
        wait(pool.submit(write))

    def event(expected):
        until = time.monotonic()+8
        while True:
            actual = struct.unpack('<BHHHB', read(child.stderr, 8))
            if actual == expected:
                return
            # SetCapture can enqueue a real-cursor motion in addition to our
            # posted coordinate sample. It must not obscure the requested event;
            # key/button/releases/barriers are still checked without skipping.
            assert actual[0] == 1 and time.monotonic() < until, (actual, expected)

    try:
        assert struct.unpack('<I', read(child.stdout, 4))[0] == 0x31424150
        windows = []

        @callback
        def visit(window, _):
            pid = wintypes.DWORD()
            user.GetWindowThreadProcessId(window, ctypes.byref(pid))
            if pid.value == child.pid:
                windows.append(window)
            return True

        user.EnumChildWindows(parent, visit, 0)
        assert len(windows) == 1
        window = windows[0]

        def post(message, value=0, param=0):
            assert user.PostMessageW(window, message, value, param)
            # Let posted messages precede the next explicit pipe barrier.
            until = time.monotonic() + .03
            while time.monotonic() < until:
                pause()

        def position(x, y):
            return (x & 65535) | ((y & 65535) << 16)

        # Disabled input must not emit the posted F24 down/up.
        post(0x100, 0x87)
        post(0x101, 0x87)
        send(struct.pack('<IB', 0xffffffff, 0))
        event((6, 0, 0, 0, 0))
        send(struct.pack('<IB', 0xffffffff, 1))
        event((6, 0, 0, 1, 0))
        bounds = wintypes.RECT()
        assert user.GetClientRect(window, ctypes.byref(bounds))
        width, height = (1080, 1920) if args.rotation in ('90', '270') else (1920, 1080)
        scale = min(bounds.right/width, bounds.bottom/height)
        vw, vh = int(width*scale), int(height*scale)
        left, top = (bounds.right-vw)//2, (bounds.bottom-vh)//2
        post(0x200, 0, position(0, 0))  # ignored letterbox
        post(0x200, 0, position(left, top))
        event((1, 0, 0, 0, 0))
        post(0x200, 0, position(left+vw-1, top+vh-1))
        event((1, 65535, 65535, 0, 0))
        wheel_point = wintypes.POINT(left+vw-1, top+vh-1)
        assert user.ClientToScreen(window, ctypes.byref(wheel_point))
        post(0x20a, 120 << 16, position(wheel_point.x, wheel_point.y))
        event((3, 65535, 65535, 120, 0))
        post(0x100, 0x11, 1 << 24)  # VK_CONTROL + extended = right Ctrl
        event((4, 0, 0, 0xa3, 1))
        post(0x8)  # focus loss releases owned remote state
        event((5, 0, 0, 0, 0))
        post(0x201, 0, position(left, top))
        event((2, 0, 0, 0, 1))
        post(0x200, 0, position(-20, bounds.bottom+30))
        event((1, 0, 65535, 0, 0))
        post(0x202, 0, position(-20, bounds.bottom+30))
        event((2, 0, 65535, 0, 0))
        post(0x204, 0, position(left, top))
        event((2, 0, 0, 1, 1))
        post(0x215)  # capture loss
        event((5, 0, 0, 0, 0))
        send(struct.pack('<IB', 0xffffffff, 0))
        event((5, 0, 0, 0, 0))
        event((6, 0, 0, 0, 0))
        post(0x100, 0x87)
        send(struct.pack('<IB', 0xffffffff, 0))
        event((6, 0, 0, 0, 0))
        send(struct.pack('<I', 0))
        assert struct.unpack('<QQI', read(child.stdout, 20)) == (0, 0, 0)
        until = time.monotonic()+5
        while child.poll() is None:
            if time.monotonic() > until:
                raise TimeoutError('Viewer did not exit')
            pause()
        assert child.returncode == 0
        print(json.dumps({'rotation': int(args.rotation), 'view_only': True, 'viewport_coordinates': True,
            'drag_clamping': True, 'wheel_position': True, 'right_modifier': True, 'focus_release': True, 'capture_release': True, 'mode_barrier': True}))
    finally:
        if child.poll() is None:
            child.kill()
        child.communicate(timeout=5)
        user.DestroyWindow(parent)
        pool.shutdown(wait=True, cancel_futures=True)


if __name__ == '__main__':
    main()
