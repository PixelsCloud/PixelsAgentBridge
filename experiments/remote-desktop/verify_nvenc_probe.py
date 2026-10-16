"""Validate GPU encode output using synthetic images; optional desktop capture.

FFmpeg is an offline test dependency, not a production media backend. It uses
software decoding here to independently check bitstream contents and timing.
"""
import argparse
from datetime import datetime, timezone
from pathlib import Path
import json
import subprocess
import uuid


def run(command):
    return subprocess.run([str(x) for x in command], capture_output=True,
                          text=True, encoding="utf-8", timeout=90, check=True).stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--capture", action="store_true",
                        help="Also measure real desktop encoding without saving video")
    parser.add_argument("--display", type=int, default=0)
    parser.add_argument("--animate", action="store_true",
                        help="Show a short-lived nonactivating pattern window during capture")
    args = parser.parse_args()
    if args.animate and not args.capture:
        parser.error("--animate requires --capture")
    root = Path(__file__).resolve().parents[2]
    binary = root / ".build/remote-desktop/nvenc/nvenc-probe.exe"
    output = root / ".build/remote-desktop/nvenc" / ("verify-" + uuid.uuid4().hex[:12])
    output.mkdir(parents=True)
    report = {"created_at": datetime.now(timezone.utc).isoformat(),
              "production_hardware_decoder_tested": False,
              "renderer_tested": False, "cases": []}
    for fps in (15, 30):
        video = output / f"synthetic-{fps}.h264"
        encoded = json.loads(run([binary, "--fps", fps, "--seconds", 5,
                                  "--display", args.display, "--output", video]))
        info = json.loads(run(["ffprobe", "-v", "error", "-count_frames",
                               "-show_streams", "-of", "json", video]))["streams"][0]
        decoded = run(["ffmpeg", "-v", "error", "-i", video, "-threads", 2,
                       "-f", "framemd5", "-"])
        hashes = [line.split(",")[-1].strip() for line in decoded.splitlines()
                  if line and not line.startswith("#")]
        assert info["codec_name"] == "h264" and info["has_b_frames"] == 0, info
        assert (info["width"], info["height"]) == (1920, 1080), info
        assert len(hashes) == encoded["encoded_frames"] == int(info["nb_read_frames"])
        assert len(set(hashes)) == len(hashes), "Repeated decoded synthetic frame"
        assert encoded["encoded_fps"] > 12, encoded
        assert encoded["encoded_fps"] >= fps * .9, encoded
        assert all(encoded[k] == 0 for k in (
            "probe_pixel_cpu_readbacks", "probe_gpu_copies", "probe_conversions"))
        report["cases"].append({"encode": encoded, "software_decoded_frames": len(hashes),
                                "distinct_decoded_frames": len(set(hashes)), "b_frames": 0})
    if args.capture:
        report["desktop_encode"] = json.loads(run([
            binary, "--capture", "--display", args.display, "--seconds", 5,
            *(["--animate", "--fps", 30] if args.animate else [])]))
        if args.animate:
            assert report["desktop_encode"]["encoded_fps"] > 12, report["desktop_encode"]
        # An arbitrary desktop may be static; do not impose a dynamic-scene FPS
        # acceptance threshold or save its pixels in the validation artifacts.
    forbidden = output / "forbidden.h264"
    for arguments in (["--fps", "0"], ["--seconds", "0"], ["--display", "129"],
                      ["--animate"],
                      ["--capture", "--output", str(forbidden)]):
        rejected = subprocess.run([str(binary), *arguments], capture_output=True, timeout=10)
        assert rejected.returncode != 0, arguments
    assert not forbidden.exists()
    before = (output / "synthetic-15.h264").read_bytes()
    rejected = subprocess.run([str(binary), "--output", str(output / "synthetic-15.h264")],
                              capture_output=True, timeout=10)
    assert rejected.returncode != 0 and (output / "synthetic-15.h264").read_bytes() == before
    report["invalid_arguments_and_output_preservation"] = "passed"
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"report": str(output / "report.json"), "passed": True,
                      "encode_fps": [x["encode"]["encoded_fps"] for x in report["cases"]]}))


if __name__ == "__main__":
    main()
