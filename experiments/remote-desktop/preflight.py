"""Read-only preparation check; never imports or executes upstream build scripts."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tomllib


def command(*args: str) -> str:
    result = subprocess.run(args, capture_output=True, text=True, timeout=30)
    if result.returncode:
        raise RuntimeError(f"{args[0]} exited with {result.returncode}")
    return result.stdout.strip()


def checkout(path: Path, expected: str) -> dict:
    actual = command("git", "-C", str(path), "rev-parse", "HEAD")
    dirty = bool(command("git", "-C", str(path), "status", "--porcelain"))
    return {"path": str(path), "expected": expected, "actual": actual,
            "clean": not dirty, "matches": actual == expected and not dirty}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rustdesk", type=Path, required=True)
    parser.add_argument("--hwcodec", type=Path, required=True)
    args = parser.parse_args()
    pins = tomllib.loads(Path(__file__).with_name("sources.toml").read_text(encoding="utf-8"))
    rustdesk, hwcodec = args.rustdesk.resolve(), args.hwcodec.resolve()
    report: dict = {"checks": {}, "native_build_ready": False, "compiled": False,
                    "captured_desktop": False, "max_total_build_jobs": pins["build"]["max_total_jobs"]}
    try:
        report["checks"]["rustdesk"] = checkout(rustdesk, pins["rustdesk"]["commit"])
        report["checks"]["hbb_common"] = checkout(rustdesk / "libs/hbb_common", pins["rustdesk"]["hbb_common_commit"])
        report["checks"]["hwcodec"] = checkout(hwcodec, pins["hwcodec"]["commit"])
        lock = tomllib.loads((rustdesk / "Cargo.lock").read_text(encoding="utf-8"))
        source = next(p["source"] for p in lock["package"] if p["name"] == "hwcodec")
        report["checks"]["hwcodec_lock"] = {"matches": source.endswith("#" + pins["hwcodec"]["commit"])}
        external = command("git", "-C", str(hwcodec), "ls-tree", "HEAD", "externals").split()
        report["checks"]["hwcodec_externals_pin"] = {"matches":
            len(external) == 4 and external[2] == pins["hwcodec"]["externals_commit"]}
        report["hwcodec_submodules"] = command("git", "-C", str(hwcodec), "submodule", "status")
        build_text = (rustdesk / "build.py").read_text(encoding="utf-8")
        report["checks"]["libdrmtap_pin"] = {"matches":
            f"LIBDRMTAP_SHA_PINNED = '{pins['libdrmtap']['commit']}'" in build_text}
        report["tools"] = {}
        for name in ("rustc", "cargo", "cmake", "clang", "ninja"):
            path = shutil.which(name)
            report["tools"][name] = {"available": bool(path), "path": path}
            if path:
                report["tools"][name]["version"] = command(path, "--version").splitlines()[0]
        report["build_paths"] = {name: {"configured": bool(os.environ.get(name)),
            "exists": bool(os.environ.get(name)) and Path(os.environ[name]).is_dir()}
            for name in ("VCPKG_ROOT", "VCPKG_INSTALLED_ROOT", "LIBCLANG_PATH")}
        report["upstream_notes"] = [
            "scrap build.rs requires libvpx, libyuv and aom before trimming",
            "pinned hwcodec build.rs builds FFmpeg even with vram enabled",
            "encoder/decoder/renderer runtime capability and native libraries remain unverified",
        ]
        report["reference_sources_match"] = all(v["matches"] for v in report["checks"].values())
    except (OSError, RuntimeError, subprocess.TimeoutExpired, ValueError, KeyError, StopIteration) as error:
        report["error"] = str(error)
        report["reference_sources_match"] = False
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0 if report["reference_sources_match"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
