"""Check timed/legacy native frame IPC using synthetic pixels, never the desktop.

Run on an interactive Windows/macOS host with a freshly built media worker.
Only the owned worker is stopped; no installed service or user session changes.
"""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import sys
import threading
import time

MAGIC = 0x31424150


def verify_packets(worker, worker_args, width, height, records, software_decoder):
    """Explicit synthetic-only visible replay, including a live encoder switch."""
    # Use the production asynchronous adapter: a format/SPS transition may
    # temporarily buffer output, even when ordinary steady-state frames do not.
    mode = "stream-verify-software" if software_decoder else "stream-verify"
    child = subprocess.Popen([str(worker), *worker_args, "render", str(width), str(height), "15", mode],
                             stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                             creationflags=0x08000020 if sys.platform == 'win32' else 0)
    tail = bytearray()
    def drain():
        while chunk := child.stderr.read(1024):
            tail.extend(chunk)
            del tail[:-4096]
    reader = threading.Thread(target=drain, daemon=True)
    reader.start()
    deadline = threading.Timer(30, child.kill)
    deadline.start()
    def take(size):
        data = bytearray()
        while len(data) < size:
            chunk = child.stdout.read(size-len(data))
            if not chunk:
                # stdout may close just before the native exception reaches
                # stderr. Preserve the original cause instead of an empty EOF.
                try:child.wait(timeout=.5)
                except subprocess.TimeoutExpired:pass
                reader.join(timeout=.5)
                raise RuntimeError("Native synthetic replay ended: " + tail.decode('utf-8', errors='replace'))
            data.extend(chunk)
        return data
    try:
        assert struct.unpack('<II', take(8)) == (MAGIC, 2)
        if sys.platform == 'win32':
            import ctypes
            from ctypes import wintypes
            user = ctypes.WinDLL('user32', use_last_error=True)
            callback_type = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
            user.EnumWindows.argtypes = [callback_type, wintypes.LPARAM]
            user.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
            user.IsWindowVisible.argtypes = [wintypes.HWND]
            user.SetWindowPos.argtypes = [wintypes.HWND, wintypes.HWND, ctypes.c_int, ctypes.c_int,
                                         ctypes.c_int, ctypes.c_int, wintypes.UINT]
            windows = []
            def find(window, _):
                pid = wintypes.DWORD()
                user.GetWindowThreadProcessId(window, ctypes.byref(pid))
                if pid.value == child.pid and user.IsWindowVisible(window):windows.append(window)
                return True
            if not user.EnumWindows(callback_type(find), 0) or len(windows) != 1:
                raise RuntimeError('Cannot qualify the owned synthetic viewer window')
            # Only this child process's generated-content window is raised.
            if not user.SetWindowPos(windows[0], wintypes.HWND(-1), 0, 0, 0, 0, 0x53):
                raise ctypes.WinError(ctypes.get_last_error())
        accepted, presented, decoder, summary = 0, 0, None, None
        def event():
            nonlocal accepted, presented, decoder, summary
            kind = take(1)[0]
            if kind == 1:
                index = struct.unpack('<Q', take(8))[0]
                assert index == accepted
                accepted += 1
            elif kind in (2, 7):
                index = struct.unpack('<Q', take(8))[0]
                if kind == 7:take(16)
                assert index == presented and index < accepted
                presented += 1
            elif kind == 4:
                assert decoder is None
                decoder = take(1)[0]
                assert decoder in (1, 2, 3) and (not software_decoder or decoder == 2)
            elif kind == 3:summary = struct.unpack('<QQI', take(20))
            elif kind == 5:
                length = struct.unpack('<H', take(2))[0]
                assert 0 < length <= 512
                raise RuntimeError(take(length).decode('utf-8'))
            else:raise RuntimeError('Unexpected native replay event: ' + str(kind))
        for index, packet in enumerate(records):
            child.stdin.write(struct.pack('<IQ', len(packet), index) + packet)
            child.stdin.flush()
            while accepted <= index:event()
        child.stdin.write(struct.pack('<I', 0))
        child.stdin.flush()
        while summary is None:event()
        displayed, checks, max_error = summary
        assert decoder is not None and presented == len(records)
        assert child.wait(timeout=5) == 0
        assert displayed > 0, "No native synthetic presentation was observed"
        assert checks == len(records)*4 and max_error <= 12, "Encoder-switch picture verification failed"
        return dict(software_decoder_requested=software_decoder, decoder=decoder, frames=len(records),
                    displayed=displayed, quadrant_checks=checks, max_rgb_error=max_error)
    finally:
        deadline.cancel()
        if child.poll() is None:child.kill()
        child.wait(timeout=5)
        reader.join(timeout=2)
        child.stdin.close();child.stdout.close();child.stderr.close()


