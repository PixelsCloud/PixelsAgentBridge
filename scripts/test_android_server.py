#!/usr/bin/env python3
"""Full local Server/PostgreSQL/Relay acceptance through Android's ordinary foreground service."""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import socket
import shutil
import subprocess
import time
import uuid

from service_config import write_config
from test_android_remote import ROOT, PACKAGE, Mcp, adb, environment, account, installed_apk_sha256
from android_scenarios import AndroidTasks


def port():
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


def wait_port(number, process):
    deadline = time.monotonic() + 25
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError("Isolated service exited; inspect its private log")
        try:
            with socket.create_connection(("127.0.0.1", number), timeout=0.2):
                return
        except OSError:
            time.sleep(0.1)
    raise RuntimeError("Isolated service did not listen")


class AndroidMcp(Mcp):
    code = ""

    def tool(self, name, args, allow_error=False):
        args = dict(args)
        if args.get("device_code") == "987654321":
            args["device_code"] = self.code
        return super().tool(name, args, allow_error)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial", default="emulator-5554")
    parser.add_argument("--preconfigured-accessibility", action="store_true", help="Keep the phone's manually enabled service and skip permission toggles")
    parser.add_argument("--skip-lock-cycle", action="store_true", help="Do not lock a personal phone that may require a PIN")
    parser.add_argument("--background-seconds", type=int, default=0)
    parser.add_argument("--connection-cycles", type=int, default=1)
    parser.add_argument("--operation-rounds", type=int, default=0)
    parser.add_argument("--scenario-rounds", type=int, default=0)
    parser.add_argument("--driver", choices=["instrumentation", "debug"], default="instrumentation",
        help="Use the main APK debug probe when the phone refuses the optional test APK")
    options = parser.parse_args()
    if not 0 <= options.background_seconds <= 86400:
        parser.error("--background-seconds must be between 0 and 86400")
    if not 0 <= options.operation_rounds <= 100 or not 0 <= options.scenario_rounds <= 100:
        parser.error("Operation and scenario rounds must be between 0 and 100")
    if not 1 <= options.connection_cycles <= 100:
        parser.error("--connection-cycles must be between 1 and 100")
    if options.connection_cycles > 1 and options.driver != "debug":
        parser.error("Repeated service cycles currently require --driver debug")
    sdk = int(adb(options.serial, "shell", "getprop", "ro.build.version.sdk").strip())
    request = str(uuid.uuid4())
    root = ROOT / ".build/android/server" / request
    root.mkdir(parents=True)
    print(f"Formal Android service acceptance: {request} ({options.driver})", flush=True)
    apk = ROOT / "apps/android/app/build/outputs/apk/debug/app-debug.apk"
    provenance = {"apk_sha256": hashlib.sha256(apk.read_bytes()).hexdigest(),
        "serial": options.serial, "sdk": sdk, "started_at_unix": time.time(),
        "local_transport": True, "adb_reverse": True, "forced_relay": True}
    assert installed_apk_sha256(options.serial) == provenance["apk_sha256"], "Installed APK differs from the acceptance artifact"
    container = "pab-android-" + request
    docker = ["docker", "--context", "desktop-linux"]
    endpoint = subprocess.check_output([*docker, "context", "inspect", "desktop-linux", "--format", "{{.Endpoints.docker.Host}}"], text=True).strip()
    assert endpoint.startswith("npipe:") or endpoint.startswith("unix:"), "Local Docker only"
    database_port, control_port, relay_port = port(), port(), port()
    database_name = "android_" + request.replace("-", "")
    password = uuid.uuid4().hex
    env = environment()
    env["POSTGRES_PASSWORD"] = password
    database_url = f"postgres://android:{password}@127.0.0.1:{database_port}/{database_name}?sslmode=disable"
    origin = f"https://localhost:{control_port}"
    control = f"wss://localhost:{control_port}/control"
    relay = f"https://localhost:{relay_port}"
    certificate, key = root / "cert.pem", root / "key.pem"
    binaries = root / "bin"
    binaries.mkdir()
    extension = ".exe" if os.name == "nt" else ""
    tool_source = ROOT / ".build/android/tools"
    if not (tool_source / "manifest.json").exists():
        tool_source = ROOT / "target/debug"
    for name in ("pab-server", "pab-relay-server", "pab-mcp"):
        shutil.copy2(tool_source / (name + extension), binaries / (name + extension))
    server_exe = binaries / ("pab-server" + extension)
    relay_exe = binaries / ("pab-relay-server" + extension)
    processes = []
    clients = []
    mappings = []
    created = False
    test = None
    signed_in = False
    completed = False
    config = root / "mcp.toml"
    log = (root / "services.log").open("w", encoding="utf-8")
    test_log = (root / "instrumentation.log").open("w", encoding="utf-8")

    def launch_server(server_config):
        process = subprocess.Popen([str(server_exe), "serve", "--config", str(server_config)], env=env,
            stdout=log, stderr=log, creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
        processes.append(process)
        wait_port(control_port, process)
        return process

    def state(timeout=40, step=None, predicate=None):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if test is not None and test.poll() is not None:
                raise RuntimeError("Android UI acceptance ended early; inspect instrumentation.log")
            if options.driver == "debug":
                probe("status")
            try:
                value = json.loads(adb(options.serial, "shell", "run-as", PACKAGE, "cat", "files/production-session.json", check=False))
            except json.JSONDecodeError:
                time.sleep(0.2)
                continue
            if value.get("request_id") == request and value.get("error"):
                raise RuntimeError("Android production probe failed; inspect its state")
            if value.get("request_id") == request and (step is None or value.get("step") == step) and (predicate is None or predicate(value)):
                return value
            time.sleep(0.2)
        raise RuntimeError("Android production connection state did not arrive")

    def probe(action, nonce=""):
        args = ["shell", "am", "broadcast", "-n", f"{PACKAGE}/com.pixels.agent.fixture.ProductionProbeReceiver",
            "--es", "production_request_id", request, "--es", "probe_action", action]
        if nonce:
            args.extend(["--es", "probe_nonce", nonce])
        adb(options.serial, *args)

    def command(action):
        nonce = str(uuid.uuid4())
        if options.driver == "debug":
            if action == "connect":
                adb(options.serial, "shell", "am", "start", "-f", "0x10008000", "-n",
                    f"{PACKAGE}/com.pixels.agent.fixture.ProductionProbeActivity", "--es", "production_request_id", request,
                    "--ez", "probe_restart", "true")
            elif action in ("background", "target", "capture", "reset"):
                if action == "capture" and sdk < 30:
                    raise RuntimeError("The debug production probe currently requires API 30 for capture")
                activity = f"{PACKAGE}/com.pixels.agent.fixture.AutomationFixtureActivity"
                args = ["shell", "am", "start", "--activity-single-top", "--activity-clear-top", "-n", activity]
                if action == "background":
                    args.extend(["--ez", "finish_fixture", "true"])
                if action == "reset":
                    args.extend(["--ez", "reset_fixture", "true"])
                adb(options.serial, *args)
                if action == "background":
                    state(predicate=lambda value: value.get("target_destroyed"))
                probe("checkpoint", nonce)
            else:
                probe(action, nonce)
            return nonce
        value = {"request_id": request, "nonce": nonce, "action": action}
        file = root / "command.json"
        file.write_text(json.dumps(value), encoding="utf-8")
        staged = f"/data/local/tmp/pab-production-{request}.json"
        adb(options.serial, "push", str(file), staged)
        adb(options.serial, "shell", "run-as", PACKAGE, "cp", staged, "files/production-command.json")
        adb(options.serial, "shell", "rm", "-f", staged)
        return nonce

    try:
        subprocess.run([*docker, "run", "--detach", "--rm", "--pull", "never", "--name", container,
            "--label", f"pixels.android.acceptance={request}", "--cpus", "2", "--memory", "512m",
            "--publish", f"127.0.0.1:{database_port}:5432", "--env", "POSTGRES_PASSWORD",
            "--env", "POSTGRES_USER=android", "--env", f"POSTGRES_DB={database_name}", "postgres:18.6"], env=env,
            check=True, stdout=subprocess.DEVNULL, stderr=log)
        created = True
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            ready = subprocess.run([*docker, "exec", container, "pg_isready", "-h", "127.0.0.1", "-U", "android", "-d", database_name], capture_output=True)
            if ready.returncode == 0:
                break
            time.sleep(0.2)
        else:
            raise RuntimeError("Isolated PostgreSQL did not start")
        openssl = Path("C:/Program Files/Git/usr/bin/openssl.exe")
        subprocess.run([str(openssl) if openssl.is_file() else "openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", str(key),
            "-out", str(certificate), "-days", "2", "-subj", "/CN=localhost", "-addext",
            "subjectAltName=DNS:localhost,IP:127.0.0.1", "-addext", "basicConstraints=critical,CA:FALSE"],
            check=True, stdout=log, stderr=log)
        secret = uuid.uuid4().hex
        server_config = write_config(root / "server.toml", {"listen": f"127.0.0.1:{control_port}",
            "database": {"url": database_url}, "tls": {"cert": certificate, "key": key},
            "web": {"origin": origin, "assets": root / "no-web-assets", "registration_enabled": True},
            "relay": {"control_secret": secret}, "log": {"directory": root / "logs"}})
        subprocess.run([str(server_exe), "init", "--config", str(server_config)], env=env, check=True, stdout=log, stderr=log)
        server = launch_server(server_config)
        relay_config = write_config(root / "relay.toml", {"node_id": "android-local", "usage_dir": str(root / "relay-usage"),
            "https_bind": f"127.0.0.1:{relay_port}", "quic_bind": "127.0.0.1:0", "tls": {"cert": certificate, "key": key},
            "control": {"url": f"wss://localhost:{control_port}/relay-control", "ca_cert": certificate, "secret": secret},
            "log": {"directory": root / "logs"}})
        relay_process = subprocess.Popen([str(relay_exe), "--config", str(relay_config)], env=env, stdout=log, stderr=log,
            creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
        processes.append(relay_process)
        wait_port(relay_port, relay_process)
        for number in [control_port, relay_port]:
            adb(options.serial, "reverse", f"tcp:{number}", f"tcp:{number}")
            mappings.append(number)
        config = write_config(config, {"control_url": control, "relay_urls": [relay], "control_ca_cert": str(certificate),
            "relay_ca_cert": str(certificate), "operator": {"data_dir": root / "operator"}})
        account(config, "register", uuid.uuid4().hex, executable=binaries / ("pab-mcp" + extension))
        signed_in = True
        if options.driver == "instrumentation":
            adb(options.serial, "shell", "am", "force-stop", PACKAGE)
            time.sleep(1)
        ca = base64.b64encode(certificate.read_bytes()).decode("ascii")
        if options.driver == "debug":
            if not options.preconfigured_accessibility or not options.skip_lock_cycle:
                parser.error("--driver debug requires --preconfigured-accessibility --skip-lock-cycle")
            fixture = write_config(root / "production-fixture.toml", {"schema_version": 1,
                "control_url": control, "device_name": "Android production probe", "relay_urls": [relay],
                "trusted_ca_pem": certificate.read_text(encoding="utf-8")})
            staged = f"/data/local/tmp/pab-production-{request}.toml"
            adb(options.serial, "push", str(fixture), staged)
            adb(options.serial, "shell", "run-as", PACKAGE, "cp", staged, "files/production-fixture.toml")
            adb(options.serial, "shell", "rm", "-f", staged)
            started = adb(options.serial, "shell", "am", "start", "-f", "0x10008000", "-n", f"{PACKAGE}/com.pixels.agent.fixture.ProductionProbeActivity",
                "--es", "production_request_id", request)
            if "Error" in started or "not started" in started:
                raise RuntimeError("Android did not launch the production probe Activity")
        else:
            test = subprocess.Popen(["adb", "-s", options.serial, "shell", "am", "instrument", "-w", "-e", "class",
                "com.pixels.agent.ProductionConnectionDeviceTest", "-e", "production_request_id", request,
                "-e", "control_url", control, "-e", "relay_url", relay, "-e", "fixture_ca", ca, "-e", "accessibility_preconfigured", str(options.preconfigured_accessibility).lower(),
                "-e", "background_seconds", str(options.background_seconds),
                f"{PACKAGE}.test/androidx.test.runner.AndroidJUnitRunner"], stdout=test_log, stderr=test_log)
        access = state(80, predicate=lambda value: value.get("remote_ready") and value.get("accessibility_ready"))
        mcp = AndroidMcp(config, True, root / "mcp.log", executable=binaries / ("pab-mcp" + extension))
        mcp.code = access["device_code"]
        clients.append(mcp)
        mcp.tool("pab_list_devices", {})
        assert len(mcp.code) == 9 and mcp.code.isascii() and mcp.code.isdigit()
        pairing = subprocess.run([str(binaries / ("pab-mcp" + extension)), "--config", str(config),
            "device", "connect", mcp.code, "--password-stdin"], input=access["password"],
            capture_output=True, text=True, encoding="utf-8", env=environment(True), timeout=85)
        del access["password"]
        if pairing.returncode:
            raise RuntimeError(f"Normal device pairing failed: {pairing.stderr[-500:]}")
        assert json.loads(pairing.stdout)["connected"] is True
        connected, raw = mcp.connect()
        assert not raw.get("isError"), connected.get("error")
        assert connected["target"]["execution"]["os_family"] == "android"
        # Observation may briefly contain the previous window while Android delivers the
        # launch event. Wait for the fixture; never retry an input to hide this race.
        window_samples = []
        field = None
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            observed, _ = mcp.mobile(str(uuid.uuid4()), {"action": "observe"})
            assert observed["operation"]["state"] == "completed"
            screen = observed["operation"]["observation"]
            window_samples.append({"package_name": screen.get("package_name"),
                "nodes": len(screen["elements"]), "editable": sum(bool(node["editable"]) for node in screen["elements"]),
                "roles": sorted({node["role"] for node in screen["elements"]})})
            field = next((node for node in screen["elements"] if node["editable"] and node.get("name") == "fixture_input"), None)
            if field is not None and screen.get("package_name") == PACKAGE:
                break
            time.sleep(0.2)
        (root / "initial-window.json").write_text(json.dumps(window_samples, indent=2), encoding="utf-8")
        assert field is not None, "Fixture input did not become observable; inspect initial-window.json"
        tasks = AndroidTasks(mcp, PACKAGE, root / "operation-trace.jsonl")
        tasks.act("fixture_input", "set_text", text="完整服务端测试")
        tasks.act("fixture_save")
        click_id = tasks.last_input_request
        tasks.wait("fixture_save_count:1")
        observed = tasks.wait("fixture_saved:完整服务端测试")
        assert any("完整服务端测试" in (node["name"] or "") for node in observed["elements"] if not node["editable"])
        primitive_results = tasks.basics(options.operation_rounds) if options.operation_rounds else None
        scenario_results = tasks.scenarios(options.scenario_rounds) if options.scenario_rounds else None
        if scenario_results:
            for scenario in scenario_results.values():
                assert scenario["passed"] / scenario["attempted"] >= 0.98, scenario
        state(step=command("pause"))
        paused, _ = mcp.mobile(str(uuid.uuid4()), {"action": "observe"})
        assert paused["operation"]["error"] == "control_paused"
        state(step=command("resume"))
        if not options.preconfigured_accessibility:
            state(step=command("permission-off"))
            denied, _ = mcp.mobile(str(uuid.uuid4()), {"action": "observe"})
            assert denied["operation"]["error"] == "service_unavailable"
            state(step=command("permission-on"))
        if not options.skip_lock_cycle:
            state(step=command("screen-off"))
            locked, _ = mcp.mobile(str(uuid.uuid4()), {"action": "observe"})
            assert locked["operation"]["error"] == "screen_locked"
            locked_capture, _ = mcp.mobile(str(uuid.uuid4()), {"action": "screenshot"})
            assert locked_capture["operation"]["error"] == "screen_locked"
            state(step=command("unlock"))
            unlocked, _ = mcp.mobile(str(uuid.uuid4()), {"action": "observe"})
            assert unlocked["operation"]["state"] == "completed"
        state(step=command("capture"))
        captured, raw = mcp.mobile(str(uuid.uuid4()), {"action": "screenshot"})
        assert captured["operation"]["state"] == "completed" and any(item["type"] == "image" for item in raw["content"])
        if sdk < 30 and not options.preconfigured_accessibility:
            state(step=command("permission-off"))
            state(step=command("stop-capture"))
            stopped_capture, _ = mcp.mobile(str(uuid.uuid4()), {"action": "screenshot"})
            assert stopped_capture["operation"]["error"] == "consent_required"
            state(step=command("permission-on"))
        state(step=command("background"))
        background, _ = mcp.mobile(str(uuid.uuid4()), {"action": "observe"})
        assert background["operation"]["state"] == "completed"
        background_started = time.monotonic()
        background_samples = 1
        while time.monotonic() - background_started < options.background_seconds:
            time.sleep(min(10, max(0, options.background_seconds - (time.monotonic() - background_started))))
            observed_background, _ = mcp.mobile(str(uuid.uuid4()), {"action": "observe"})
            assert observed_background["operation"]["state"] == "completed"
            background_samples += 1
        generation = state()["generation"]
        server.terminate()
        server.wait(timeout=10)
        state(timeout=25, predicate=lambda value: value["phase"] == "RECONNECTING")
        server = launch_server(server_config)
        restored = state(timeout=40, predicate=lambda value: value.get("remote_ready") and value["generation"] > generation)
        assert restored["device_code"] == mcp.code
        mcp.tool("pab_disconnect", {"device_code": mcp.code})
        connected, raw = mcp.connect()
        assert not raw.get("isError"), connected.get("error")
        old, _ = mcp.mobile(click_id, operation="get")
        assert old["operation"]["state"] == "completed"
        resumed, _ = mcp.mobile(str(uuid.uuid4()), {"action": "observe"})
        assert resumed["operation"]["state"] == "completed"
        for cycle in range(1, options.connection_cycles):
            mcp.tool("pab_disconnect", {"device_code": mcp.code})
            state(step=command("disconnect"), predicate=lambda value: not value["remote_ready"])
            command("connect")
            current = state(timeout=60, predicate=lambda value: value.get("remote_ready"))
            assert current["device_code"] == mcp.code
            connected, raw = mcp.connect()
            assert not raw.get("isError"), connected.get("error")
            observed, _ = mcp.mobile(str(uuid.uuid4()), {"action": "observe"})
            assert observed["operation"]["state"] == "completed"
            original, _ = mcp.mobile(click_id, operation="get")
            assert original["operation"]["state"] == "completed"
            print(f"Formal service cycle {cycle + 1}/{options.connection_cycles} passed", flush=True)
        state(step=command("disconnect"), predicate=lambda value: not value["remote_ready"])
        command("finish")
        if test is not None:
            test.wait(timeout=20)
            test_log.flush()
            assert "OK (1 test)" in (root / "instrumentation.log").read_text(encoding="utf-8")
        summary = {"status": "passed", "request_id": request, "full_server": True, "postgresql": True,
            "provenance": provenance,
            "real_relay": True, "home_screen_start": True if options.driver == "instrumentation" else None,
            "driver": options.driver, "foreground_service": True, "unicode_input": True,
            "normal_device_pairing": True,
            "connection_cycles": options.connection_cycles,
            "primitive_results": primitive_results,
            "scenario_results": scenario_results,
            "native_click": True, "pause_resume": True, "permission_recovery": None if options.preconfigured_accessibility else True,
            "screen_off_rejected": None if options.skip_lock_cycle else True, "unlock_recovery": None if options.skip_lock_cycle else True,
            "home_status_messages": True if options.driver == "instrumentation" else None, "system_consented_screenshot": sdk < 30, "accessibility_screenshot": sdk >= 30, "sdk": sdk,
            "stop_capture_without_accessibility": True if sdk < 30 and not options.preconfigured_accessibility else None,
            "background_seconds": options.background_seconds, "background_samples": background_samples,
            "activity_closed_connection_survives": True, "control_reconnect_same_identity": True,
            "query_original_after_reconnect": True, "home_screen_disconnect": True if options.driver == "instrumentation" else None}
        (root / "result.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
        completed = True
        print(json.dumps(summary))
    finally:
        if not completed:
            # Preserve failures too. Screen contents, passwords and input values are excluded.
            (root / "result.json").write_text(json.dumps({"status": "failed", "request_id": request,
                "provenance": provenance, "driver": options.driver,
                "reason": "campaign_incomplete; inspect private diagnostics"}, indent=2) + "\n", encoding="utf-8")
        if options.driver == "debug":
            try:
                command("finish")
            except Exception:
                pass  # Preserve the backup for explicit recovery if the process was lost.
        if test and test.poll() is None:
            command("finish")
            try:
                test.wait(timeout=20)
            except subprocess.TimeoutExpired:
                adb(options.serial, "shell", "am", "force-stop", PACKAGE, check=False)
                test.terminate()
                test.wait(timeout=5)
        for client in clients:
            client.close()
        if signed_in:
            try:
                account(config, "logout", executable=binaries / ("pab-mcp" + extension))
            except Exception:
                pass
        for number in mappings:
            adb(options.serial, "reverse", "--remove", f"tcp:{number}", check=False)
        for process in reversed(processes):
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=10)
        if created:
            label = subprocess.check_output([*docker, "inspect", "--format", '{{index .Config.Labels "pixels.android.acceptance"}}', container], text=True).strip()
            assert label == request
            subprocess.run([*docker, "rm", "--force", container], stdout=subprocess.DEVNULL, check=True)
        test_log.close()
        log.close()


if __name__ == "__main__":
    main()
