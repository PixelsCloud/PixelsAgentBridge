#!/usr/bin/env python3
"""Exercise the formal MCP binary against an Android fixture through real PAB/iroh."""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import queue
import shutil
import subprocess
import threading
import time
import uuid

from android_scenarios import AndroidTasks

ROOT = Path(__file__).resolve().parents[1]
PACKAGE = "com.pixels.agent.debug"


def environment(relay_only=False):
    env = {key: value for key, value in os.environ.items() if not key.startswith("PAB_")}
    if relay_only:
        env["PAB_TEST_RELAY_ONLY"] = "1"
    return env


def account(config: Path, action: str, password: str | None = None, *, executable: Path | None = None,
            username: str = "android-acceptance"):
    exe = executable or ROOT / "target/debug" / ("pab-mcp.exe" if os.name == "nt" else "pab-mcp")
    args = [str(exe), "--config", str(config), "account", action]
    if password is not None:
        args.extend([username, "--password-stdin"])
    result = subprocess.run(args, input=password, capture_output=True, text=True,
        encoding="utf-8", env=environment(), timeout=25)
    if result.returncode:
        raise RuntimeError(f"Isolated account {action} failed: {result.stderr[-500:]}")


def adb(serial: str, *args: str, check: bool = True, timeout: float = 20) -> str:
    result = subprocess.run(["adb", "-s", serial, *args], capture_output=True,
                            stdin=subprocess.DEVNULL, text=True, encoding="utf-8", errors="replace", timeout=timeout, check=check)
    return result.stdout


def installed_apk_sha256(serial: str) -> str:
    paths = adb(serial, "shell", "pm", "path", PACKAGE).splitlines()
    if len(paths) != 1 or not paths[0].startswith("package:/data/app/"):
        raise RuntimeError("Expected one installed base APK for the selected package")
    path = paths[0].removeprefix("package:")
    digest = adb(serial, "shell", "sha256sum", path, timeout=60).split()[0]
    if len(digest) != 64 or any(c not in "0123456789abcdef" for c in digest):
        raise RuntimeError("Installed APK hash could not be verified")
    return digest


