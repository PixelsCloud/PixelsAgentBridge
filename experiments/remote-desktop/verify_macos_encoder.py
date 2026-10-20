"""Generated-only Mac encoder fallback and native decoder interoperability.

Run in an authorized GUI session: verify_macos_encoder.py WORKER NEW_DIRECTORY.
Briefly shows generated quadrant windows. Never captures the user's desktop,
sends input, changes TCC, or replaces an installed application.
"""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import time

from verify_macos_renderer import read, render, retire, spawn


def capture_case(worker, folder, mode):
    screens = json.loads(subprocess.check_output([str(worker), "inventory"], timeout=10))
    screen = next(s for s in screens if s["primary"])
    fps, total = 15, 90
    path = folder / (mode + ".pabvideo")
    process, timer = spawn(worker, ["capture", screen["id"], str(fps), mode])
    report = {"mode": mode, "passed": False, "source": "generated quadrants"}
    try:
        magic, width, height = struct.unpack("<III", read(process.stdout, 12))
        assert magic == 0x31424150 and [width, height] == [screen["pixel_width"], screen["pixel_height"]]
        report["dimensions"] = [width, height]
        last_time = 0
        keys = []
        start = time.monotonic()
        with path.open("xb") as output:
            output.write(b"PABMAC1\0" + struct.pack("<IIII", width, height, fps, total))
            for index in range(total):
                process.stdin.write(b"\1"); process.stdin.flush()
                header = read(process.stdout, 24)
                size, number, sampled, key = struct.unpack("<IQQI", header)
                assert 0 < size <= 16777216 and number == index and sampled >= last_time and key in (0, 1)
                packet = read(process.stdout, size)
                if index == 0 or mode == "synthetic-fail-encoder" and index == 2:
                    assert key == 1
                    types = {nal[0] & 31 for nal in packet.split(b"\0\0\0\1")[1:] if nal}
                    assert {5, 7, 8} <= types, "Fallback must publish SPS, PPS and IDR together"
                if key: keys.append(index)
                output.write(header); output.write(packet)
                last_time = sampled
                if index + 1 == fps: start = time.monotonic()
            elapsed = time.monotonic() - start
        process.stdin.write(b"\0"); process.stdin.flush()
        out, err = process.communicate(timeout=5)
        assert process.returncode == 0 and not out
        report["diagnostic"] = err.decode(errors="replace")
        expected = {
            "synthetic": "encoder=videotoolbox_hardware",
            "synthetic-software": "encoder=openh264_software",
            "synthetic-fail-encoder-init": "Diagnostic encoder initialization failure",
            "synthetic-fail-encoder": "Diagnostic running encoder failure",
        }[mode]
        assert expected in report["diagnostic"]
        if mode == "synthetic-fail-encoder":
            assert report["diagnostic"].count("encoder fallback=openh264_software") == 1
        rate = (total - fps) / elapsed
        report.update(frames=total, keyframes=keys, encoder_fps=rate,
                      fixture_sha256=hashlib.sha256(path.read_bytes()).hexdigest())
        assert rate > 12 and rate >= fps * .9
        report["passed"] = True
    except Exception as error:
        report["error"] = repr(error)
    finally:
        retire(process, timer)
        if "diagnostic" not in report:
            report["diagnostic"] = process.stderr.read().decode(errors="replace")
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("worker", type=Path)
    parser.add_argument("directory", type=Path)
    args = parser.parse_args()
    worker = args.worker.resolve(strict=True)
    folder = args.directory.resolve()
    folder.mkdir(parents=True, exist_ok=False)
    report = {"worker_sha256": hashlib.sha256(worker.read_bytes()).hexdigest(), "capture": [], "render": [],
              "actual_screen_capture_tested": False, "product_network_tested": False}
    def save():
        (folder / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    for mode in ("synthetic", "synthetic-software", "synthetic-fail-encoder-init", "synthetic-fail-encoder"):
        case = capture_case(worker, folder, mode)
        report["capture"].append(case); save(); print(json.dumps(case), flush=True)
        if not case["passed"]: raise SystemExit(1)
    for case in report["capture"]:
        for decoder in ("hardware", "software"):
            result = render(worker, folder, case["mode"], 0, True, decoder)
            report["render"].append(result); save(); print(json.dumps(result), flush=True)
    report["passed"] = all(case["passed"] for case in report["render"])
    save()
    raise SystemExit(0 if report["passed"] else 1)


if __name__ == "__main__":
    main()
