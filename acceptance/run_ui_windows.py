"""Run the actual isolated UI backend against our disposable WinForms fixture.

Build: cargo build -p pab-desktop-control --example ui_worker --locked
Launch ui_controls.ps1 into a fresh directory, then pass that directory here.
This validates product backend/worker code, independently of MCP host acceptance.
"""
import ctypes
import json
import queue
import subprocess
import sys
import threading
import time
import uuid
from pathlib import Path


def main():
    root = Path(sys.argv[1]).resolve()
    info = json.loads((root / "ready.json").read_text(encoding="utf-8-sig"))
    user = ctypes.WinDLL("user32", use_last_error=True)
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.OpenProcess.restype = ctypes.c_void_p
    handle = kernel.OpenProcess(0x1000, False, info["pid"])
    assert handle
    times = [ctypes.c_uint64() for _ in range(4)]
    assert kernel.GetProcessTimes(ctypes.c_void_p(handle), *(ctypes.byref(v) for v in times))
    kernel.CloseHandle(ctypes.c_void_p(handle))
    hwnd = ctypes.c_void_p(info["hwnd"])
    user.ShowWindow(hwnd, 5)
    key = "PAB_UI_FIXTURE_" + str(uuid.uuid4())
    assert user.SetPropW(hwnd, ctypes.c_wchar_p(key), ctypes.c_void_p(77))
    owner = {"connection": str(uuid.uuid4()), "helper_instance": str(uuid.uuid4())}
    ticket = {"window_ref": str(uuid.uuid4()), "id": info["hwnd"], "pid": info["pid"],
              "process_identity": f"windows_filetime:{times[0].value}", "marker_key": key, "marker": 77}
    child = subprocess.Popen([str(Path("target/debug/examples/ui_worker.exe").resolve())],
                             stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                             encoding="utf-8", creationflags=subprocess.CREATE_NO_WINDOW)
    replies = queue.Queue()

    def read():
        for line in child.stdout:
            replies.put(json.loads(line))
        replies.put(None)

    reader = threading.Thread(target=read)
    reader.start()

    def send(command, identity=owner, timeout=5):
        request_id = str(uuid.uuid4())
        child.stdin.write(json.dumps({"request_id": request_id, "owner": identity, "command": command}) + "\n")
        child.stdin.flush()
        result = replies.get(timeout=timeout)
        assert result and result["request_id"] == request_id, result
        return result["snapshot"]

    def query(request, window=None, identity=owner, timeout=5):
        return send({"command": "query", "ticket": window, "request": request}, identity, timeout)

    def act(reference, action, expected=None):
        result = query({"operation": "action", "element_ref": reference, "action": action,
                        "expected": expected or {}, "timeout_ms": 5000})
        assert result["outcome"] == "completed", result
        return result

    try:
        tree_request = {"operation": "query", "scope": {"type": "window", "window_ref": ticket["window_ref"]}}
        tree = query(tree_request, ticket)
        assert not tree["error_code"] and not tree["truncated"], tree
        refs = {e["name"]: e["element_ref"] for e in tree["elements"] if e["name"]}
        assert sum(e["name"] == "Fixture duplicate" for e in tree["elements"]) == 2
        secure = query({"operation":"get", "element_ref":refs["Fixture secure"], "include_value":True})
        assert secure["elements"][0]["protected"] and secure["elements"][0]["value"] is None, secure
        for name, action in [("Fixture readonly", {"type":"set_value","value":"must-not-write"}),
                             ("Fixture secure", {"type":"set_value","value":"must-not-write"}),
                             ("Fixture disabled", {"type":"invoke"})]:
            denied = query({"operation":"action", "element_ref":refs[name], "action":action})
            assert denied["outcome"] == "rejected" and denied["action_dispatched"] is False, denied
        for name in ["Fixture radio", "Fixture second"]:
            assert act(refs[name], {"type":"select"})["verification"] == "matched"
        edit = refs["Fixture input"]
        for value in ["PAB 中文🙂", "", "PAB 中文🙂"]:
            assert act(edit, {"type": "set_value", "value": value})["verification"] == "matched"
        assert act(refs["Fixture option"], {"type": "set_checked", "checked": True})["verification"] == "matched"
        same = act(refs["Fixture option"], {"type": "set_checked", "checked": True})
        assert same["action_dispatched"] is False
        act(refs["Apply fixture"], {"type": "invoke"})
        for _ in range(30):
            if (root / "result.json").exists():
                break
            time.sleep(.1)
        actual = json.loads((root / "result.json").read_text(encoding="utf-8-sig"))
        assert actual == {"value": "PAB 中文🙂", "clicks": 1, "checked": True, "radio":True, "selected":1}, actual
        stranger = {**owner, "connection": str(uuid.uuid4())}
        rejected = query({"operation": "get", "element_ref": edit, "include_value": True}, identity=stranger)
        assert rejected["error_code"] == "stale_element", rejected
        # Removing the helper's marker must invalidate existing native references.
        user.RemovePropW(hwnd, ctypes.c_wchar_p(key))
        stale = query({"operation": "get", "element_ref": edit, "include_value": False})
        assert stale["error_code"] == "stale_window", stale
        report = {"actual_value_matches": True, "actual_clicks": actual["clicks"],
                  "actual_checked": actual["checked"], "set_checked_no_replay": True,
                  "cross_connection_rejected": True, "stale_window_rejected": True,
                  "secure_redacted":True, "readonly_disabled_secure_rejected":True,
                  "radio_and_list_actual_selected":True, "empty_unicode_values":True,
                  "nodes": len(tree["elements"])}
        (root / "backend-report.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
        print(json.dumps(report))
    finally:
        child.kill()
        child.wait(timeout=5)
        child.stdin.close()
        reader.join(timeout=5)
        assert not reader.is_alive()
        user.RemovePropW(hwnd, ctypes.c_wchar_p(key))
        (root / "stop").touch()


if __name__ == "__main__":
    main()
