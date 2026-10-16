"""Read-only Linux display/codec prerequisites; never opens/captures a screen."""
from datetime import datetime, timezone
import json
from pathlib import Path
import platform
import shutil
import subprocess


def command(argv):
    try:
        result = subprocess.run(argv, capture_output=True, text=True, timeout=15)
        return {"exit_code": result.returncode, "stdout": result.stdout.strip(),
                "stderr": result.stderr.strip()}
    except (OSError, subprocess.TimeoutExpired) as error:
        return {"error": type(error).__name__}


report = {
    "schema_version": 1,
    "sampled_at": datetime.now(timezone.utc).isoformat(),
    "os": platform.system(), "kernel": platform.release(),
    "architecture": platform.machine(),
    "distribution": Path("/etc/os-release").read_text(),
    "tools": {name: bool(shutil.which(name)) for name in (
        "cargo", "rustc", "cc", "cmake", "ffmpeg", "vainfo", "nvidia-smi")},
    "drm_nodes": sorted(str(p) for p in Path("/dev/dri").glob("*")),
    "uinput_present": Path("/dev/uinput").exists(),
    "gpus": command(["lspci", "-nnk"]),
    "sessions": [], "capture_tested": False,
    "hardware_codec_tested": False, "effective_fps": None,
}
# Keep only display-controller paragraphs; don't inventory unrelated hardware.
if "stdout" in report["gpus"]:
    text = report["gpus"]["stdout"]
    blocks = []
    for line in text.splitlines():
        if not line.startswith("\t"):
            blocks.append([line])
        elif blocks:
            blocks[-1].append(line)
    report["gpus"]["stdout"] = "\n".join(
        "\n".join(block) for block in blocks
        if any(x in block[0] for x in ("VGA compatible", "3D controller", "Display controller")))
sessions = command(["loginctl", "list-sessions", "--no-legend", "--no-pager"])
if sessions.get("exit_code") == 0:
    for line in sessions["stdout"].splitlines():
        sid = line.split()[0]
        report["sessions"].append(command([
            "loginctl", "show-session", sid, "-p", "Type", "-p", "Class",
            "-p", "State", "-p", "Remote", "-p", "Active", "-p", "LockedHint"]))
else:
    report["session_query"] = sessions
report["active_compilers"] = command([
    "pgrep", "-x", "cargo|rustc|cc1|cc1plus|cmake|ninja|make"])
print(json.dumps(report, indent=2))
