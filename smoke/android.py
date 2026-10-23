"""Run the owned Android remote fixture and restore the ordinary connection afterwards.

Uses ADB reverse and a real local TLS Relay, not a public-network acceptance claim.
No installation, data clearing, signing change or account-settings rewrite.
"""
from pathlib import Path
import argparse
import hashlib
import json
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
from android_home import Home, normal_status
from test_android_remote import PACKAGE, adb, installed_apk_sha256


def run(serial, apk, mcp, fixture, output):
    output = Path(output).resolve()
    if not output.is_relative_to((ROOT / "smoke/runs").resolve()):
        raise ValueError("Android evidence must stay under smoke/runs")
    for file in (apk, mcp, fixture):
        if not Path(file).is_file():
            raise ValueError("Supply existing APK, MCP and remote-fixture artifacts")
    apk_hash = hashlib.sha256(Path(apk).read_bytes()).hexdigest()
    if installed_apk_sha256(serial) != apk_hash:
        raise ValueError("Installed APK differs; perform a data-preserving signed upgrade separately")
    before = normal_status(serial)
    if before.get("paused") or not before.get("accessibility_ready"):
        raise ValueError("Prepare an unpaused test device with accessibility enabled")
    output.mkdir(parents=True, exist_ok=True)
    result = {"status": "unconfirmed", "apk_sha256": apk_hash, "cleanup": False}
    counts = {"passed": 0, "failed": 0, "skipped": 0}
    try:
        adb(serial, "shell", "am", "start", "-f", "0x14000000", "-n", f"{PACKAGE}/com.pixels.agent.MainActivity")
        home = Home(serial)
        home.click("hide", already="show")
        if before.get("connection_requested"):
            home.click("disconnect")
            deadline = time.monotonic() + 15
            while normal_status(serial).get("remote_ready") and time.monotonic() < deadline:
                time.sleep(.2)
            if normal_status(serial).get("remote_ready"):
                raise RuntimeError("Ordinary disconnect did not complete")
        with (output / "android.log").open("w", encoding="utf-8") as log:
            subprocess.run([sys.executable, str(ROOT / "scripts/test_android_remote.py"),
                "--serial", serial, "--relay-only", "--apk", str(Path(apk).resolve()),
                "--mcp-executable", str(Path(mcp).resolve()), "--fixture-executable", str(Path(fixture).resolve()),
                "--output-root", str(output / "fixture")], check=True, stdout=log, stderr=subprocess.STDOUT)
        reports = list((output / "fixture").glob("*/result.json"))
        if len(reports) != 1:
            raise RuntimeError("Expected one fresh fixture result")
        observed = json.loads(reports[0].read_text(encoding="utf-8"))
        checks = ["normal_device_pairing", "wrong_password_rejected", "android_identity", "unicode_input",
                  "native_click", "request_deduplicated", "request_conflict_rejected",
                  "cross_connection_reference_rejected", "foreign_operation_rejected", "native_gesture",
                  "operation_query", "completed_action_not_rolled_back", "remote_screenshot",
                  "home_navigation", "background_observation", "original_operation_after_reconnect"]
        if (observed.get("status") != "passed" or observed.get("connection_path") != "relay"
                or observed["provenance"]["apk_sha256"] != apk_hash
                or not all(observed.get(key) is True for key in checks)):
            raise RuntimeError("Missing Android business postconditions")
        counts["passed"] = len(checks)
        result.update(status="pass", fixture_result=str(reports[0].relative_to(output)))
    except BaseException:
        counts["failed"] += 1
        result["status"] = "fail"
        raise
    finally:
        try:
            if before.get("connection_requested"):
                Home(serial).start()
            after = normal_status(serial)
            result["cleanup"] = (after.get("connection_requested") == before.get("connection_requested")
                and after.get("paused") == before.get("paused")
                and (not before.get("remote_ready") or after.get("remote_ready")))
            if not result["cleanup"]:
                counts["failed"] += 1
                result["status"] = "unconfirmed"
        except BaseException:
            counts["failed"] += 1
            result["status"] = "unconfirmed"
            raise
        finally:
            (output / "android-result.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
            (output / "counts.json").write_text(json.dumps(counts), encoding="utf-8")
    return 0 if result["status"] == "pass" else 1


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("serial", "apk", "mcp", "fixture", "output"):
        parser.add_argument("--" + name, required=True)
    args = parser.parse_args()
    raise SystemExit(run(args.serial, args.apk, args.mcp, args.fixture, args.output))
