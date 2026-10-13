"""Verify the installed Executor through a freshly built MCP binary.

Supply a unique marker already present in a native source. Independently
confirm its native count/position before interpreting this probe as acceptance.
"""

import argparse
import base64
from datetime import datetime
import hashlib
import json
import os
from pathlib import Path
import queue
import re
import subprocess
import sys
import threading
import time
import uuid


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--mcp", required=True, type=Path)
parser.add_argument("--device-code", required=True)
parser.add_argument("--source", required=True)
parser.add_argument("--marker", required=True)
parser.add_argument("--start-ms", type=int, required=True)
parser.add_argument("--end-ms", type=int, required=True)
parser.add_argument("--expected-sha256")
parser.add_argument("--expected-count", type=int, default=1)
parser.add_argument("--page-limit", type=int, default=2, choices=range(1, 129))
parser.add_argument("--include-positions", action="store_true", help="Include every native position for independent manifest comparison")
parser.add_argument("--skip-full-event-read", action="store_true", help="Compare listed positions and previews; a separate single-event probe must verify raw bytes")
parser.add_argument("--latency-samples", type=int, default=0, help="Sequential native follow samples; measures local write request through MCP result")
parser.add_argument("--comparison-requests", type=int, default=0, help="Time the same remote path with small system-info queries")
parser.add_argument("--mcp-log-dir", type=Path, help="Isolated MCP log directory for path-classified latency samples")
parser.add_argument("--implicit-sources", action="store_true", help="Omit source selection and verify the default set")
parser.add_argument("--follow-check", action="store_true", help="Write a native test event after an empty follow page")
parser.add_argument("--restart-check", action="store_true", help="Restart the Executor between two follow pages")
parser.add_argument("--linux-container", help="Disposable systemd container for independent Linux event writes/restart")
parser.add_argument("--pixels-rotation-check", action="store_true", help="Rotate a disposable Pixels log file and verify a gap")
parser.add_argument("--unfiltered-follow-count", type=int, default=0, help="Generate and verify this many unfiltered Linux journal events through the follow cursor")
args = parser.parse_args()
if args.expected_count < 0:
    parser.error("--expected-count must be nonnegative")
if args.unfiltered_follow_count and (args.source != "journal:local" or not args.linux_container or not 1 <= args.unfiltered_follow_count <= 2000):
    parser.error("--unfiltered-follow-count requires a Linux container and 1..2000 events")
if not 0 <= args.latency_samples <= 200:
    parser.error("--latency-samples must be 0..200")
if args.latency_samples and args.source not in {"eventlog:Application", "unified:oslogstore", "journal:local"}:
    parser.error("--latency-samples requires an eventlog, OSLogStore, or journal source")
if args.latency_samples and args.source == "journal:local" and not args.linux_container:
    parser.error("journal latency samples require --linux-container")
if not 0 <= args.comparison_requests <= 20:
    parser.error("--comparison-requests must be 0..20")
if args.restart_check and not args.follow_check:
    parser.error("--restart-check requires --follow-check")
MAC_SOURCES = {"unified:local", "unified:oslogstore"}


def debug(message):
    if os.environ.get("PAB_LOG_ACCEPTANCE_DEBUG") == "1":
        print(message, file=sys.stderr, flush=True)

environment = os.environ.copy()
if args.mcp_log_dir:
    args.mcp_log_dir.mkdir(parents=True, exist_ok=True)
    environment["PAB_LOG_DIR"] = str(args.mcp_log_dir.resolve())
if not environment.get("PAB_CONTROL_URL") or not environment.get("PAB_RELAY_URLS"):
    settings_path = Path(os.environ.get("PAB_INSTALLED_SETTINGS", r"C:\Program Files\PixelsAgentBridge\settings.json"))
    if settings_path.is_file():
        settings = json.loads(settings_path.read_text(encoding="utf-8-sig"))
        environment.setdefault("PAB_CONTROL_URL", settings["control_url"])
        environment.setdefault("PAB_RELAY_URLS", settings["relay_urls"])

process = subprocess.Popen(
    [str(args.mcp.resolve())],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    env=environment,
    creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
)
responses = queue.Queue()
errors = []
operation_timings_ms = {}


def read_stdout():
    for line in process.stdout:
        try:
            responses.put(json.loads(line))
        except json.JSONDecodeError:
            responses.put({"invalid_line": line[:200].decode(errors="replace")})


def read_stderr():
    for line in process.stderr:
        if len(errors) < 20:
            errors.append(line.decode(errors="replace")[:300])


