"""Exercise real native media through local reliable QUIC. No video is saved."""
import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import subprocess
import uuid


def validate(report, verify=False):
    assert report["stage"] == "reliable_native_media"
    assert report["media_gpu_texture_copies"] == report["media_pixel_cpu_readbacks"] == 0
    assert report["display_confirmation"] == "dxgi_frame_statistics_each_present_id"
    assert len(report["cases"]) == 2
    for generation, case in enumerate(report["cases"], 1):
        assert case["generation"] == generation
        assert case["display"]["displayed"] == case["frames"] + case["warmup_frames"]
        assert case["effective_display_fps"] > 12
        assert case["effective_display_fps"] >= report["target_fps"] * .9
        assert case["parallel_legacy_max_ms"] < 1000
        if verify:
            assert case["display"]["test_pixel_checks"] == case["display"]["displayed"] * 4
            assert case["display"]["test_max_rgb_error"] <= 12
        else:
            assert case["display"]["test_pixel_checks"] == 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    helper = root / ".build/remote-desktop/stream-media/stream-media-probe.exe"
    folder = root / ".build/remote-desktop/stream-media" / ("verify-" + uuid.uuid4().hex[:12])
    folder.mkdir()
    report = {"created_at": datetime.now(timezone.utc).isoformat(), "passed": False, "cases": []}
    try:
        for mode, fps, verify in [("synthetic", 15, True), ("synthetic", 30, False),
                                  ("animate", 15, False), ("animate", 30, False)]:
            run = subprocess.run([str(args.binary.resolve()), "loopback", str(helper), str(fps), str(fps*5),
                                  mode, "verify" if verify else "normal", "restart"],
                                 capture_output=True, text=True, encoding="utf-8", timeout=85)
            (folder / f"{mode}-{fps}.stderr.txt").write_text(run.stderr, encoding="utf-8")
            if run.returncode:
                raise RuntimeError(f"{mode}/{fps}: {run.stderr.strip()}")
            data = json.loads(run.stdout)
            report["cases"].append({"source": mode, "verify_pixels": verify, "metrics": data})
            validate(data, verify)
            print(json.dumps({"source": mode, "fps": fps, "display_fps": [c["effective_display_fps"] for c in data["cases"]]}), flush=True)
        report["passed"] = True
    finally:
        (folder / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        print(json.dumps({"report": str(folder / "report.json"), "passed": report["passed"]}), flush=True)


if __name__ == "__main__":
    main()
