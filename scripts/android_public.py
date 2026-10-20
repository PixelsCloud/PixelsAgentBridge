#!/usr/bin/env python3
"""Isolated account and MCP campaigns against the ordinary public Android connection.

Pairing reads the device password from stdin. It is never saved in this script's files
or placed in a command argument. Phone authorization and physical steps remain explicit.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time
import uuid

from android_scenarios import AndroidTasks
from service_config import write_config
from test_android_remote import ROOT, PACKAGE, account, environment, Mcp, installed_apk_sha256


class PublicMcp(Mcp):
    def __init__(self, config, code, relay_only, log, executable):
        self.code = code
        super().__init__(config, relay_only, log, executable=executable)

    def tool(self, name, args, allow_error=False):
        values = dict(args)
        if values.get("device_code") == "987654321":
            values["device_code"] = self.code
        return super().tool(name, values, allow_error)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["prepare", "pair", "campaign", "soak", "logout"])
    parser.add_argument("--run-id", required=True, type=uuid.UUID)
    parser.add_argument("--device-code")
    parser.add_argument("--serial", help="Verify the USB phone before pairing; campaigns need no ADB")
    parser.add_argument("--relay-only", action="store_true")
    parser.add_argument("--rounds", type=int, default=20)
    parser.add_argument("--seconds", type=int, default=28800)
    options = parser.parse_args()
    if not 1 <= options.rounds <= 100 or not 60 <= options.seconds <= 86400:
        parser.error("Rounds must be 1..100; soak seconds must be 60..86400")
    root = ROOT / ".build/android/public" / str(options.run_id)
    root.mkdir(parents=True, exist_ok=True)
    config = root / "mcp.toml"
    executable = root / ("pab-mcp.exe" if os.name == "nt" else "pab-mcp")
    metadata = root / "session.json"
    if options.action == "prepare":
        if metadata.exists() or config.exists():
            raise RuntimeError("This public acceptance session already exists; do not replace its login")
        staged = ROOT / ".build/android/tools" / executable.name
        shutil.copy2(staged, executable)
        write_config(config, {"control_url": "wss://pab.rgaa.vip/control",
            "relay_urls": ["https://pab-relay.rgaa.vip"], "operator": {"data_dir": root / "operator"}})
        username = "android-check-" + options.run_id.hex[:16]
        account(config, "register", uuid.uuid4().hex + uuid.uuid4().hex,
                executable=executable, username=username)
        apk = ROOT / "apps/android/app/build/outputs/apk/debug/app-debug.apk"
        metadata.write_text(json.dumps({"run_id": str(options.run_id), "username": username,
            "apk_sha256": hashlib.sha256(apk.read_bytes()).hexdigest(),
            "created_at_unix": time.time(), "default_server": True}, indent=2), encoding="utf-8")
        print(f"Isolated public account prepared; session={options.run_id}")
        return
    session = json.loads(metadata.read_text(encoding="utf-8"))
    if options.action == "logout":
        account(config, "logout", executable=executable)
        print("Isolated public acceptance account signed out")
        return
    code = options.device_code or session.get("device_code")
    if not isinstance(code, str) or len(code) != 9 or not code.isascii() or not code.isdigit():
        parser.error("Provide the real nine-digit device code")
    if options.action == "pair":
        if not options.serial or installed_apk_sha256(options.serial) != session["apk_sha256"]:
            raise ValueError("Pairing requires the USB phone with this acceptance APK")
        password = sys.stdin.read(1027).rstrip("\r\n")
        if not password or len(password.encode("utf-8")) > 1024:
            raise ValueError("Invalid password length")
        pair_command = [str(executable), "--config", str(config), "device", "connect", code, "--password-stdin"]
        rejected = subprocess.run(pair_command, input="invalid-test-password-" + uuid.uuid4().hex,
            text=True, encoding="utf-8", capture_output=True, env=environment(options.relay_only), timeout=85)
        if rejected.returncode == 0 or "the device rejected authentication" not in rejected.stderr:
            diagnostic = rejected.stderr.replace(password, '[redacted]')[-800:]
            raise RuntimeError("Negative password check did not reach verified authentication rejection: " + diagnostic)
        result = subprocess.run(pair_command, input=password, text=True, encoding="utf-8", capture_output=True,
            env=environment(options.relay_only), timeout=85)
        del password
        if result.returncode:
            raise RuntimeError("Normal public pairing failed; inspect the isolated CLI result")
        assert json.loads(result.stdout)["connected"] is True
        session["device_code"] = code
        session["normal_pairing"] = True
        session["wrong_password_rejected"] = True
        session["serial"] = options.serial
        metadata.write_text(json.dumps(session, indent=2), encoding="utf-8")
        print("Normal public device-code/password pairing passed")
        return
    label = f"{options.action}-{uuid.uuid4()}"
    campaign = root / label
    campaign.mkdir()
    client = PublicMcp(config, code, options.relay_only, campaign / "mcp.log", executable)
    result = {"status": "failed", "run_id": label, "provenance": {
        "apk_sha256": session["apk_sha256"], "public_server": True,
        "serial": session.get("serial"),
        "forced_relay": options.relay_only, "device_code": code},
        # Physical and network assertions require a separate contemporaneous record.
        "physical_usb_unplug_confirmed": False, "network_environment_verified": False}
    try:
        connected, raw = client.connect()
        assert not raw.get("isError") and connected.get("connected") is True
        assert connected["target"]["execution"]["os_family"] == "android"
        tasks = AndroidTasks(client, PACKAGE, campaign / "operation-trace.jsonl")
        if options.action == "campaign":
            result["primitive_results"] = tasks.basics(options.rounds)
        else:
            start = time.monotonic()
            samples = 0
            while time.monotonic() - start < options.seconds:
                deadline = time.monotonic() + 60
                tasks.call({"action": "capabilities"})
                value, _ = client.mobile(str(uuid.uuid4()), {"action": "observe"})
                operation = value["operation"]
                assert operation["state"] == "completed" or (
                    operation["state"] == "failed" and operation.get("error") in ("screen_locked", "control_paused"))
                with (campaign / "standby-trace.jsonl").open("a", encoding="utf-8") as output:
                    output.write(json.dumps({"elapsed_seconds": round(time.monotonic() - start, 3),
                        "state": operation["state"], "error": operation.get("error"),
                        "connection_path": value.get("connection_path")}) + "\n")
                samples += 1
                progress = {"elapsed_seconds": round(time.monotonic() - start, 3), "samples": samples}
                (campaign / "progress.json").write_text(json.dumps(progress), encoding="utf-8")
                print(f"Public standby sample {samples}; elapsed={progress['elapsed_seconds']}s", flush=True)
                time.sleep(max(0, min(deadline - time.monotonic(), options.seconds - (time.monotonic() - start))))
            tasks.call({"action": "capabilities"})
            result["standby_seconds"] = time.monotonic() - start
            result["standby_samples"] = samples + 1
        result["status"] = "passed"
    finally:
        client.close()
        (campaign / "result.json").write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(result))


if __name__ == "__main__":
    main()
