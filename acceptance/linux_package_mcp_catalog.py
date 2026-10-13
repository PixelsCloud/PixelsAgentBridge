"""Check that an installed headless Linux package starts MCP with log tools."""

import json
import queue
import subprocess
import sys
import threading


container = sys.argv[1]
process = subprocess.Popen(
    ["docker", "exec", "-i", container, "/opt/pixels-agent-bridge/run-mcp.sh"],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    text=True,
    bufsize=1,
)
responses = queue.Queue()


def reader():
    for line in process.stdout:
        responses.put(json.loads(line))


threading.Thread(target=reader, daemon=True).start()


def rpc(serial, method, params=None):
    message = {"jsonrpc": "2.0", "id": serial, "method": method}
    if params is not None:
        message["params"] = params
    process.stdin.write(json.dumps(message) + "\n")
    process.stdin.flush()
    while True:
        result = responses.get(timeout=30)
        if result.get("id") == serial:
            if "error" in result:
                raise RuntimeError(result["error"])
            return result["result"]


try:
    rpc(1, "initialize", {
        "protocolVersion": "2025-11-25",
        "capabilities": {},
        "clientInfo": {"name": "linux-package-log-catalog", "version": "1.0"},
    })
    process.stdin.write('{"jsonrpc":"2.0","method":"notifications/initialized"}\n')
    process.stdin.flush()
    names = {item["name"] for item in rpc(2, "tools/list")["tools"]}
    required = {"pab_system_log_sources", "pab_system_log_read", "pab_system_log_event_read"}
    assert required <= names, required - names
    print(json.dumps({"container": container, "log_tools": sorted(required), "tool_count": len(names)}))
finally:
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)
