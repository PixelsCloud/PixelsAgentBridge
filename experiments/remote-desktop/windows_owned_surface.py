"""Read composed pixels from an isolated viewer's native child window.

The caller supplies its Desktop PID; ownership is checked before and after
capture. This never captures another application's desktop pixels or moves it.
"""
import ctypes as C
from ctypes import wintypes as W
from PIL import Image


def capture(hwnd, desktop_pid):
    u = C.WinDLL('user32', use_last_error=True)
    g = C.WinDLL('gdi32', use_last_error=True)

    def fn(lib, name, result, *params):
        f = getattr(lib, name)
        f.restype, f.argtypes = result, params
        return f

    fn(u, 'GetAncestor', W.HWND, W.HWND, W.UINT)
    fn(u, 'GetWindowThreadProcessId', W.DWORD, W.HWND, C.POINTER(W.DWORD))
    fn(u, 'GetClassNameW', C.c_int, W.HWND, W.LPWSTR, C.c_int)
    fn(u, 'GetClientRect', W.BOOL, W.HWND, C.POINTER(W.RECT))
    fn(u, 'IsWindowVisible', W.BOOL, W.HWND)
    fn(u, 'IsIconic', W.BOOL, W.HWND)
    fn(u, 'GetDC', W.HDC, W.HWND)
    fn(u, 'ReleaseDC', C.c_int, W.HWND, W.HDC)
    fn(u, 'PrintWindow', W.BOOL, W.HWND, W.HDC, W.UINT)
    fn(g, 'CreateCompatibleDC', W.HDC, W.HDC)
    fn(g, 'DeleteDC', W.BOOL, W.HDC)
    fn(g, 'SelectObject', W.HANDLE, W.HDC, W.HANDLE)
    fn(g, 'DeleteObject', W.BOOL, W.HANDLE)
    fn(g, 'GdiFlush', W.BOOL)

    def identity():
        parent = u.GetAncestor(hwnd, 2)
        owner, child = W.DWORD(), W.DWORD()
        u.GetWindowThreadProcessId(parent, C.byref(owner))
        u.GetWindowThreadProcessId(hwnd, C.byref(child))
        name, rect = C.create_unicode_buffer(128), W.RECT()
        u.GetClassNameW(hwnd, name, len(name))
        assert owner.value == desktop_pid and name.value == 'PixelsRemoteDesktopSurface'
        assert u.IsWindowVisible(hwnd) and not u.IsIconic(parent)
        assert u.GetClientRect(hwnd, C.byref(rect))
        assert 0 < rect.right <= 8192 and 0 < rect.bottom <= 8192
        return parent, child.value, rect.right, rect.bottom

    expected = identity()
    _, _, width, height = expected

    class Header(C.Structure):
        _fields_ = [('size', W.DWORD), ('width', W.LONG), ('height', W.LONG),
                    ('planes', W.WORD), ('bits', W.WORD), ('compression', W.DWORD),
                    ('image_size', W.DWORD), ('xppm', W.LONG), ('yppm', W.LONG),
                    ('colors', W.DWORD), ('important', W.DWORD)]

    class Info(C.Structure):
        _fields_ = [('header', Header), ('color', W.DWORD)]

    fn(g, 'CreateDIBSection', W.HBITMAP, W.HDC, C.POINTER(Info), W.UINT,
       C.POINTER(C.c_void_p), W.HANDLE, W.DWORD)
    info = Info()
    info.header = Header(C.sizeof(Header), width, -height, 1, 32, 0,
                         width * height * 4, 0, 0, 0, 0)
    screen = memory = bitmap = previous = None
    try:
        screen = u.GetDC(None)
        assert screen
        memory = g.CreateCompatibleDC(screen)
        assert memory
        bits = C.c_void_p()
        bitmap = g.CreateDIBSection(screen, C.byref(info), 0, C.byref(bits), None, 0)
        assert bitmap and bits.value
        previous = g.SelectObject(memory, bitmap)
        assert previous and previous != C.c_void_p(-1).value
        assert u.PrintWindow(hwnd, memory, 2), 'Owned surface capture failed'
        assert g.GdiFlush()
        assert identity() == expected, 'Native surface changed during capture'
        return Image.frombytes('RGB', (width, height),
                               C.string_at(bits, width * height * 4), 'raw', 'BGRX')
    finally:
        if previous and previous != C.c_void_p(-1).value:
            g.SelectObject(memory, previous)
        if bitmap:
            g.DeleteObject(bitmap)
        if memory:
            g.DeleteDC(memory)
        if screen:
            u.ReleaseDC(None, screen)
