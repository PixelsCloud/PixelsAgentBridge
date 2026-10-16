"""Opt-in native media acceptance; only generated quadrants may be saved/replayed.

capture WORKER DIRECTORY [--prefix synthetic|windows] generates six-second
fixtures on the encoder host. Transfer the files to the Mac fixture directory.
render WORKER DIRECTORY --suite rotations|software|failure|invalid|lifecycle runs the
Mac renderer, briefly showing its own window. No screen capture/input or TCC
changes. Reports contain timings/counts/hashes, never decoded desktop pixels.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import struct
import subprocess
import threading
import time


def read(pipe, size):
    data = bytearray()
    while len(data) < size:
        part = pipe.read(size - len(data))
        if not part:
            raise RuntimeError("Worker pipe ended")
        data.extend(part)
    return bytes(data)


def spawn(worker, arguments):
    process = subprocess.Popen(
        [str(worker), *arguments], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
    )
    timer = threading.Timer(25, process.kill)
    timer.start()
    return process, timer


def retire(process, timer):
    timer.cancel()
    if process.poll() is None:
        process.kill()
    process.wait(timeout=5)


def fixture(path):
    with path.open("rb") as source:
        assert read(source, 8) == b"PABMAC1\0"
        width, height, fps, count = struct.unpack("<IIII", read(source, 16))
        assert fps in (15, 30) and count == fps * 6
        assert 0 < width <= 8192 and 0 < height <= 8192 and width * height <= 33554432
        packets = []
        for index in range(count):
            size, number, captured, key = struct.unpack("<IQQI", read(source, 24))
            assert 0 < size <= 16777216 and number == index and key in (0, 1)
            packets.append(read(source, size))
        assert not source.read(1)
    return width, height, fps, packets


def capture(worker, folder, prefix):
    screens = json.loads(subprocess.check_output([str(worker), "inventory"], timeout=10))
    screen = next(s for s in screens if s["primary"])
    for fps in (15, 30):
        destination = folder / f"{prefix}-{fps}.pabvideo"
        # Never replace a prior fixture or record a user's actual desktop.
        with destination.open("xb") as output:
            process, timer = spawn(worker, ["capture", screen["id"], str(fps), "synthetic"])
            try:
                magic, width, height = struct.unpack("<III", read(process.stdout, 12))
                assert magic == 0x31424150
                assert [width, height] == [screen["pixel_width"], screen["pixel_height"]]
                output.write(b"PABMAC1\0" + struct.pack("<IIII", width, height, fps, fps * 6))
                for index in range(fps * 6):
                    process.stdin.write(b"\1")
                    process.stdin.flush()
                    header = read(process.stdout, 24)
                    size, number, captured, key = struct.unpack("<IQQI", header)
                    assert 0 < size <= 16777216 and number == index and key in (0, 1)
                    assert index or key == 1
                    output.write(header)
                    output.write(read(process.stdout, size))
                process.stdin.write(b"\0")
                process.stdin.flush()
                out, err = process.communicate(timeout=5)
                assert process.returncode == 0 and not out, err.decode(errors="replace")
            finally:
                retire(process, timer)
        print(json.dumps({"fixture": destination.name, "sha256": hashlib.sha256(destination.read_bytes()).hexdigest()}), flush=True)


def render(worker, folder, source, rotation, verify, decoder, lifecycle=False):
    width, height, fps, packets = fixture(folder / f"{source}.pabvideo")
    mode = "stream-verify" if verify else "stream"
    if decoder == "software":
        mode += "-software"
    elif decoder == "failure":
        mode += "-fail-decoder"
    if lifecycle:
        mode += "-lifecycle"
        packets = packets[:1]
    process, timer = spawn(worker, ["render", str(width), str(height), str(fps), mode, "0", str(rotation)])
    state = {"consumed": 0, "presented": 0}
    condition = threading.Condition()
    report = {"source": source, "rotation": rotation, "verify": verify, "decoder": decoder, "lifecycle": lifecycle,
              "size": [width, height], "target_fps": fps, "passed": False,
              "fixture_sha256": hashlib.sha256((folder / f"{source}.pabvideo").read_bytes()).hexdigest()}

    def receive():
        try:
            while True:
                event = read(process.stdout, 1)[0]
                if event == 4:
                    kind = read(process.stdout, 1)[0]
                    with condition:
                        assert "decoder" not in state and kind in (2, 3)
                        state["decoder"] = kind
                        condition.notify_all()
                elif event in (1, 2):
                    index = struct.unpack("<Q", read(process.stdout, 8))[0]
                    with condition:
                        key = "consumed" if event == 1 else "presented"
                        assert index == state[key]
                        state[key] += 1
                        condition.notify_all()
                elif event == 3:
                    displayed, checks, error = struct.unpack("<QQI", read(process.stdout, 20))
                    with condition:
                        state["summary"] = {"displayed": displayed, "checks": checks, "max_rgb_error": error}
                        state["ended"] = time.monotonic()
                        condition.notify_all()
                    return
                else:
                    raise RuntimeError("Invalid renderer event")
        except Exception as error:
            with condition:
                state["error"] = repr(error)
                condition.notify_all()

    def until(predicate):
        with condition:
            assert condition.wait_for(lambda: state.get("error") or predicate(), timeout=10), "Renderer timeout"
            if state.get("error"):
                raise RuntimeError(state["error"])

    thread = None
    try:
        assert struct.unpack("<II", read(process.stdout, 8)) == (0x31424150, 2)
        thread = threading.Thread(target=receive, daemon=True)
        thread.start()
        start = timeline = time.monotonic()
        for index, packet in enumerate(packets):
            until(lambda: index - state["presented"] < 4)
            delay = timeline + index / fps - time.monotonic()
            if delay > 0:
                time.sleep(delay)
            process.stdin.write(struct.pack("<IQ", len(packet), index) + packet)
            process.stdin.flush()
            until(lambda: state["consumed"] == index + 1)
            if index + 1 == fps:
                start = time.monotonic()
        process.stdin.write(struct.pack("<I", 0))
        process.stdin.flush()
        until(lambda: "summary" in state)
        assert process.wait(timeout=5) == 0
        summary = state["summary"]
        measured = max(0, summary["displayed"] - fps) / (state["ended"] - start)
        report.update(display_fps=None if lifecycle else measured, summary=summary)
        assert state["consumed"] == state["presented"] == len(packets)
        assert summary["displayed"] <= len(packets)
        assert summary["checks"] == (len(packets) * 4 if verify else 0) + (12 if lifecycle else 0)
        assert summary["max_rgb_error"] <= 12
        if not lifecycle:
            assert measured > 12 and measured >= fps * .9
        else:
            assert summary["displayed"] == 1
        assert state["decoder"] == (3 if decoder == "hardware" else 2)
        report["passed"] = True
    except Exception as error:
        report["error"] = repr(error)
    finally:
        retire(process, timer)
        if thread:
            thread.join(timeout=2)
        report["stderr"] = process.stderr.read().decode(errors="replace")
    if decoder == "failure" and "Diagnostic initial decoder failure" not in report["stderr"]:
        report.update(passed=False, error="Injection did not follow a successful hardware decode")
    if lifecycle and "lifecycle=resize_minimize_restore_close_passed" not in report["stderr"]:
        report.update(passed=False, error="Lifecycle diagnostic did not finish")
    return report


def invalid(worker, folder):
    width, height, fps, packets = fixture(folder / "synthetic-15.pabvideo")
    def frame(packet, index=0):
        return struct.pack("<IQ", len(packet), index) + packet
    cases = [
        ("oversize", struct.pack("<I", 16777217), "packet outside bounds", width),
        ("truncated", struct.pack("<IQ", 100, 0) + b"123", "Parent pipe ended", width),
        ("sequence", frame(packets[0], 1), "sequence mismatch", width),
        ("prefix", frame(b"not-annex-b"), "prefix missing", width),
        ("empty-nal", frame(b"\0\0\1\0\0\1"), "Invalid H264 NAL", width),
        ("forbidden-bit", frame(b"\0\0\1\xe5\x88"), "Invalid H264 NAL", width),
        ("no-idr", frame(packets[1]), "requires an IDR", width),
        ("geometry", frame(packets[0]), "dimensions changed", width - 2),
        ("duplicate", frame(packets[0]) + frame(packets[1], 0), "sequence mismatch", width),
    ]
    for software in (False, True):
        for name, wire, message, render_width in cases:
            process, timer = spawn(worker, ["render", str(render_width), str(height), str(fps),
                                          "stream-software" if software else "stream"])
            report = {"case": name, "software": software, "passed": False}
            try:
                out, err = process.communicate(wire, timeout=15)
                assert process.returncode != 0 and message in err.decode(errors="replace")
                # No acceptance/output credit for the invalid frame.
                expected = struct.pack("<II", 0x31424150, 2)
                if name == "duplicate":
                    expected += bytes([4, 2 if software else 3, 1]) + struct.pack("<Q", 0)
                    expected += bytes([2]) + struct.pack("<Q", 0)
                assert out == expected
                report["passed"] = True
            except Exception as error:
                report["error"] = repr(error)
            finally:
                retire(process, timer)
            yield report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["capture", "render"])
    parser.add_argument("worker", type=Path)
    parser.add_argument("folder", type=Path)
    parser.add_argument("--prefix", choices=["synthetic", "windows"], default="synthetic")
    parser.add_argument("--suite", choices=["rotations", "software", "failure", "invalid", "lifecycle"], default="rotations")
    args = parser.parse_args()
    args.folder.mkdir(parents=True, exist_ok=True)
    if args.action == "capture":
        capture(args.worker.resolve(), args.folder, args.prefix)
        return
    if args.suite == "invalid":
        results = invalid(args.worker.resolve(), args.folder)
    else:
        if args.suite == "rotations":
            cases = [("synthetic-15", r, True, "hardware") for r in (0, 90, 180, 270)]
            cases += [("windows-30", 0, v, "hardware") for v in (True, False)]
        elif args.suite == "lifecycle":
            cases = [("synthetic-15", r, True, d, True) for r in (0, 90, 180, 270) for d in ("hardware", "software")]
        else:
            cases = [(s, 0, True, args.suite) for s in ("synthetic-15", "synthetic-30", "windows-15", "windows-30")]
        results = (render(args.worker.resolve(), args.folder, *case) for case in cases)
    reports = []
    digest = hashlib.sha256(args.worker.read_bytes()).hexdigest()
    for report in results:
        report["worker_sha256"] = digest
        reports.append(report)
        print(json.dumps(report), flush=True)
        (args.folder / f"mac-render-{args.suite}.json").write_text(json.dumps(reports, indent=2), encoding="utf-8")
    raise SystemExit(0 if all(r["passed"] for r in reports) else 1)


if __name__ == "__main__":
    main()
