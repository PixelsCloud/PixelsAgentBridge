"""Sample generated frame codes from an owned, visible native viewer surface.

Reads either the desktop compositor or the owned window's composed content,
not decoder events. Window capture permits unrelated windows to cover the viewer.
Only generated frame-code pixels, IDs and times are saved. Requires the generated 1080p pattern from
linux_display_fixture.py or macos_display_fixture.m; rejects invalid
complementary cells and backwards frame IDs.
"""
import argparse
import ctypes as C
from ctypes import wintypes as W
import json
import pathlib
import time


def threshold_failures(failure, rate, invalid, samples, duration):
    """Keep observation validity separate from observed video throughput."""
    reasons = []
    if failure is not None:
        reasons.append('sampling_exception')
    if rate <= 12:
        reasons.append('observed_fps_not_above_12')
    if invalid > samples * .05:
        reasons.append('invalid_frame_codes_above_5_percent')
    if duration <= 0 or samples / duration < 30:
        reasons.append('sampling_rate_below_30_hz')
    return reasons


def main():
    p = argparse.ArgumentParser()
    p.add_argument('profile', type=pathlib.Path)
    p.add_argument('--seconds', type=int, default=30)
    p.add_argument('--output', type=pathlib.Path, help='Unique result path for repeated bounded samples')
    p.add_argument('--capture-method', choices=('desktop', 'window'), default='desktop')
    args = p.parse_args()
    assert 10 <= args.seconds <= 60
    result_path = args.output or args.profile / 'presentation.json'
    assert not result_path.exists(), 'Preserve the previous presentation sample'
    owner = json.loads((args.profile / 'process.json').read_text())['pid']
    u, g = C.WinDLL('user32', use_last_error=True), C.WinDLL('gdi32', use_last_error=True)
    def fn(lib, name, result, *params):
        f = getattr(lib, name); f.restype = result; f.argtypes = params
        return f
    fn(u, 'SetProcessDpiAwarenessContext', W.BOOL, W.HANDLE)(W.HANDLE(-4))
    callback = C.WINFUNCTYPE(W.BOOL, W.HWND, W.LPARAM)
    fn(u, 'EnumWindows', W.BOOL, callback, W.LPARAM)
    fn(u, 'EnumChildWindows', W.BOOL, W.HWND, callback, W.LPARAM)
    fn(u, 'GetWindowThreadProcessId', W.DWORD, W.HWND, C.POINTER(W.DWORD))
    fn(u, 'WindowFromPoint', W.HWND, W.POINT)
    fn(u, 'GetClassNameW', C.c_int, W.HWND, W.LPWSTR, C.c_int)
    fn(u, 'GetWindowRect', W.BOOL, W.HWND, C.POINTER(W.RECT))
    fn(u, 'IsWindowVisible', W.BOOL, W.HWND)
    fn(u, 'GetAncestor', W.HWND, W.HWND, W.UINT)
    fn(u, 'IsIconic', W.BOOL, W.HWND)
    fn(u, 'PrintWindow', W.BOOL, W.HWND, W.HDC, W.UINT)
    fn(u, 'GetDC', W.HDC, W.HWND)
    fn(u, 'ReleaseDC', C.c_int, W.HWND, W.HDC)
    fn(g, 'CreateCompatibleDC', W.HDC, W.HDC)
    fn(g, 'SelectObject', W.HANDLE, W.HDC, W.HANDLE)
    fn(g, 'DeleteObject', W.BOOL, W.HANDLE)
    fn(g, 'DeleteDC', W.BOOL, W.HDC)
    fn(g, 'GdiFlush', W.BOOL)
    fn(g, 'BitBlt', W.BOOL, W.HDC, C.c_int, C.c_int, C.c_int, C.c_int,
       W.HDC, C.c_int, C.c_int, W.DWORD)
    surfaces = []
    @callback
    def child(hwnd, _):
        pid = W.DWORD(); u.GetWindowThreadProcessId(hwnd, C.byref(pid))
        name = C.create_unicode_buffer(256); u.GetClassNameW(hwnd, name, 256)
        # The media worker owns this child HWND, reparented into the Desktop
        # window. Parent enumeration is restricted to the isolated Desktop PID.
        if name.value == 'PixelsRemoteDesktopSurface':
            surfaces.append(hwnd)
        return True
    @callback
    def top(hwnd, _):
        pid = W.DWORD(); u.GetWindowThreadProcessId(hwnd, C.byref(pid))
        if pid.value == owner: u.EnumChildWindows(hwnd, child, 0)
        return True
    u.EnumWindows(top, 0)
    assert len(surfaces) == 1 and u.IsWindowVisible(surfaces[0])
    surface_pid = W.DWORD(); u.GetWindowThreadProcessId(surfaces[0], C.byref(surface_pid))
    parent_window = u.GetAncestor(surfaces[0], 2)
    parent_pid = W.DWORD(); u.GetWindowThreadProcessId(parent_window, C.byref(parent_pid))
    assert parent_pid.value == owner
    bounds = W.RECT(); assert u.GetWindowRect(surfaces[0], C.byref(bounds))
    width, height = bounds.right - bounds.left, bounds.bottom - bounds.top
    scale = min(width / 1920, height / 1080)
    origin = (bounds.left + (width - 1920 * scale) / 2,
              bounds.top + (height - 1080 * scale) / 2)
    points = [(round(origin[0] + (240 + i * 96) * scale),
               round(origin[1] + (200 + row * 96) * scale))
              for row in range(2) for i in range(16)]

    def window_hits():
        # Failure-only metadata helps distinguish an occluding local window
        # from damaged/incorrect video. Never inspect another window's title.
        hits = []
        for x, y in points:
            hwnd = u.WindowFromPoint(W.POINT(x, y))
            pid = W.DWORD(); u.GetWindowThreadProcessId(hwnd, C.byref(pid))
            name = C.create_unicode_buffer(256); u.GetClassNameW(hwnd, name, 256)
            hits.append({'hwnd': hwnd, 'pid': pid.value, 'class': name.value})
        return hits
    left, top_y = min(x for x, y in points), min(y for x, y in points)
    w, h = max(x for x, y in points) - left + 1, max(y for x, y in points) - top_y + 1
    if args.capture_method == 'window':
        left, top_y, w, h = bounds.left, bounds.top, width, height

    class Header(C.Structure):
        _fields_ = [('size', W.DWORD), ('width', W.LONG), ('height', W.LONG),
                    ('planes', W.WORD), ('bits', W.WORD), ('compression', W.DWORD),
                    ('image_size', W.DWORD), ('xppm', W.LONG), ('yppm', W.LONG),
                    ('colors', W.DWORD), ('important', W.DWORD)]
    class Info(C.Structure):
        _fields_ = [('header', Header), ('color', W.DWORD * 1)]
    fn(g, 'CreateDIBSection', W.HBITMAP, W.HDC, C.POINTER(Info), W.UINT,
       C.POINTER(C.c_void_p), W.HANDLE, W.DWORD)
    info = Info(); info.header = Header(C.sizeof(Header), w, -h, 1, 32, 0, w*h*4, 0, 0, 0, 0)
    screen = u.GetDC(None); memory = g.CreateCompatibleDC(screen)
    bits = C.c_void_p(); bitmap = g.CreateDIBSection(screen, C.byref(info), 0, C.byref(bits), None, 0)
    assert screen and memory and bitmap and bits.value
    previous = g.SelectObject(memory, bitmap)
    pixels = (C.c_ubyte * (w*h*4)).from_address(bits.value)
    offsets = [((y-top_y)*w+x-left)*4 for x, y in points]
    events, invalid, samples, repeated = [], 0, 0, 0
    first_invalid = None
    first_nonmonotonic = None
    failure = None
    capture_calls = capture_seconds = slow_capture_calls = 0
    max_capture_seconds = max_sample_gap_seconds = 0
    previous_sample_at = None
    started_unix_ms = round(time.time() * 1000)
    start = time.perf_counter(); next_sample = start; last = None
    try:
        while time.perf_counter() - start < args.seconds:
            current_pid = W.DWORD(); u.GetWindowThreadProcessId(surfaces[0], C.byref(current_pid))
            assert current_pid.value == surface_pid.value and u.GetAncestor(surfaces[0], 2) == parent_window, 'Native surface ownership changed'
            fresh = W.RECT(); assert u.GetWindowRect(surfaces[0], C.byref(fresh))
            assert bytes(fresh) == bytes(bounds), 'Viewer moved during measurement'
            assert u.IsWindowVisible(surfaces[0]) and not u.IsIconic(u.GetAncestor(surfaces[0], 2)), 'Viewer hidden or minimized'
            capture_started = time.perf_counter()
            if args.capture_method == 'window':
                # PW_RENDERFULLCONTENT captures this HWND's composed image.
                # No decoder-side callbacks or frame counters are consulted.
                assert u.PrintWindow(surfaces[0], memory, 2), 'Owned window capture failed'
            else:
                assert g.BitBlt(memory, 0, 0, w, h, screen, left, top_y, 0x00CC0020 | 0x40000000)
            assert g.GdiFlush(), 'GDI flush failed'
            captured_at = time.perf_counter()
            capture_elapsed = captured_at - capture_started
            capture_calls += 1
            capture_seconds += capture_elapsed
            slow_capture_calls += capture_elapsed > 1/60
            max_capture_seconds = max(max_capture_seconds, capture_elapsed)
            if previous_sample_at is not None:
                max_sample_gap_seconds = max(max_sample_gap_seconds, captured_at - previous_sample_at)
            previous_sample_at = captured_at
            values = []
            for offset in offsets:
                rgb = pixels[offset:offset+3]
                values.append(1 if min(rgb) > 200 else 0 if max(rgb) < 50 else -1)
            samples += 1
            valid = all(v >= 0 for v in values) and all(values[i] != values[i+16] for i in range(16))
            valid = valid and values[:2] == [1, 0] and values[14:16] == [0, 1]
            if valid:
                number = sum(values[i+2] << i for i in range(12))
                if number != last:
                    if last is not None and not 0 < (number-last) % 4096 < 2048:
                        first_nonmonotonic = {
                            'elapsed': time.perf_counter()-start, 'sample': samples,
                            'previous_number': last, 'number': number,
                            'modular_delta': (number-last) % 4096, 'bits': values,
                            'bgr': [list(pixels[offset:offset+3]) for offset in offsets],
                            'window_hits': window_hits(),
                        }
                        raise AssertionError('Nonmonotonic presentation')
                    events.append([time.perf_counter()-start, number]); last = number
                else: repeated += 1
            else:
                invalid += 1
                if first_invalid is None:
                    first_invalid = {'elapsed': time.perf_counter()-start,
                                     'bits': values,
                                     'bgr': [list(pixels[offset:offset+3]) for offset in offsets],
                                     'window_hits': window_hits()}
            next_sample += 1/60
            time.sleep(max(0, next_sample-time.perf_counter()))
    except BaseException as error:
        # Save the triggering numbers and preceding events before propagating
        # any failure. Never convert a partial/failed sample into a pass.
        failure = error
    finally:
        g.SelectObject(memory, previous); g.DeleteObject(bitmap); g.DeleteDC(memory); u.ReleaseDC(None, screen)
    duration = time.perf_counter()-start
    steady = [e for e in events if e[0] >= 3]
    # Use the entire measurement window, including any stalled tail. An interval
    # between the first/last observed frame would hide a freeze near the end.
    rate = len(steady) / (duration - 3) if duration > 3 else 0
    times = [min(3, duration)] + [e[0] for e in steady] + [duration]
    longest_gap = max(b-a for a, b in zip(times, times[1:]))
    reasons = threshold_failures(failure, rate, invalid, samples, duration)
    report = {'method': ('desktop-visible pixels' if args.capture_method == 'desktop' else 'owned native window composed pixels') + ', 12-bit generated frame code with complement',
              'capture_method': args.capture_method,
              'started_unix_ms': started_unix_ms, 'finished_unix_ms': round(time.time() * 1000),
              'source_dimensions': [1920,1080], 'surface_dimensions': [width,height],
              'surface_hwnd': surfaces[0], 'desktop_pid': owner,
              'surface_pid': surface_pid.value, 'parent_hwnd': parent_window,
              'surface_screen_bounds': [bounds.left,bounds.top,bounds.right,bounds.bottom],
              'sample_points': points, 'first_invalid_sample': first_invalid,
              'first_nonmonotonic_sample': first_nonmonotonic,
              'failure': None if failure is None else type(failure).__name__ + ': ' + str(failure),
              'seconds': duration, 'samples': samples, 'invalid_samples': invalid,
              'sample_hz': samples/duration if duration > 0 else 0,
              'threshold_failures': reasons,
              'capture_timing': {'calls': capture_calls, 'total_seconds': capture_seconds,
                                 'max_seconds': max_capture_seconds,
                                 'calls_above_60_hz_budget': slow_capture_calls,
                                 'max_sample_gap_seconds': max_sample_gap_seconds},
              'repeated_samples': repeated, 'unique_presentations': len(events),
              'warmup_seconds': 3, 'steady_fps': rate,
              'longest_presentation_gap_seconds': longest_gap, 'events': events,
              'passed': not reasons}
    with result_path.open('x', encoding='utf-8') as output:
        json.dump(report, output, indent=2)
    print(json.dumps({k:v for k,v in report.items() if k != 'events'}), flush=True)
    if failure is not None:
        raise failure
    assert report['passed'], 'Actual presentation threshold not met'


if __name__ == '__main__': main()
