"""Create or resume a follow cursor from a fresh, short-lived MCP process.

This separates the client connection from the Executor's durable scan. Run
``init``, exit, write a native event while this process is absent, then run
``resume`` with the printed cursor.
"""

import argparse
import json
import os
from pathlib import Path
import queue
import subprocess
import threading
import time


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("phase", choices=("init", "resume", "inventory"))
parser.add_argument("--mcp", type=Path, required=True)
parser.add_argument("--device-code", required=True)
parser.add_argument("--source", required=True)
parser.add_argument("--marker", required=True)
parser.add_argument("--cursor")
args = parser.parse_args()
if args.phase == "resume" and not args.cursor:
    parser.error("resume requires --cursor")

environment = os.environ.copy()
if not environment.get("PAB_CONTROL_URL") or not environment.get("PAB_RELAY_URLS"):
    settings = Path(os.environ.get("PAB_INSTALLED_SETTINGS", r"C:\Program Files\PixelsAgentBridge\settings.json"))
    if settings.is_file():
        config = json.loads(settings.read_text(encoding="utf-8-sig"))
        environment.setdefault("PAB_CONTROL_URL", config["control_url"])
        environment.setdefault("PAB_RELAY_URLS", config["relay_urls"])

process = subprocess.Popen(
    [str(args.mcp.resolve())], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
    stderr=subprocess.PIPE, env=environment,
    creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
)
responses = queue.Queue()


def read_stdout():
    for line in process.stdout:
        responses.put(json.loads(line))


def drain_stderr():
    for _ in process.stderr:
        pass


threading.Thread(target=read_stdout, daemon=True).start()
threading.Thread(target=drain_stderr, daemon=True).start()
serial = 0


def rpc(method, params=None):
    global serial
    serial += 1
    message = {"jsonrpc": "2.0", "id": serial, "method": method}
    if params is not None:
        message["params"] = params
    process.stdin.write((json.dumps(message) + "\n").encode())
    process.stdin.flush()
    for _ in range(60):
        try:
            reply = responses.get(timeout=1)
        except queue.Empty:
            if process.poll() is not None:
                raise RuntimeError(f"MCP exited {process.returncode}")
            continue
        if reply.get("id") == serial:
            if "error" in reply:
                raise RuntimeError(reply["error"])
            return reply["result"]
    raise TimeoutError(method)


def tool(name, arguments):
    reply = rpc("tools/call", {"name": name, "arguments": arguments})
    if reply.get("isError"):
        raise RuntimeError(reply.get("content"))
    return reply.get("structuredContent") or json.loads(reply["content"][0]["text"])


def completed(name, arguments):
    value = tool(name, arguments)
    operation = value.get("operation_ref", {}).get("operation_id")
    for _ in range(90):
        result = value.get("result", {})
        if result.get("state") == "completed":
            return result["data"]["snapshot"]
        if result.get("state") in {"failed", "interrupted", "unconfirmed"}:
            raise RuntimeError(result)
        if not operation:
            raise RuntimeError("missing operation reference")
        value = tool("pab_get_operation", {
            "device_code": args.device_code, "operation_id": operation,
            "wait_until": "complete", "wait_ms": 1000,
        })
    raise TimeoutError(name)


try:
    rpc("initialize", {
        "protocolVersion": "2025-11-25", "capabilities": {},
        "clientInfo": {"name": "system-log-reconnect", "version": "1.0"},
    })
    process.stdin.write(b'{"jsonrpc":"2.0","method":"notifications/initialized"}\n')
    process.stdin.flush()
    assert tool("pab_connect", {"device_code": args.device_code, "wait_ms": 10000})["connected"]
    if args.phase == "inventory":
        after = None
        availability = {}
        examples = {}
        for _ in range(100):
            selection = {"device_code": args.device_code, "limit": 32}
            if after:
                selection["after"] = after
            snapshot = completed("pab_system_log_sources", selection)
            for source in snapshot["sources"]:
                state = source["availability"]
                availability[state] = availability.get(state, 0) + 1
                examples.setdefault(state, source["source_id"])
            after = snapshot["next_source"]
            if not after:
                break
        else:
            raise TimeoutError("source inventory exceeded 100 pages")
        print(json.dumps({"availability": availability, "example_source_ids": examples}))
    elif args.phase == "init":
        page = completed("pab_system_log_read", {
            "device_code": args.device_code, "sources": [args.source],
            "start_unix_ms": int(time.time() * 1000) - 60_000,
            "mode": "follow", "message_contains": args.marker, "limit": 1,
        })
        assert not page["events"] and page["cursor"] and page["has_more"], page
        print(json.dumps({"cursor": page["cursor"], "capture_state": page["capture_state"]}))
    else:
        cursor = args.cursor
        for _ in range(30):
            page = completed("pab_system_log_read", {
                "device_code": args.device_code, "cursor": cursor, "limit": 1,
            })
            if page["events"]:
                assert len(page["events"]) == 1
                event = page["events"][0]
                assert args.marker in event["message_preview"], event
                print(json.dumps({
                    "found": True, "source_id": event["source_id"],
                    "native_position": event["native_position"],
                    "integrity": page["integrity"],
                    "capture_state": page["capture_state"],
                }))
                break
            assert page["capture_state"] in {"following", "catching_up", "lagging"}, page
            cursor = page["cursor"]
            time.sleep(.2)
        else:
            raise TimeoutError("event written while disconnected was not returned")
finally:
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)