threading.Thread(target=read_stdout, daemon=True).start()
threading.Thread(target=read_stderr, daemon=True).start()
serial = 0


def rpc(method, params=None, timeout=30):
    global serial
    serial += 1
    message = {"jsonrpc": "2.0", "id": serial, "method": method}
    if params is not None:
        message["params"] = params
    process.stdin.write((json.dumps(message) + "\n").encode())
    process.stdin.flush()
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            response = responses.get(timeout=min(1, max(.01, deadline - time.monotonic())))
        except queue.Empty:
            if process.poll() is not None:
                raise RuntimeError(f"MCP exited {process.returncode}: {errors[-3:]}")
            continue
        if response.get("id") == serial:
            if "error" in response:
                raise RuntimeError(response["error"])
            return response["result"]
    raise TimeoutError(f"MCP {method} timed out")


def tool(name, arguments, timeout=60):
    response = rpc("tools/call", {"name": name, "arguments": arguments}, timeout)
    if response.get("isError"):
        raise RuntimeError(f"{name}: {response.get('content')}")
    return response.get("structuredContent") or json.loads(response["content"][0]["text"])


def completed(name, arguments):
    started = time.perf_counter()
    debug(f"call {name} cursor={arguments.get('cursor')}")
    value = tool(name, arguments)
    operation = value.get("operation_ref", {}).get("operation_id")
    for _ in range(90):
        result = value.get("result", {})
        if result.get("state") in {"completed", "failed", "interrupted", "unconfirmed"}:
            if result["state"] != "completed":
                raise RuntimeError(f"{name}: {result['state']}: {result.get('error')}")
            debug(f"done {name}")
            operation_timings_ms.setdefault(name, []).append(round((time.perf_counter() - started) * 1000, 1))
            return result
        if not operation:
            raise RuntimeError(f"{name}: no result or operation reference")
        value = tool("pab_get_operation", {
            "device_code": args.device_code,
            "operation_id": operation,
            "wait_until": "complete",
            "wait_ms": 1000,
        })
    raise TimeoutError(f"{name} did not complete")


def native_command(program, argv):
    launched = tool("pab_run_command", {
        "device_code": args.device_code,
        "program": program,
        "args": argv,
    })
    task_id = launched["operation_ref"]["task_id"]
    for _ in range(30):
        task = tool("pab_get_task", {
            "device_code": args.device_code,
            "task_id": task_id,
            "wait_until": "complete",
            "wait_ms": 1000,
        })["snapshot"]
        if task["state"] in {"succeeded", "failed", "cancelled"}:
            assert task["state"] == "succeeded" and task["completion"]["exit_code"] == 0, task
            return
    raise TimeoutError(f"native command did not finish: {program}")


