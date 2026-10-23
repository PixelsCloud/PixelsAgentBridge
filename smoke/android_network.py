"""Run existing Android network acceptance with explicit operator config and state restoration.

Requires an already authenticated dedicated operator; never registers an account,
installs an APK, or uses a built-in deployment address. Device pairing uses stdin.
"""
from pathlib import Path
import argparse
import json
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
sys.path.insert(0, str(ROOT))
from android_home import Home, normal_status
from test_android_remote import installed_apk_sha256, environment
from smoke.run import digest, redact


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial", required=True)
    parser.add_argument("--operator-config", type=Path, required=True)
    parser.add_argument("--apk", type=Path, required=True)
    parser.add_argument("--mcp", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    if not output.is_relative_to((ROOT / "smoke/runs").resolve()) or output.exists():
        raise ValueError("Select a new directory under smoke/runs")
    apk_hash = digest(args.apk)
    if installed_apk_sha256(args.serial) != apk_hash:
        raise ValueError("Installed APK differs; no implicit install is permitted")
    config, mcp = args.operator_config.resolve(strict=True), args.mcp.resolve(strict=True)
    before = normal_status(args.serial)
    if not before.get("accessibility_ready"):
        raise ValueError("Prepare accessibility before this test")
    output.mkdir(parents=True)
    result = {"status": "unconfirmed", "apk_sha256": apk_hash, "mcp_sha256": digest(mcp),
              "cleanup": False, "suites": []}
    home = Home(args.serial)
    try:
        home.start()
        home.set_paused(False)
        code, password = home.revealed_credentials()
        home.click("hide", already="show")
        pairing = subprocess.run([str(mcp), "--config", str(config), "device", "connect", code,
                                  "--password-stdin"], input=password, capture_output=True,
                                 encoding="utf-8", env=environment(True), timeout=60)
        if pairing.returncode:
            result["pairing_error"] = redact((pairing.stdout + pairing.stderr).replace(password, "[REDACTED]"))[-3000:]
            del password
            raise RuntimeError("Normal pairing failed; verify the dedicated operator's login and trust configuration")
        del password
        for suite in ("execution", "port_forward"):
            home.start()
            home.set_paused(False)
            with (output / (suite + ".log")).open("w", encoding="utf-8") as log:
                completed = subprocess.run([sys.executable, str(ROOT / f"scripts/test_android_{suite}.py"),
                    "--serial", args.serial, "--operator-config", str(config), "--device-code", code,
                    "--apk-sha256", apk_hash, "--mcp-executable", str(mcp), "--output-root", str(output / suite)],
                    stdout=log, stderr=subprocess.STDOUT)
            reports = list((output / suite).glob("*/result.json"))
            if len(reports) != 1:
                raise RuntimeError(f"Missing fresh result: {suite}")
            observed = json.loads(reports[0].read_text(encoding="utf-8"))
            checks = observed.get("checks", [])
            ok = (completed.returncode == 0 and observed.get("status") == "passed" and checks
                  and all(c.get("passed") is True for c in checks) and not observed.get("cleanup_pending")
                  and not observed.get("cleanup_errors"))
            result["suites"].append({"name": suite, "passed": bool(ok), "checks": len(checks),
                                     "report": str(reports[0].relative_to(output))})
            if not ok:
                raise RuntimeError(f"Android {suite} did not pass; inspect its local evidence")
        result["status"] = "pass"
    finally:
        try:
            home.start()
            home.set_paused(bool(before.get("paused")))
            home.click("hide", already="show")
            if not before.get("connection_requested"):
                home.click("disconnect")
            after = normal_status(args.serial)
            result["cleanup"] = all(after.get(k) == before.get(k) for k in ("paused", "connection_requested"))
        finally:
            if not result["cleanup"]:
                result["status"] = "unconfirmed"
            (output / "result.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
    return 0 if result["status"] == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