def check(worker, software, worker_args=(), fail_encoder=False, show_synthetic=False, budget_bps=750000):
    if software and fail_encoder:
        raise ValueError("encoder fault qualification requires hardware selection")
    executable = [str(worker), *worker_args]
    inventory = subprocess.run([*executable, "inventory"], capture_output=True, timeout=15, check=True)
    screens = json.loads(inventory.stdout)
    if not screens:
        raise RuntimeError("An interactive display is required")
    mode = "synthetic-software" if software else "synthetic"
    if fail_encoder:
        mode += "-fail-encoder"
    process = subprocess.Popen([*executable, "capture", screens[0]["id"], "15", mode],
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    tail = bytearray()

    def drain():
        while chunk := process.stderr.read(1024):
            tail.extend(chunk)
            del tail[:-4096]

    reader = threading.Thread(target=drain, daemon=True)
    reader.start()
    deadline = threading.Timer(30, process.kill)
    deadline.start()

    def read(size):
        value = bytearray()
        while len(value) < size:
            chunk = process.stdout.read(size - len(value))
            if not chunk:
                raise RuntimeError("Native IPC ended early: " + tail.decode("utf-8", errors="replace"))
            value.extend(chunk)
        return bytes(value)

    def unpack(fmt):
        return struct.unpack(fmt, read(struct.calcsize(fmt)))

    frames = []
    records = []
    try:
        magic, width, height = unpack("<III")
        assert magic == MAGIC and 0 < width <= 8192 and 0 < height <= 8192
        pts = 0
        # Alternate layouts on one encoder, including after a budget change.
        commands = [3, 1, 4, 1, 4, 3] if sys.platform == 'win32' else [3, 1, 3, 1, 3, 3]
        for index, command in enumerate(commands):
            if index == 4:
                # Exhausted display credit can leave an old native deadline
                # far in the past. Recovery must not spend it as burst credit.
                time.sleep(.25)
            if index == 3:
                process.stdin.write(b"\x02" + struct.pack("<I", budget_bps))
                process.stdin.flush()
                assert unpack("<I")[0] in (MAGIC, MAGIC + 1)
            process.stdin.write(bytes([command]))
            process.stdin.flush()
            size, sequence, timestamp, key = unpack("<IQQI")
            assert 0 < size <= 16 * 1024 * 1024 and sequence == index and timestamp >= pts
            if index == 5:
                assert timestamp - pts >= 1_000_000 // 15 - 1000, "Native capture burst after a pause"
            assert key in (0, 1) and (index != 0 or key == 1)
            if fail_encoder and index == 2:
                assert key == 1, "Software fallback did not restart with an independent IDR"
            timing = unpack("<QQ") if command in (3, 4) else None
            if timing:
                assert 0 <= timing[0] < 30_000_000 and 0 < timing[1] < 30_000_000
            detail = unpack("<QQQQ") if command == 4 else None
            if detail:
                assert detail[0] >= sum(timing)
                assert sum(detail[1:]) <= timing[1]
                if fail_encoder and index == 2:
                    assert detail[1] > 0, "Injected GPU failure did not enter software readback"
            packet = read(size)
            if show_synthetic:records.append(packet)
            assert packet.startswith((b"\x00\x00\x01", b"\x00\x00\x00\x01"))
            frames.append({"id": sequence + 1, "bytes": size, "pts_us": timestamp,
                           "wait_us": timing[0] if timing else None,
                           "encode_us": timing[1] if timing else None,
                           "pipeline_us": detail,
                           "synthetic_sha256": hashlib.sha256(packet).hexdigest()})
            pts = timestamp
        process.stdin.write(b"\x00")
        process.stdin.flush()
        assert process.wait(timeout=5) == 0
        reader.join(timeout=2)
        diagnostics = tail.decode("utf-8", errors="replace")
        if fail_encoder:
            assert "fallback=openh264_software" in diagnostics, "Missing native fallback evidence"
        replay, replay_errors = [], []
        if show_synthetic:
            for decoder in (False, True):
                try:replay.append(verify_packets(worker, worker_args, width, height, records, decoder))
                except Exception as error:
                    replay_errors.append(dict(software_decoder_requested=decoder, error=str(error)))
            if replay_errors:
                raise RuntimeError("Synthetic replay failed: " + json.dumps(replay_errors))
        return {"status": "passed", "synthetic": True, "software_requested": software,
                "requested_budget_bps": budget_bps,
                "encoder_fallback_checked": fail_encoder,
                "native_diagnostics": diagnostics,
                "visible_replay": replay,
                "pacing_after_pause_checked": True,
                "width": width, "height": height, "frames": frames}
    finally:
        deadline.cancel()
        if process.poll() is None:
            process.kill()
        process.wait(timeout=5)
        reader.join(timeout=2)
        process.stdin.close()
        process.stdout.close()
        process.stderr.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--worker", type=Path, required=True)
    parser.add_argument("--software", action="store_true")
    parser.add_argument("--fail-encoder", action="store_true")
    parser.add_argument("--show-synthetic", action="store_true", help="Open both native decoder paths on an available test desktop")
    parser.add_argument("--budget-bps", type=int, default=750000,
                        help="Native IPC budget update; use 0 to isolate full-quality encoder-switch verification")
    parser.add_argument("--worker-arg", action="append", default=[], help="Prefix argument for an embedded worker, e.g. --worker-arg=--media-worker")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.software and args.fail_encoder:
        parser.error("--software and --fail-encoder are mutually exclusive")
    if args.budget_bps != 0 and not 150000 <= args.budget_bps <= 16000000:
        parser.error("budget must be zero or 150000..16000000 bits per second")
    result = check(args.worker.resolve(strict=True), args.software, args.worker_arg, args.fail_encoder, args.show_synthetic, args.budget_bps)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"status": result["status"], "frames": len(result["frames"])}))


if __name__ == "__main__":
    main()
