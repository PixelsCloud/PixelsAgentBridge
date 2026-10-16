"""Verify local GPU encode/decode/display, with visible short-lived windows.

Run separately from other GPU benchmarks. JSON reports contain only metrics;
neither synthetic nor real desktop videos are written. Real desktop pixel
readback is explicitly forbidden. This does not test network or input.
"""
import argparse
import ctypes
from ctypes import wintypes
from datetime import datetime, timezone
import json
from pathlib import Path
import subprocess
import time
import uuid


def verify_window_close(binary, display):
    """Close only the owned probe's viewer; never send input to another app."""
    user32 = ctypes.WinDLL("user32", use_last_error=True)
    callback_type = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    user32.EnumWindows.argtypes = [callback_type, wintypes.LPARAM]
    user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
    user32.GetClassNameW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
    user32.PostMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
    process = subprocess.Popen([str(binary), "--display", str(display), "--seconds", "60"],
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, encoding="utf-8")
    windows = []

    @callback_type
    def visit(window, _):
        pid = wintypes.DWORD()
        user32.GetWindowThreadProcessId(window, ctypes.byref(pid))
        name = ctypes.create_unicode_buffer(128)
        user32.GetClassNameW(window, name, len(name))
        if pid.value == process.pid and name.value == "PixelsDecodeDisplayProbe":
            windows.append(window)
        return True

    try:
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline and not windows and process.poll() is None:
            user32.EnumWindows(visit, 0)
            time.sleep(.05)
        if not windows:
            raise RuntimeError("Owned probe viewer did not appear for cancellation test")
        if not user32.PostMessageW(windows[0], 0x0010, 0, 0):  # WM_CLOSE
            raise ctypes.WinError(ctypes.get_last_error())
        _, error = process.communicate(timeout=10)
        assert process.returncode != 0 and "Viewer closed by user" in error, error
        windows.clear()
        user32.EnumWindows(visit, 0)
        assert not windows, "Probe viewer survived process shutdown"
    finally:
        if process.poll() is None:
            process.kill()
            process.communicate(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--display", type=int, default=0)
    parser.add_argument("--capture", action="store_true",
                        help="Also display a real desktop loopback with an animated test window")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    binary = root / ".build/remote-desktop/loopback/loopback-probe.exe"
    output = root / ".build/remote-desktop/loopback" / ("verify-" + uuid.uuid4().hex[:12])
    output.mkdir(parents=True)
    report = {"created_at": datetime.now(timezone.utc).isoformat(), "cases": [],
              "network_tested": False, "input_tested": False, "passed": False}

    def save():
        (output / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")

    cases = [("synthetic_correctness", 15, ["--verify-pixels"]),
             ("synthetic_15fps", 15, []), ("synthetic_30fps", 30, [])]
    if args.capture:
        cases.extend((f"desktop_{fps}fps", fps, ["--capture", "--animate"]) for fps in (15, 30))
    try:
        for name, fps, flags in cases:
            result = subprocess.run([str(binary), "--display", str(args.display), "--fps", str(fps),
                                     "--seconds", "5", *flags], capture_output=True, text=True,
                                    encoding="utf-8", timeout=45)
            (output / f"{name}.stderr.txt").write_text(result.stderr, encoding="utf-8")
            if result.returncode:
                raise RuntimeError(f"{name} failed: {result.stderr.strip()}")
            metrics = json.loads(result.stdout)
            report["cases"].append({"name": name, "metrics": metrics})
            save()
            count = metrics["encoded_frames"]
            assert count == metrics["decoded_frames"] == metrics["present_submissions"] == metrics["display_confirmed_frames"]
            assert metrics["display_confirmation"] == "dxgi_frame_statistics_each_present_id"
            assert metrics["decoder_gpu_surfaces_required"]
            assert 0 < metrics["maximum_pending_frames"] <= 4
            assert metrics["media_pixel_cpu_readbacks"] == metrics["media_gpu_texture_copies"] == 0
            assert metrics["gpu_color_conversion_passes"] == count
            if "--verify-pixels" in flags:
                assert metrics["test_pixel_readbacks"] == metrics["test_pixel_gpu_copies"] == count * 4
                assert metrics["test_max_rgb_error"] <= 12
            else:
                assert metrics["test_pixel_readbacks"] == metrics["test_pixel_gpu_copies"] == 0
                assert metrics["effective_display_fps"] > 12, metrics
                assert metrics["effective_display_fps"] >= fps * .9, metrics
            if name.startswith("synthetic"):
                assert (metrics["width"], metrics["height"]) == (1920, 1080)
            print(json.dumps({"case": name, "display_fps": metrics["effective_display_fps"],
                              "latency_p95_ms": metrics["source_to_display_observed_p95_ms"]}), flush=True)
        for flags in (["--fps", "0"], ["--seconds", "0"], ["--display", "129"],
                      ["--animate"], ["--capture", "--verify-pixels"], ["--output", "forbidden.h264"]):
            result = subprocess.run([str(binary), *flags], capture_output=True, timeout=10)
            assert result.returncode, flags
        report["argument_rejections"] = "passed"
        verify_window_close(binary, args.display)
        report["owned_window_close_and_process_cleanup"] = "passed"
        report["passed"] = True
    except BaseException as error:
        report["error"] = str(error)
        raise
    finally:
        save()
        print(json.dumps({"report": str(output / "report.json"), "passed": report["passed"]}), flush=True)


if __name__ == "__main__":
    main()