try:
    rpc("initialize", {
        "protocolVersion": "2025-11-25",
        "capabilities": {},
        "clientInfo": {"name": "system-log-acceptance", "version": "1.0"},
    })
    process.stdin.write(b'{"jsonrpc":"2.0","method":"notifications/initialized"}\n')
    process.stdin.flush()
    names = {item["name"] for item in rpc("tools/list")["tools"]}
    required = {"pab_system_log_sources", "pab_system_log_read", "pab_system_log_event_read"}
    assert required <= names, required - names
    for _ in range(12):
        if tool("pab_connect", {"device_code": args.device_code, "wait_ms": 10000}).get("connected"):
            break
    else:
        raise TimeoutError("device did not connect within 120 seconds")
    inventory = []
    after = None
    for _ in range(100):
        selection = {"device_code": args.device_code, "limit": 32}
        if after:
            selection["after"] = after
        sources = completed("pab_system_log_sources", selection)
        snapshot = sources["data"]["snapshot"]
        inventory.extend(snapshot["sources"])
        after = snapshot["next_source"]
        if not after:
            break
    else:
        raise TimeoutError("source inventory did not finish")
    source = next((s for s in inventory if s["source_id"] == args.source), None)
    assert source and source["availability"] == "readable", (args.source, source)

    cursor = None
    found = []
    progress = None
    for _ in range(100):
        selection = {"device_code": args.device_code, "limit": args.page_limit}
        if cursor:
            selection["cursor"] = cursor
        else:
            selection.update({
                "start_unix_ms": args.start_ms,
                "end_unix_ms": args.end_ms,
                "message_contains": args.marker,
            })
            if not args.implicit_sources:
                selection["sources"] = [args.source]
        result = completed("pab_system_log_read", selection)
        page = result["data"]["snapshot"]
        assert page["type"] == "page", page
        found.extend(page["events"])
        debug(f"range read matched={len(found)} has_more={page['has_more']} integrity={page['integrity']}")
        progress = page["sources"]
        if not page["has_more"]:
            break
        cursor = page["cursor"]
        assert cursor, page
    else:
        raise TimeoutError("log scan did not reach the selected range end")
    assert len(found) == args.expected_count, {"matches": len(found), "progress": progress}
    if args.implicit_sources and args.source == "unified:oslogstore":
        assert not any(item["source_id"] == "unified:local" for item in progress), progress
    hashes = []
    for event in found:
        assert event["source_id"] == args.source
        if args.skip_full_event_read:
            assert args.marker in event["message_preview"]
            hashes.append(event["sha256"])
            continue
        raw = bytearray()
        while True:
            chunk = completed("pab_system_log_event_read", {
                "device_code": args.device_code,
                "event_ref": event["event_ref"],
                "offset": len(raw),
                "max_bytes": 8192,
            })["data"]["snapshot"]
            assert chunk["type"] == "event_chunk", chunk
            raw.extend(base64.b64decode(chunk["data_base64"]))
            if chunk["next_offset"] == chunk["total_bytes"]:
                break
            assert chunk["next_offset"] == len(raw), chunk
        if args.source == "journal:local":
            fields = json.loads(raw)
            messages = [
                base64.b64decode(field["value_base64"])
                for field in fields if field["name"] == "MESSAGE"
            ]
            assert len(messages) == 1 and args.marker.encode() in messages[0]
        else:
            assert args.marker.encode() in raw
        digest = hashlib.sha256(raw).hexdigest()
        assert digest == chunk["sha256"] == event["sha256"]
        hashes.append(digest)
    assert len({event["native_position"] for event in found}) == len(found)
    if args.expected_sha256:
        assert len(hashes) == 1 and hashes[0] == args.expected_sha256.lower()
    for _ in range(args.comparison_requests):
        completed("pab_system_info", {"device_code": args.device_code})
    event = found[0] if found else None
    manifest_sha256 = hashlib.sha256("\n".join(sorted(hashes)).encode()).hexdigest()
    unfiltered_result = None
    if args.unfiltered_follow_count:
        prefix = "PAB_UNFILTERED_" + uuid.uuid4().hex + "_"
        first = completed("pab_system_log_read", {
            "device_code": args.device_code,
            "sources": [args.source],
            "start_unix_ms": int(time.time() * 1000) - 5000,
            "mode": "follow",
            "limit": 16,
        })["data"]["snapshot"]
        follow_cursor = first["cursor"]
        assert follow_cursor and first["integrity"] == "scanning", first
        # Drain the pre-existing journal before the measured burst.
        for _ in range(100):
            if first["capture_state"] == "following":
                break
            first = completed("pab_system_log_read", {
                "device_code": args.device_code, "cursor": follow_cursor, "limit": 16,
            })["data"]["snapshot"]
            follow_cursor = first["cursor"]
            assert first["integrity"] == "scanning", first
        else:
            raise TimeoutError("unfiltered follow did not reach the initial journal tail")
        count = args.unfiltered_follow_count
        subprocess.run([
            "docker", "exec", args.linux_container, "bash", "-lc",
            f"for i in $(seq 1 {count}); do logger -t PABLogAcceptance {prefix}$(printf '%04d' $i); done",
        ], check=True, capture_output=True, timeout=90)
        native = [json.loads(line) for line in subprocess.check_output([
            "docker", "exec", args.linux_container, "journalctl", "-t", "PABLogAcceptance",
            "--output=json", "--no-pager",
        ], text=True).splitlines() if prefix in line]
        expected = [f"{prefix}{index:04d}" for index in range(1, count + 1)]
        assert [row["MESSAGE"] for row in native] == expected, "independent journal burst is incomplete"
        seen = []
        states = []
        for _ in range(300):
            page = completed("pab_system_log_read", {
                "device_code": args.device_code, "cursor": follow_cursor, "limit": 16,
            })["data"]["snapshot"]
            follow_cursor = page["cursor"]
            assert page["integrity"] == "scanning" and follow_cursor, page
            markers = [event for event in page["events"] if prefix in event["message_preview"]]
            seen.extend(markers)
            states.append(page["capture_state"])
            assert page["capture_state"] in {"catching_up", "lagging", "following"}, page
            if markers and len(seen) < count:
                assert page["capture_state"] != "following", "reported following with unread burst events"
            if len(seen) == count and page["capture_state"] == "following":
                break
            if not page["events"]:
                time.sleep(.05)
        else:
            raise TimeoutError(f"unfiltered follow did not catch up: {len(seen)}/{count}")
        assert [event["message_preview"] for event in seen] == expected
        assert [event["native_position"] for event in seen] == [row["__CURSOR"] for row in native]
        assert "catching_up" in states or "lagging" in states, states
        unfiltered_result = {"count": count, "native_positions_matched": True,
                             "states": {state: states.count(state) for state in sorted(set(states))},
                             "rounds": len(states), "final_state": states[-1]}
    latency_result = None
    if args.latency_samples:
        prefix = "PAB_LATENCY_" + uuid.uuid4().hex + "_"
        first = completed("pab_system_log_read", {
            "device_code": args.device_code,
            "sources": [args.source],
            "start_unix_ms": int(time.time() * 1000) - 60000,
            "mode": "follow",
            "message_contains": prefix,
            "limit": 2,
        })["data"]["snapshot"]
        assert first["has_more"] and not first["events"] and first["cursor"], first
        follow_cursor = first["cursor"]
        samples_ms = []
        sample_intervals = []
        for index in range(1, args.latency_samples + 1):
            message = f"{prefix}{index:03d}"
            started_unix_ms = time.time_ns() // 1_000_000
            started = time.perf_counter()
            if args.source == "eventlog:Application":
                native_command("eventcreate.exe", [
                    "/L", "APPLICATION", "/T", "INFORMATION", "/ID", "905",
                    "/SO", "PABLogAcceptance", "/D", message,
                ])
            elif args.source == "journal:local":
                subprocess.run(["docker", "exec", args.linux_container, "logger", "-t", "PABLogAcceptance", message], check=True, capture_output=True)
            else:
                native_command("/usr/bin/logger", ["-t", "PABLogAcceptance", message])
            for _ in range(40):
                follow = completed("pab_system_log_read", {
                    "device_code": args.device_code,
                    "cursor": follow_cursor,
                    "limit": 2,
                })["data"]["snapshot"]
                follow_cursor = follow["cursor"]
                assert follow_cursor and follow["integrity"] not in {"gap", "partial"}, follow
                if follow["events"]:
                    assert len(follow["events"]) == 1 and message in follow["events"][0]["message_preview"], follow
                    samples_ms.append(round((time.perf_counter() - started) * 1000, 1))
                    sample_intervals.append((started_unix_ms, time.time_ns() // 1_000_000))
                    break
                time.sleep(.05)
            else:
                raise TimeoutError(f"latency sample {index} was not observed")
        ordered_ms = sorted(samples_ms)
        latency_result = {
            "count": len(samples_ms),
            "p50_ms": ordered_ms[(len(ordered_ms) - 1) * 50 // 100],
            "p95_ms": ordered_ms[(len(ordered_ms) - 1) * 95 // 100],
            "p99_ms": ordered_ms[(len(ordered_ms) - 1) * 99 // 100],
            "max_ms": ordered_ms[-1],
            "samples_ms": samples_ms,
            "measurement": "local monotonic time from remote native log-write request to MCP follow page; includes command dispatch",
        }
        if args.mcp_log_dir:
            path_events = []
            pattern = re.compile(r'^(\S+).*PAB connection path selected selected_path="(direct|relay|unknown)"')
            for line in (args.mcp_log_dir / "mcp.log").read_text(encoding="utf-8").splitlines():
                match = pattern.search(line)
                if match:
                    when_ms = int(datetime.fromisoformat(match[1].replace("Z", "+00:00")).timestamp() * 1000)
                    path_events.append((when_ms, match[2]))
            paths = []
            for begin, end in sample_intervals:
                before = [path for when, path in path_events if when <= begin]
                changed = any(begin < when <= end for when, _ in path_events)
                paths.append("transition" if changed else (before[-1] if before else "unknown"))
            latency_result["path_counts"] = {path: paths.count(path) for path in sorted(set(paths))}
            latency_result["path_events"] = path_events
    follow_result = None
    if args.follow_check:
        assert args.source in {"eventlog:Application", *MAC_SOURCES, "journal:local"}
        follow_marker = "PAB_FOLLOW_" + uuid.uuid4().hex
        first = completed("pab_system_log_read", {
            "device_code": args.device_code,
            "sources": [args.source],
            "start_unix_ms": int(time.time() * 1000) - (2000 if args.source in MAC_SOURCES else 60000),
            "mode": "follow",
            "message_contains": follow_marker,
            "limit": 2,
        })["data"]["snapshot"]
        assert first["type"] == "page" and first["has_more"] and not first["events"], first
        assert first["capture_state"] in {"following", "catching_up"}, first
        assert first["oldest_unread_age_ms"] is None, first
        debug(f"initial follow sources={first['sources']}")
        follow_cursor = first["cursor"]
        assert follow_cursor
        def write_follow_event(suffix):
            message = follow_marker + suffix
            if args.source == "eventlog:Application":
                native_command("eventcreate.exe", ["/L", "APPLICATION", "/T", "INFORMATION", "/ID", "905", "/SO", "PABLogAcceptance", "/D", message])
            elif args.source == "journal:local" and args.linux_container:
                subprocess.run(["docker", "exec", args.linux_container, "logger", "-t", "PABLogAcceptance", message], check=True, capture_output=True)
            else:
                native_command("/usr/bin/logger", ["-t", "PABLogAcceptance", message])

        write_follow_event("_A")
        last_follow_progress = None
        for _ in range(100):
            follow = completed("pab_system_log_read", {
                "device_code": args.device_code,
                "cursor": follow_cursor,
                "limit": 2,
            })["data"]["snapshot"]
            last_follow_progress = follow["sources"]
            debug(f"follow events={len(follow['events'])} sources={last_follow_progress}")
            if args.source in MAC_SOURCES and follow["integrity"] != "scanning":
                raise RuntimeError(f"macOS follow cannot advance: {last_follow_progress}")
            if follow["events"]:
                assert len(follow["events"]) == 1, follow
                assert follow["capture_state"] in {"following", "catching_up", "lagging"}, follow
                follow_event = follow["events"][0]
                assert follow_marker in follow_event["message_preview"], follow_event
                repeated = completed("pab_system_log_read", {
                    "device_code": args.device_code,
                    "cursor": follow_cursor,
                    "limit": 2,
                })["data"]["snapshot"]
                assert repeated["events"] == follow["events"], "retry changed a published page"
                follow_result = {
                    "marker": follow_marker,
                    "native_position": follow_event["native_position"],
                    "retry_stable": True,
                    "initial_capture_state": first["capture_state"],
                    "event_capture_state": follow["capture_state"],
                }
                resume_cursor = follow["cursor"]
                break
            follow_cursor = follow["cursor"]
            time.sleep(.1)
        else:
            raise TimeoutError(f"follow did not return the newly written native event: {last_follow_progress}")
        if args.restart_check:
            if args.source == "eventlog:Application":
                task_name = "PabLogRestart" + uuid.uuid4().hex[:12]
                schedule = (
                    f"$a=New-ScheduledTaskAction -Execute 'powershell.exe' "
                    f"-Argument '-NoProfile -NonInteractive -Command \"Restart-Service PixelsAgentBridgeExecutor -Force\"'; "
                    f"$t=New-ScheduledTaskTrigger -Once -At (Get-Date).AddSeconds(10); "
                    f"Register-ScheduledTask -TaskName '{task_name}' -Action $a -Trigger $t "
                    f"-User 'SYSTEM' -RunLevel Highest -Force | Out-Null"
                )
                native_command("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", schedule])
            elif args.source in MAC_SOURCES:
                task_name = "pab-log-restart-" + uuid.uuid4().hex[:12]
                result_path = "/private/tmp/" + task_name + ".result"
                schedule = (
                    "test -e " + result_path + " && exit 0; "
                    "sleep 8; test -e " + result_path + " && exit 0; "
                    "/bin/launchctl kickstart -k "
                    "system/com.pixelsagentbridge.executor; echo $? > " + result_path
                )
                native_command("/bin/launchctl", ["submit", "-l", task_name, "--", "/bin/sh", "-c", schedule])
            elif args.source == "journal:local" and args.linux_container:
                subprocess.run(["docker", "exec", args.linux_container, "systemctl", "restart", "pixels-agent-bridge-executor.service"], check=True, capture_output=True)
            else:
                task_name = "pab-log-restart-" + uuid.uuid4().hex[:12]
                native_command("/usr/bin/systemd-run", [
                    "--collect", "--unit", task_name, "--on-active=8s",
                    "/usr/bin/systemctl", "restart", "pixels-agent-bridge-executor.service",
                ])
            time.sleep(3 if args.source == "journal:local" and args.linux_container else 15)
            for _ in range(30):
                try:
                    if tool("pab_connect", {"device_code": args.device_code}).get("connected"):
                        break
                except RuntimeError:
                    pass
                time.sleep(1)
            else:
                raise TimeoutError("Executor did not reconnect after scheduled restart")
            if args.source == "eventlog:Application":
                verify = (
                    f"$i=Get-ScheduledTaskInfo -TaskName '{task_name}'; "
                    f"if($i.LastTaskResult -ne 0){{throw 'restart task failed'}}; "
                    f"if((Get-Service PixelsAgentBridgeExecutor).Status -ne 'Running'){{throw 'service stopped'}}; "
                    f"Unregister-ScheduledTask -TaskName '{task_name}' -Confirm:$false"
                )
                native_command("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", verify])
            elif args.source in MAC_SOURCES:
                native_command("/usr/bin/grep", ["-Fx", "0", result_path])
                native_command("/bin/rm", [result_path])
                native_command("/bin/launchctl", ["remove", task_name])
            elif args.source == "journal:local" and args.linux_container:
                status = subprocess.run(["docker", "exec", args.linux_container, "systemctl", "is-active", "pixels-agent-bridge-executor.service"], check=True, capture_output=True, text=True)
                assert status.stdout.strip() == "active"
            else:
                native_command("/usr/bin/systemctl", ["is-active", "pixels-agent-bridge-executor.service"])
            write_follow_event("_B")
            for _ in range(100):
                resumed = completed("pab_system_log_read", {
                    "device_code": args.device_code,
                    "cursor": resume_cursor,
                    "limit": 2,
                })["data"]["snapshot"]
                if resumed["events"]:
                    assert len(resumed["events"]) == 1
                    assert follow_marker + "_B" in resumed["events"][0]["message_preview"]
                    follow_result["restart_native_position"] = resumed["events"][0]["native_position"]
                    break
                resume_cursor = resumed["cursor"]
                time.sleep(.1)
            else:
                raise TimeoutError("follow cursor did not resume after Executor restart")
    rotation_result = None
    if args.pixels_rotation_check:
        assert args.source == "pixels-file:mcp.log" and args.linux_container
        before_rotation = completed("pab_system_log_read", {
            "device_code": args.device_code,
            "sources": [args.source],
            "start_unix_ms": args.start_ms,
            "mode": "follow",
            "message_contains": args.marker,
            "limit": 2,
        })["data"]["snapshot"]
        assert len(before_rotation["events"]) == 1 and before_rotation["cursor"]
        log_path = "/var/lib/pixels-agent-bridge/logs/mcp.log"
        subprocess.run([
            "docker", "exec", args.linux_container, "mv", log_path, log_path + ".1",
        ], check=True, capture_output=True)
        replacement = (
            "from datetime import datetime,timezone; "
            f"open({log_path!r},'wb').write((datetime.now(timezone.utc).isoformat() + ' INFO {args.marker}_B\\n').encode())"
        )
        subprocess.run([
            "docker", "exec", args.linux_container, "python3", "-c", replacement,
        ], check=True, capture_output=True)
        after_rotation = completed("pab_system_log_read", {
            "device_code": args.device_code,
            "cursor": before_rotation["cursor"],
            "limit": 2,
        })["data"]["snapshot"]
        assert not after_rotation["events"] and after_rotation["integrity"] == "gap", after_rotation
        rotation_result = {
            "gap": True,
            "reason": after_rotation["sources"][0]["reason"],
        }
    print(json.dumps({
        "device_code": args.device_code,
        "source": args.source,
        "matched": len(found),
        "native_position": event["native_position"] if event else None,
        **({"native_positions": [item["native_position"] for item in found]} if args.include_positions else {}),
        **({"message_previews": [item["message_preview"] for item in found]} if args.include_positions else {}),
        "raw_bytes": event["total_bytes"] if event else None,
        "sha256": event["sha256"] if event else None,
        "manifest_sha256": manifest_sha256,
        "latency": latency_result,
        "unfiltered_follow": unfiltered_result,
        "integrity": page["integrity"],
        "source_progress": progress,
        "follow": follow_result,
        "rotation": rotation_result,
        "operation_timings_ms": {
            name: {
                "count": len(samples),
                "total": round(sum(samples), 1),
                "p50": sorted(samples)[(len(samples) - 1) * 50 // 100],
                "p95": sorted(samples)[(len(samples) - 1) * 95 // 100],
                "max": max(samples),
            }
            for name, samples in operation_timings_ms.items()
        },
    }, ensure_ascii=False))
finally:
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)
