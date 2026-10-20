#!/usr/bin/env python3
"""Run isolated Android prototype checks on an explicitly selected device."""

from __future__ import annotations

import argparse
import base64
import json
from pathlib import Path
import re
import os
import queue
import subprocess
import threading
import time
import uuid

ROOT = Path(__file__).resolve().parents[1]
PACKAGE = "com.pixels.agent.debug"


def adb(serial: str, *arguments: str, check: bool = True, timeout: int = 30) -> subprocess.CompletedProcess[str]:
    return subprocess.run(["adb", "-s", serial, *arguments], text=True, encoding="utf-8",
                          errors="replace", capture_output=True, check=check, timeout=timeout)


def control_test(serial: str, result_dir: Path) -> None:
    executable = ROOT / "target" / "debug" / "examples" / (
        "control-fixture.exe" if os.name == "nt" else "control-fixture")
    if not executable.is_file():
        raise SystemExit("Build the isolated fixture with: python scripts/build_android.py control-fixture")
    # The server binds loopback only. ADB reverse is a test tunnel, never part of the app.
    server = subprocess.Popen([str(executable)], stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                              text=True, encoding="utf-8")
    lines: queue.Queue[str] = queue.Queue()
    events: list[dict] = []

    def consume() -> None:
        assert server.stdout is not None
        for line in server.stdout:
            lines.put(line)
            try:
                events.append(json.loads(line))
            except json.JSONDecodeError:
                pass

    reader = threading.Thread(target=consume, daemon=True)
    reader.start()
    port = None
    try:
        ready = json.loads(lines.get(timeout=10))
        if not ready.get("ready"):
            raise RuntimeError("TLS fixture did not start")
        port = ready["port"]
        adb(serial, "reverse", f"tcp:{port}", f"tcp:{port}")
        ca = base64.b64encode(ready["ca_pem"].encode("utf-8")).decode("ascii")
        # Let the emulator finish removing the old task before instrumentation creates
        # a replacement process. Otherwise its task watchdog kills the new process.
        adb(serial, "shell", "am", "force-stop", PACKAGE)
        time.sleep(1)
        try:
            result = adb(serial, "shell", "am", "instrument", "-w", "-e", "class",
                         "com.pixels.agent.ConnectionDeviceTest", "-e", "fixture_port", str(port),
                         "-e", "fixture_ca", ca,
                         f"{PACKAGE}.test/androidx.test.runner.AndroidJUnitRunner", timeout=90)
        except subprocess.TimeoutExpired as error:
            output = error.stdout or b""
            if isinstance(output, bytes):
                output = output.decode("utf-8", errors="replace")
            (result_dir / "control-instrumentation.log").write_text(output, encoding="utf-8")
            raise SystemExit("Control instrumentation did not finish within 90 seconds") from error
        (result_dir / "control-instrumentation.log").write_text(result.stdout, encoding="utf-8")
        if not re.search(r"OK \(1 test\)", result.stdout) or "FAILURES" in result.stdout:
            print(result.stdout)
            raise SystemExit("Android control connection acceptance failed")
        time.sleep(0.2)
        names = [event.get("event") for event in events]
        summary = {"status": "passed", "registered": names.count("registered"),
                   "authenticated": names.count("authenticated"),
                   "forced_disconnect": names.count("forced_disconnect"),
                   "tls_rejected": names.count("rejected")}
        if summary["registered"] != 1 or summary["authenticated"] != 3 or summary["forced_disconnect"] != 1 or summary["tls_rejected"] < 1:
            raise SystemExit(f"Fixture events did not match expected identity reuse: {summary}")
        (result_dir / "control-acceptance.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")
        print(json.dumps(summary))
    finally:
        if port is not None:
            adb(serial, "reverse", "--remove", f"tcp:{port}", check=False)
        server.terminate()
        try:
            server.wait(timeout=5)
        except subprocess.TimeoutExpired:
            server.kill()
            server.wait(timeout=5)
        reader.join(timeout=2)
        (result_dir / "control-fixture-events.json").write_text(
            json.dumps([event for event in events if "event" in event], indent=2), encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["smoke", "instrument", "control"])
    parser.add_argument("--serial", default="emulator-5554")
    args = parser.parse_args()
    result_dir = ROOT / ".build" / "android"
    result_dir.mkdir(parents=True, exist_ok=True)
    if adb(args.serial, "get-state").stdout.strip() != "device":
        raise SystemExit("The selected device is not connected and authorized")
    if args.action == "control":
        control_test(args.serial, result_dir)
        return
    if args.action == "instrument":
        adb(args.serial, "shell", "am", "force-stop", PACKAGE)
        time.sleep(1)
        try:
            result = adb(args.serial, "shell", "am", "instrument", "-w", "-e", "class",
                         "com.pixels.agent.AutomationDeviceTest",
                         f"{PACKAGE}.test/androidx.test.runner.AndroidJUnitRunner", timeout=60)
        except subprocess.TimeoutExpired as error:
            output = error.stdout or b""
            if isinstance(output, bytes):
                output = output.decode("utf-8", errors="replace")
            (result_dir / "instrumentation.log").write_text(output, encoding="utf-8")
            raise SystemExit("Instrumentation did not finish within 60 seconds; inspect the emulator process lifecycle") from error
        (result_dir / "instrumentation.log").write_text(result.stdout, encoding="utf-8")
        matched = re.search(r"OK \((\d+) tests?\)", result.stdout)
        if not matched or int(matched[1]) < 1 or "FAILURES" in result.stdout:
            print(result.stdout)
            raise SystemExit("Android instrumentation did not pass")
        print(f"Android instrumentation passed: {matched[1]} tests")
        return
    request_id = str(uuid.uuid4())
    adb(args.serial, "shell", "am", "start", "-f", "0x10008000", "-n",
        f"{PACKAGE}/com.pixels.agent.fixture.AutomationFixtureActivity",
        "--ez", "run_smoke_probe", "true", "--es", "smoke_request_id", request_id)
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        completed = adb(args.serial, "shell", "run-as", PACKAGE, "cat",
                        "files/automation-smoke.json", check=False)
        try:
            result = json.loads(completed.stdout)
        except json.JSONDecodeError:
            time.sleep(0.2)
            continue
        if result.get("request_id") != request_id or result.get("status") == "running":
            time.sleep(0.2)
            continue
        (result_dir / "automation-smoke.json").write_text(
            json.dumps(result, indent=2), encoding="utf-8")
        print(json.dumps(result))
        if result.get("status") != "passed":
            raise SystemExit("Android accessibility smoke probe failed")
        return
    raise SystemExit("Timed out waiting for this smoke request; no previous result was accepted")


if __name__ == "__main__":
    main()
