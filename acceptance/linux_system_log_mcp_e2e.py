"""Short, disposable Linux system-log MCP acceptance against the formal package.

The temporary device password is captured from the container and placed in the
current user's local Bridge credential database only for this run. It is never
printed, saved in this source tree, or added to the account device catalog.
"""

import json
import hashlib
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import time
import uuid


ROOT = Path(__file__).resolve().parents[1]
PACKAGE = ROOT / ".build/packages/pixels-agent-bridge-linux-x86_64-release.tar.gz"
MCP = ROOT / "target/release/pab-mcp.exe"
CONTAINER = "pab-system-log-mcp-" + uuid.uuid4().hex[:10]
DATABASE = Path(os.environ.get(
    "PAB_BRIDGE_DATABASE",
    Path(os.environ["LOCALAPPDATA"]) / "PixelsAgentBridge/bridge.sqlite3",
))


def run(*argv: str) -> str:
    result = subprocess.run(
        argv, capture_output=True, text=True, encoding="utf-8", errors="replace",
    )
    if result.returncode:
        raise RuntimeError(f"command failed ({result.returncode}): {result.stderr[-1500:]}")
    return result.stdout.strip()


def docker(*argv: str) -> str:
    return run("docker", *argv)


def main() -> None:
    if not PACKAGE.is_file() or not MCP.is_file() or not DATABASE.is_file():
        raise RuntimeError("formal Linux package, current MCP binary, or Bridge database missing")
    device_id = None
    inserted = False
    started = False
    try:
        docker(
            "run", "-d", "--rm", "--name", CONTAINER, "--privileged",
            "--cgroupns=private", "--tmpfs", "/run", "--tmpfs", "/run/lock",
            "--tmpfs", "/tmp", "pab-linux-headless-systemd-test", "/lib/systemd/systemd",
        )
        started = True
        docker("cp", str(PACKAGE), f"{CONTAINER}:/root/pab-linux.tar.gz")
        docker("exec", CONTAINER, "bash", "-lc", "mkdir -p /root/pab-package && tar -xzf /root/pab-linux.tar.gz -C /root/pab-package && bash /root/pab-package/install.sh wss://pab.rgaa.vip/control https://pab-relay.rgaa.vip")
        if docker("exec", CONTAINER, "systemctl", "is-active", "pixels-agent-bridge-executor.service") != "active":
            raise RuntimeError("installed Linux Executor is not active")

        access_script = (
            "import json,sqlite3; "
            "c=sqlite3.connect('/var/lib/pixels-agent-bridge/executor.sqlite3'); "
            "r=c.execute('select tenant_id,device_id,device_code,temporary_password from device_access where id=1').fetchone(); "
            "print(json.dumps({'tenant_id':r[0],'device_id':r[1],'device_code':r[2],'password':r[3]}))"
        )
        access = None
        for _ in range(24):
            try:
                access = json.loads(docker("exec", CONTAINER, "python3", "-c", access_script))
                break
            except RuntimeError as error:
                if "no such table: device_access" not in str(error):
                    raise
                time.sleep(2)
        if access is None:
            service_state = docker("exec", CONTAINER, "systemctl", "is-active", "pixels-agent-bridge-executor.service")
            raise RuntimeError(f"Linux Executor did not register within 48s; service={service_state}")
        device_id = access["device_id"]
        with sqlite3.connect(DATABASE, timeout=10) as db:
            if db.execute("select 1 from device_credentials where device_id=?", (device_id,)).fetchone():
                raise RuntimeError("test device credential already exists; refusing to overwrite")
            db.execute(
                "insert into device_credentials(device_id,password) values (?,?)",
                (device_id, access["password"]),
            )
            db.commit()
            inserted = True
        del access["password"]

        marker = "PAB_LINUX_MCP_" + uuid.uuid4().hex
        docker("exec", CONTAINER, "logger", "-t", "PABLogAcceptance", marker)
        native_rows = [
            json.loads(line)
            for line in docker(
                "exec", CONTAINER, "journalctl", "-t", "PABLogAcceptance",
                "--output=json", "--no-pager",
            ).splitlines()
            if marker in line
        ]
        if len(native_rows) != 1:
            raise AssertionError(f"independent journal count={len(native_rows)}")
        native = native_rows[0]
        timestamp_ms = int(native["__REALTIME_TIMESTAMP"]) // 1000
        probe = None
        for _ in range(6):
            try:
                probe = subprocess.run(
                    [
                        sys.executable, str(ROOT / "acceptance/system_log_mcp_live.py"),
                        "--mcp", str(MCP), "--device-code", access["device_code"],
                        "--source", "journal:local", "--marker", marker,
                        "--start-ms", str(timestamp_ms - 5000),
                        "--end-ms", str(timestamp_ms + 5000),
                        "--follow-check", "--restart-check",
                        "--linux-container", CONTAINER,
                    ],
                    capture_output=True, text=True, timeout=75,
                    env={**os.environ, "PAB_LOG_ACCEPTANCE_DEBUG": "1"},
                )
            except subprocess.TimeoutExpired as error:
                partial = error.stderr or b""
                if isinstance(partial, bytes):
                    partial = partial.decode("utf-8", errors="replace")
                raise RuntimeError(f"Linux MCP probe timed out: {partial[-3000:]}") from error
            if probe.returncode == 0 or "endpoint control was rejected with NotFound" not in probe.stderr:
                break
            time.sleep(5)
        if probe.returncode:
            journal = docker(
                "exec", CONTAINER, "journalctl", "-u",
                "pixels-agent-bridge-executor.service", "-n", "30", "--no-pager", "-o", "cat",
            )
            journal = "\n".join(
                line for line in journal.splitlines()
                if not any(word in line.lower() for word in ("password", "secret", "token"))
            )
            raise RuntimeError(
                f"Linux MCP probe failed (tenant={access['tenant_id']}): "
                f"{probe.stderr[-1200:]}\nService log: {journal[-1600:]}"
            )
        result = json.loads(probe.stdout)
        if result["native_position"] != native["__CURSOR"]:
            raise AssertionError("MCP native cursor differs from independent journalctl")
        if result["matched"] != 1 or result["integrity"] != "unverified":
            raise AssertionError("unexpected match count or integrity")
        if not result["follow"] or not result["follow"]["retry_stable"] or not result["follow"].get("restart_native_position"):
            raise AssertionError("Linux follow/restart did not complete")

        burst_count = int(os.environ.get("PAB_LOG_BURST_COUNT", "100"))
        if not 1 <= burst_count <= 100:
            raise ValueError("PAB_LOG_BURST_COUNT must be 1..100")
        burst_marker = "PAB_LINUX_BURST_" + uuid.uuid4().hex + "_"
        docker("exec", CONTAINER, "bash", "-lc", (
            f"for i in $(seq 1 {burst_count}); do "
            f"logger -t PABLogAcceptance {burst_marker}$(printf '%03d' $i); "
            "done"
        ))
        burst_native = [
            json.loads(line) for line in docker(
                "exec", CONTAINER, "journalctl", "-t", "PABLogAcceptance",
                "--output=json", "--no-pager",
            ).splitlines() if burst_marker in line
        ]
        if len(burst_native) != burst_count or [row["MESSAGE"] for row in burst_native] != [
            f"{burst_marker}{index:03d}" for index in range(1, burst_count + 1)
        ]:
            raise AssertionError("independent journal burst is incomplete or out of order")
        burst_start = int(burst_native[0]["__REALTIME_TIMESTAMP"]) // 1000 - 5000
        burst_end = int(burst_native[-1]["__REALTIME_TIMESTAMP"]) // 1000 + 5000
        burst_full_raw = os.environ.get("PAB_LOG_BURST_FULL_RAW") == "1"
        try:
            burst_probe = subprocess.run(
                [
                sys.executable, str(ROOT / "acceptance/system_log_mcp_live.py"),
                "--mcp", str(MCP), "--device-code", access["device_code"],
                "--source", "journal:local", "--marker", burst_marker,
                "--start-ms", str(burst_start), "--end-ms", str(burst_end),
                "--expected-count", str(burst_count), "--include-positions", "--page-limit", "128",
                "--comparison-requests", os.environ.get("PAB_LOG_COMPARISON_REQUESTS", "0"),
                "--latency-samples", os.environ.get("PAB_LOG_LATENCY_SAMPLES", "0"),
                "--linux-container", CONTAINER,
                *(["--mcp-log-dir", os.environ["PAB_LOG_MCP_LOG_DIR"]] if "PAB_LOG_MCP_LOG_DIR" in os.environ else []),
                *([] if burst_full_raw else ["--skip-full-event-read"]),
                ],
                capture_output=True, text=True, timeout=300 if burst_full_raw else 120,
                env={**os.environ, "PAB_LOG_ACCEPTANCE_DEBUG": "1"},
            )
        except subprocess.TimeoutExpired as error:
            partial = error.stderr or b""
            if isinstance(partial, bytes):
                partial = partial.decode("utf-8", errors="replace")
            raise RuntimeError(f"Linux burst probe timed out: {partial[-1800:]}") from error
        if burst_probe.returncode:
            raise RuntimeError(f"Linux journal burst MCP probe failed: {burst_probe.stderr[-2000:]}")
        burst_result = json.loads(burst_probe.stdout)
        if burst_result["native_positions"] != [row["__CURSOR"] for row in burst_native]:
            raise AssertionError("MCP burst journal cursor manifest differs from independent journalctl")
        if burst_result["message_previews"] != [row["MESSAGE"] for row in burst_native]:
            raise AssertionError("MCP burst journal messages differ from independent journalctl")

        large_marker = "PAB_LINUX_LARGE_" + uuid.uuid4().hex
        send_large = (
            "import ctypes,sys; "
            "marker=sys.argv[1]; "
            "lib=ctypes.CDLL('libsystemd.so.0'); "
            "Iovec=type('Iovec',(ctypes.Structure,),{'_fields_':[('base',ctypes.c_void_p),('size',ctypes.c_size_t)]}); "
            "payload=ctypes.create_string_buffer(b'MESSAGE='+marker.encode()+b'X'*70000); "
            "entry=Iovec(ctypes.cast(payload,ctypes.c_void_p),len(payload.raw)-1); "
            "lib.sd_journal_sendv.argtypes=[ctypes.POINTER(Iovec),ctypes.c_int]; "
            "lib.sd_journal_sendv.restype=ctypes.c_int; "
            "status=lib.sd_journal_sendv(ctypes.byref(entry),1); "
            "assert status>=0,status"
        )
        docker("exec", CONTAINER, "python3", "-c", send_large, large_marker)
        large_native = []
        for _ in range(20):
            large_native = [
                json.loads(line) for line in docker(
                    "exec", CONTAINER, "journalctl", "--all", "--output=json", "--no-pager",
                ).splitlines() if large_marker in line
            ]
            if large_native:
                break
            time.sleep(.25)
        if len(large_native) != 1 or large_native[0].get("MESSAGE") != large_marker + "X" * 70000:
            lengths = [len(str(row.get("MESSAGE", ""))) for row in large_native]
            raise AssertionError(f"independent journal reader large MESSAGE count={len(large_native)} lengths={lengths}")
        large_ms = int(large_native[0]["__REALTIME_TIMESTAMP"]) // 1000
        large_probe = subprocess.run(
            [
                sys.executable, str(ROOT / "acceptance/system_log_mcp_live.py"),
                "--mcp", str(MCP), "--device-code", access["device_code"],
                "--source", "journal:local", "--marker", large_marker,
                "--start-ms", str(large_ms - 5000), "--end-ms", str(large_ms + 5000),
                "--expected-count", "1", "--page-limit", "1",
            ],
            capture_output=True, text=True, timeout=90,
        )
        if large_probe.returncode:
            raise RuntimeError(f"Linux large journal MCP probe failed: {large_probe.stderr[-2000:]}")
        large_result = json.loads(large_probe.stdout)
        if large_result["matched"] != 1 or large_result["native_position"] != large_native[0]["__CURSOR"]:
            raise AssertionError("large journal record lost or native cursor changed")
        if large_result["raw_bytes"] <= 70000:
            raise AssertionError("large journal event was truncated")

        pixels_marker = "PAB_PIXELS_FILE_" + uuid.uuid4().hex
        pixels_path = "/var/lib/pixels-agent-bridge/logs/mcp.log"
        write_pixels = (
            "from datetime import datetime,timezone; from pathlib import Path; "
            f"p=Path({pixels_path!r}); "
            "assert not p.exists(); "
            f"p.write_bytes((datetime.now(timezone.utc).isoformat() + ' INFO {pixels_marker}\\n').encode())"
        )
        docker("exec", CONTAINER, "python3", "-c", write_pixels)
        pixels_raw = subprocess.check_output(["docker", "exec", CONTAINER, "cat", pixels_path])
        pixels_hash = hashlib.sha256(pixels_raw).hexdigest()
        pixels_now = int(time.time() * 1000)
        pixels_probe = subprocess.run(
            [
                sys.executable, str(ROOT / "acceptance/system_log_mcp_live.py"),
                "--mcp", str(MCP), "--device-code", access["device_code"],
                "--source", "pixels-file:mcp.log", "--marker", pixels_marker,
                "--start-ms", str(pixels_now - 5000),
                "--end-ms", str(pixels_now + 5000),
                "--expected-sha256", pixels_hash,
                "--pixels-rotation-check", "--linux-container", CONTAINER,
            ],
            capture_output=True, text=True, timeout=75,
        )
        if pixels_probe.returncode:
            raise RuntimeError(f"Pixels file MCP/rotation probe failed: {pixels_probe.stderr[-2000:]}")
        pixels_result = json.loads(pixels_probe.stdout)
        if not pixels_result["rotation"] or not pixels_result["rotation"]["gap"]:
            raise AssertionError("Pixels file rotation was not reported as a gap")
        print(json.dumps({
            "device_code": access["device_code"], "marker": marker,
            "native_position_matched": True,
            "raw_sha256": result["sha256"],
            "raw_bytes": result["raw_bytes"],
            "integrity": result["integrity"],
            "follow_retry_stable": True,
            "restart_resumed": True,
            "burst_count": burst_result["matched"],
            "burst_positions_matched": True,
            "burst_manifest_sha256": burst_result["manifest_sha256"],
            "burst_full_raw": burst_full_raw,
            "burst_operation_timings_ms": burst_result["operation_timings_ms"],
            "burst_latency": burst_result["latency"],
            "large_message_bytes": 70000,
            "large_event_raw_bytes": large_result["raw_bytes"],
            "large_event_sha256": large_result["sha256"],
            "pixels_file_sha256": pixels_hash,
            "pixels_file_rotation_gap": True,
            "service": "active",
        }))
    finally:
        if inserted and device_id:
            with sqlite3.connect(DATABASE, timeout=10) as db:
                db.execute("delete from device_credentials where device_id=?", (device_id,))
                db.commit()
        if started:
            docker("stop", CONTAINER)


if __name__ == "__main__":
    main()
