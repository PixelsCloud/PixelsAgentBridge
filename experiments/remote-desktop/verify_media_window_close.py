"""Close only this test's native window and verify its capture child exits."""
import argparse
import ctypes
from ctypes import wintypes
import json
from pathlib import Path
import subprocess
import time


def children(pid):
    command = f"@(Get-CimInstance Win32_Process -Filter 'ParentProcessId={pid}' | Select-Object -ExpandProperty ProcessId) | ConvertTo-Json -Compress"
    text = subprocess.check_output(["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", command], text=True).strip()
    values = json.loads(text) if text else []
    return set(values if isinstance(values, list) else [values])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--helper", type=Path)
    parser.add_argument("--window-class", default="PixelsDecodeDisplayProbe")
    parser.add_argument("--kill-parent", action="store_true", help="Test unexpected parent loss instead of WM_CLOSE")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    helper = args.helper.resolve() if args.helper else root / ".build/remote-desktop/stream-media/stream-media-probe.exe"
    user32 = ctypes.WinDLL("user32", use_last_error=True)
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    callback_type = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    user32.EnumWindows.argtypes = [callback_type, wintypes.LPARAM]
    user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
    user32.GetClassNameW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
    user32.PostMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
    kernel32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel32.OpenProcess.restype = wintypes.HANDLE
    kernel32.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
    kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
    process = subprocess.Popen([str(args.binary.resolve()), "loopback", str(helper), "30", "1500", "synthetic", "normal", "once"],
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, encoding="utf-8", creationflags=subprocess.CREATE_NO_WINDOW)
    owned = set()
    windows = []
    handles = []

    @callback_type
    def visit(window, _):
        pid = wintypes.DWORD()
        user32.GetWindowThreadProcessId(window, ctypes.byref(pid))
        name = ctypes.create_unicode_buffer(128)
        user32.GetClassNameW(window, name, len(name))
        if pid.value in owned and name.value == args.window_class: windows.append(window)
        return True

    try:
        deadline = time.monotonic() + 15
        while not windows and time.monotonic() < deadline and process.poll() is None:
            owned.update(children(process.pid))
            user32.EnumWindows(visit, 0)
            time.sleep(.05)
        assert windows and len(owned) >= 2, "native capture/viewer did not start"
        for pid in owned:
            handle = kernel32.OpenProcess(0x00100000, False, pid)  # SYNCHRONIZE, no mutation rights
            if handle: handles.append(handle)
        if args.kill_parent:
            process.kill()  # Only the parent created by this test.
        else:
            assert user32.PostMessageW(windows[0], 0x0010, 0, 0), "WM_CLOSE failed"
        _, error = process.communicate(timeout=15)
        assert process.returncode != 0
        if not args.kill_parent: assert "Viewer closed by user" in error, error
        for handle in handles:
            assert kernel32.WaitForSingleObject(handle, 5000) == 0, "owned media child survived close"
        print(json.dumps({"trigger": "parent_loss" if args.kill_parent else "owned_window_close", "capture_and_viewer_exit": "passed", "observed_children": len(handles)}))
    finally:
        for handle in handles: kernel32.CloseHandle(handle)
        if process.poll() is None:
            process.kill()
            process.communicate(timeout=5)


if __name__ == "__main__":
    main()