class Mcp:
    def __init__(self, config: Path, relay_only: bool, log: Path, *, executable: Path | None = None):
        env = environment(relay_only)
        self.log = log.open("w", encoding="utf-8")
        exe = executable or ROOT / "target/debug" / ("pab-mcp.exe" if os.name == "nt" else "pab-mcp")
        self.process = subprocess.Popen([str(exe), "--config", str(config)], env=env,
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.log, text=True,
            encoding="utf-8", bufsize=1)
        self.responses: queue.Queue[dict] = queue.Queue()
        self.sequence = 0
        def consume():
            assert self.process.stdout is not None
            for line in self.process.stdout:
                try:
                    self.responses.put(json.loads(line))
                except json.JSONDecodeError:
                    pass
        self.reader = threading.Thread(target=consume, daemon=True)
        self.reader.start()
        self.rpc("initialize", {"protocolVersion": "2025-03-26", "capabilities": {},
            "clientInfo": {"name": "android-acceptance", "version": "1"}})
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def send(self, value):
        assert self.process.stdin is not None
        self.process.stdin.write(json.dumps(value, ensure_ascii=False) + "\n")
        self.process.stdin.flush()

    def rpc(self, method, params, timeout=35):
        self.sequence += 1
        current = self.sequence
        self.send({"jsonrpc": "2.0", "id": current, "method": method, "params": params})
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                response = self.responses.get(timeout=max(0.01, deadline - time.monotonic()))
            except queue.Empty:
                break
            if response.get("id") == current:
                if "error" in response:
                    raise RuntimeError(f"MCP protocol error for {method}")
                return response["result"]
        raise RuntimeError(f"MCP timed out for {method}; inspect isolated log")

    def tool(self, name, args, allow_error=False):
        response = self.rpc("tools/call", {"name": name, "arguments": args})
        if response.get("isError") and not allow_error:
            problem = response.get("structuredContent", {}).get("error", {})
            raise RuntimeError(f"{name} failed: {problem}")
        value = response.get("structuredContent")
        if value is None:
            text = next(item["text"] for item in response["content"] if item["type"] == "text")
            value = json.loads(text)
        return value, response

    def mobile(self, request, command=None, operation="execute"):
        args = {"device_code": "987654321", "operation": operation, "request_id": request}
        if command is not None:
            args["command"] = command
        value, raw = self.tool("pab_mobile_control", args, allow_error=True)
        if "operation" not in value:
            raise RuntimeError(f"Mobile call failed: {value.get('error')}")
        deadline = time.monotonic() + 12
        while value["operation"]["state"] == "running" and time.monotonic() < deadline:
            time.sleep(0.1)
            value, raw = self.tool("pab_mobile_control", {"device_code": "987654321",
                "operation": "get", "request_id": request}, allow_error=True)
            if "operation" not in value:
                raise RuntimeError(f"Mobile query failed: {value.get('error')}")
        return value, raw

    def connect(self):
        deadline = time.monotonic() + 40
        while time.monotonic() < deadline:
            value, raw = self.tool("pab_connect", {"device_code": "987654321"}, allow_error=True)
            # A newly started MCP may still be syncing its account. This is a
            # pre-connection readiness check; no accepted device work is replayed.
            if raw.get("isError") and value.get("error", {}).get("message") == (
                "Bridge connection is unavailable: account synchronization is pending"
            ):
                time.sleep(0.1)
                continue
            if raw.get("isError") or value.get("connected"):
                return value, raw
        raise RuntimeError("Device connection remained pending; it was not counted as authentication")

    def close(self):
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        self.log.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial", default="emulator-5554")
    parser.add_argument("--relay-only", action="store_true")
    parser.add_argument("--output-root", type=Path, default=ROOT / ".build/android/remote")
    parser.add_argument("--mcp-executable", type=Path)
    parser.add_argument("--fixture-executable", type=Path)
    parser.add_argument("--apk", type=Path, default=ROOT / "apps/android/app/build/outputs/apk/debug/app-debug.apk")
    parser.add_argument("--capture-consent-tap", nargs=2, type=int, metavar=("X", "Y"),
        help="Click the actual Android 9 system consent dialog at these emulator-specific coordinates")
    options = parser.parse_args()
    sdk = int(adb(options.serial, "shell", "getprop", "ro.build.version.sdk").strip())
    apk = options.apk.resolve()
    apk_hash = hashlib.sha256(apk.read_bytes()).hexdigest()
    assert installed_apk_sha256(options.serial) == apk_hash, "Installed APK differs from the test artifact"
    request = str(uuid.uuid4())
    run = options.output_root.resolve() / request
    run.mkdir(parents=True)
    extension = ".exe" if os.name == "nt" else ""
    binaries = run / "bin"
    binaries.mkdir()
    server_exe = binaries / ("remote-fixture" + extension)
    mcp_exe = binaries / ("pab-mcp" + extension)
    shutil.copy2(options.fixture_executable or ROOT / "target/debug/examples" / server_exe.name, server_exe)
    staged = ROOT / ".build/android/tools" / mcp_exe.name
    shutil.copy2(options.mcp_executable or (staged if staged.exists() else ROOT / "target/debug" / mcp_exe.name), mcp_exe)
    server = subprocess.Popen([str(server_exe)], stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                              text=True, encoding="utf-8")
    lines: queue.Queue[dict] = queue.Queue()
    def consume():
        assert server.stdout is not None
        for line in server.stdout:
            try:
                lines.put(json.loads(line))
            except json.JSONDecodeError:
                pass
    threading.Thread(target=consume, daemon=True).start()
    ports = []
    clients = []
    settings = None
    signed_in = False
    try:
        boot_deadline = time.monotonic() + 90
        while time.monotonic() < boot_deadline:
            if adb(options.serial, "shell", "getprop", "sys.boot_completed", check=False).strip() == "1":
                break
            time.sleep(1)
        else:
            raise RuntimeError("Emulator did not finish booting")
        ready = lines.get(timeout=15)
        assert ready["ready"]
        ports = [ready["control_port"], ready["relay_port"]]
        adb(options.serial, "shell", "run-as", PACKAGE, "am", "stopservice", "--user", "0", "-n", f"{PACKAGE}/com.pixels.agent.platform.capture.ProjectionCaptureService", check=False)
        for port in ports:
            adb(options.serial, "reverse", f"tcp:{port}", f"tcp:{port}")
        ca = run / "ca.pem"
        ca.write_text(ready["ca_pem"], encoding="utf-8")
        url = f"wss://localhost:{ports[0]}/control"
        relay = f"https://localhost:{ports[1]}/"
        toml = ("schema_version = 1\n" + f"control_url = {json.dumps(url)}\n"
            + 'device_name = "Remote Android fixture"\n' + f"relay_urls = [{json.dumps(relay)}]\n"
            + f"trusted_ca_pem = {json.dumps(ready['ca_pem'])}\n"
            + f"relay_only_for_testing = {str(options.relay_only).lower()}\n")
        config = run / "android.toml"
        config.write_text(toml, encoding="utf-8")
        staged = f"/data/local/tmp/pab-android-{request}.toml"
        adb(options.serial, "push", str(config), staged)
        adb(options.serial, "shell", "run-as", PACKAGE, "cp", staged, "files/remote-fixture.toml")
        adb(options.serial, "shell", "rm", "-f", staged)
        adb(options.serial, "shell", "am", "start", "-f", "0x10008000", "-n",
            f"{PACKAGE}/com.pixels.agent.fixture.AutomationFixtureActivity", "--ez", "run_remote_fixture", "true",
            "--es", "remote_request_id", request)
        deadline = time.monotonic() + 35
        access = None
        while time.monotonic() < deadline:
            try:
                candidate = json.loads(adb(options.serial, "shell", "run-as", PACKAGE, "cat", "files/remote-fixture-access.json", check=False))
            except json.JSONDecodeError:
                time.sleep(0.2)
                continue
            if candidate.get("request_id") == request:
                if candidate.get("ready"):
                    access = candidate
                    break
                if candidate.get("phase") == "FAILED":
                    raise RuntimeError(f"Android remote fixture failed: {candidate.get('error')}")
            time.sleep(0.2)
        if access is None:
            raise RuntimeError("Android did not publish its remote endpoint")
        if not access["accessibility_ready"]:
            raise RuntimeError("Enable Pixels accessibility through Android settings before this acceptance")
        operator = run / "operator"
        operator.mkdir()
        settings = run / "mcp.toml"
        settings.write_text(f"control_url = {json.dumps(url)}\nrelay_urls = [{json.dumps(relay)}]\n"
            + f"control_ca_cert = {json.dumps(ca.as_posix())}\nrelay_ca_cert = {json.dumps(ca.as_posix())}\n"
            + f"[operator]\ndata_dir = {json.dumps(operator.as_posix())}\n", encoding="utf-8")
        account(settings, "register", uuid.uuid4().hex, executable=mcp_exe)
        signed_in = True
        mcp = Mcp(settings, options.relay_only, run / "mcp.log", executable=mcp_exe)
        clients.append(mcp)
        tools = mcp.rpc("tools/list", {})
        assert "pab_mobile_control" in {item["name"] for item in tools["tools"]}
        mcp.tool("pab_list_devices", {})
        pair_command = [str(mcp_exe), "--config", str(settings), "device", "connect", "987654321", "--password-stdin"]
        rejected = subprocess.run(pair_command, input="deliberately-wrong-password", text=True,
            encoding="utf-8", capture_output=True, env=environment(options.relay_only), timeout=85)
        assert rejected.returncode != 0 and "device rejected authentication" in rejected.stderr
        paired = subprocess.run(pair_command, input=access["password"], text=True,
            encoding="utf-8", capture_output=True, env=environment(options.relay_only), timeout=85)
        assert paired.returncode == 0, "Normal device password pairing failed"
        del access["password"]
        mcp.tool("pab_disconnect", {"device_code": "987654321"})
        connected, connected_raw = mcp.connect()
        assert not connected_raw.get("isError"), connected.get("error")
        assert connected["target"]["execution"]["os_family"] == "android"
        observed, _ = mcp.mobile(str(uuid.uuid4()), {"action": "observe"})
        assert observed["operation"]["state"] == "completed", observed["operation"].get("error")
        if options.relay_only:
            assert observed["connection_path"] == "relay"
        tasks = AndroidTasks(mcp, PACKAGE, run / "operation-trace.jsonl")
        tasks.wait("fixture_input")
        tasks.act("fixture_input", "set_text", text="Pixels 远程测试")
        tasks.act("fixture_save")
        click_id = tasks.last_input_request
        command = tasks.last_input_command
        tasks.wait("fixture_saved:Pixels 远程测试")
        for _ in range(20):
            duplicate, _ = mcp.mobile(click_id, command)
            assert duplicate["operation"]["state"] == "completed"
        conflict, _ = mcp.mobile(click_id, {"action": "observe"})
        assert conflict["operation"]["error"] == "request_conflict"
        observed, _ = mcp.mobile(str(uuid.uuid4()), {"action": "observe"})
        assert any("Pixels 远程测试" in (node["name"] or "") for node in observed["operation"]["observation"]["elements"] if not node["editable"])
        button = next(node for node in observed["operation"]["observation"]["elements"] if node["clickable"] and node["name"] == "fixture_save")
        effects = json.loads(adb(options.serial, "shell", "run-as", PACKAGE, "cat", "files/remote-fixture-effects.json"))
        assert effects["request_id"] == request and effects["save_count"] == 1
        second = Mcp(settings, options.relay_only, run / "mcp-second.log", executable=mcp_exe)
        clients.append(second)
        second_connected, second_raw = second.connect()
        assert not second_raw.get("isError"), second_connected.get("error")
        stolen, _ = second.mobile(str(uuid.uuid4()), {"action": "click", "element_ref": button["element_ref"]})
        assert stolen["operation"]["error"] == "target_stale"
        foreign, _ = second.mobile(click_id, operation="get")
        assert foreign["operation"]["error"] == "not_found"
        fresh, _ = mcp.mobile(str(uuid.uuid4()), {"action": "observe"})
        observation = fresh["operation"]["observation"]
        button = next(node for node in observation["elements"] if node["clickable"] and node["name"] == "fixture_save")
        bounds = button["bounds"]
        x, y = (bounds["left"] + bounds["right"]) // 2, (bounds["top"] + bounds["bottom"]) // 2
        gesture, _ = mcp.mobile(str(uuid.uuid4()), {"action": "gesture", "observation_ref": observation["observation_ref"],
            "from_x": x, "from_y": y, "to_x": x, "to_y": y, "duration_ms": 100})
        assert gesture["operation"]["state"] == "completed", gesture["operation"].get("error")
        time.sleep(0.3)
        effects = json.loads(adb(options.serial, "shell", "run-as", PACKAGE, "cat", "files/remote-fixture-effects.json"))
        assert effects["save_count"] == 2
        original, _ = mcp.mobile(click_id, operation="get")
        assert original["operation"]["state"] == "completed"
        cancelled, _ = mcp.mobile(click_id, operation="cancel")
        assert cancelled["operation"]["state"] == "completed", "Cancellation must not undo completed input"
        if sdk < 30:
            no_consent, _ = mcp.mobile(str(uuid.uuid4()), {"action": "screenshot"})
            assert no_consent["operation"]["error"] == "consent_required"
        screenshot = False
        if sdk >= 30 or options.capture_consent_tap:
            if sdk < 30:
                adb(options.serial, "shell", "am", "start", "-f", "0x30000000", "-n",
                    f"{PACKAGE}/com.pixels.agent.fixture.AutomationFixtureActivity", "--ez", "request_capture_consent", "true",
                    "--es", "remote_request_id", request)
                time.sleep(1)
                adb(options.serial, "shell", "input", "tap", *map(str, options.capture_consent_tap))
                time.sleep(1)
            capture, raw = mcp.mobile(str(uuid.uuid4()), {"action": "screenshot"})
            assert capture["operation"]["state"] == "completed", capture["operation"].get("error")
            content = next(item for item in raw["content"] if item["type"] == "image")
            jpeg = base64.b64decode(content["data"], validate=True)
            assert content["mimeType"] == "image/jpeg" and jpeg.startswith(b"\xff\xd8") and len(jpeg) > 100
            assert capture["image_included"] and len(jpeg) == capture["operation"]["image"]["size"]
            screenshot = True
            if sdk < 30:
                # Android 9 am reports a nonzero status even when it prints Service stopped.
                # Use this debug package's own UID; the product service remains unexported.
                adb(options.serial, "shell", "run-as", PACKAGE, "am", "stopservice", "--user", "0", "-n", f"{PACKAGE}/com.pixels.agent.platform.capture.ProjectionCaptureService", check=False)
                time.sleep(0.3)
                revoked, _ = mcp.mobile(str(uuid.uuid4()), {"action": "screenshot"})
                assert revoked["operation"]["error"] == "consent_required"
        mcp.tool("pab_disconnect", {"device_code": "987654321"})
        reconnected, reconnect_raw = mcp.connect()
        assert not reconnect_raw.get("isError"), reconnected.get("error")
        recovered, _ = mcp.mobile(click_id, operation="get")
        assert recovered["operation"]["state"] == "completed"
        home, _ = mcp.mobile(str(uuid.uuid4()), {"action": "navigate", "destination": "home"})
        assert home["operation"]["state"] == "completed" and home["operation"]["platform_acknowledged"]
        time.sleep(0.5)
        launcher, _ = mcp.mobile(str(uuid.uuid4()), {"action": "observe"})
        assert launcher["operation"]["state"] == "completed"
        assert not any(node["name"] == "fixture_save" for node in launcher["operation"]["observation"]["elements"])
        summary = {"status": "passed", "request_id": request, "connection_path": launcher["connection_path"],
            "provenance": {"serial": options.serial, "sdk": sdk, "apk_sha256": apk_hash,
                "mcp_sha256": hashlib.sha256(mcp_exe.read_bytes()).hexdigest(),
                "fixture_sha256": hashlib.sha256(server_exe.read_bytes()).hexdigest()},
            "normal_device_pairing": True, "duplicate_submissions":20,
            "wrong_password_rejected": True, "android_identity": True, "unicode_input": True,
            "native_click": True, "click_count": 1, "request_deduplicated": True,
            "request_conflict_rejected": True, "cross_connection_reference_rejected": True,
            "foreign_operation_rejected": True}
        summary.update(native_gesture=True, total_save_count=2, operation_query=True,
            completed_action_not_rolled_back=True, screenshot_requires_consent=sdk < 30, accessibility_screenshot=sdk >= 30, sdk=sdk, remote_screenshot=screenshot,
            home_navigation=True, background_observation=True, original_operation_after_reconnect=True)
        (run / "result.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")
        print(json.dumps(summary))
    finally:
        for client in clients:
            client.close()
        if signed_in and settings is not None:
            try:
                account(settings, "logout", executable=mcp_exe)
            except Exception as error:
                print(f"Isolated account cleanup failed: {type(error).__name__}")
        if options.capture_consent_tap:
            adb(options.serial, "shell", "run-as", PACKAGE, "am", "stopservice", "--user", "0", "-n", f"{PACKAGE}/com.pixels.agent.platform.capture.ProjectionCaptureService", check=False)
        adb(options.serial, "shell", "run-as", PACKAGE, "rm", "-f", "files/remote-fixture-access.json", "files/remote-fixture-effects.json", "files/remote-fixture.toml", check=False)
        # Ending the debug fixture releases its private connection. Do not force-stop the app.
        adb(options.serial, "shell", "am", "start", "-f", "0x10008000", "-n", f"{PACKAGE}/com.pixels.agent.MainActivity", check=False)
        for port in ports:
            adb(options.serial, "reverse", "--remove", f"tcp:{port}", check=False)
        server.terminate()
        try:
            server.wait(timeout=5)
        except subprocess.TimeoutExpired:
            server.kill()
            server.wait(timeout=5)


if __name__ == "__main__":
    main()
