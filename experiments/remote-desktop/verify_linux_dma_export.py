"""Isolated VM check of production DMA exporter -> user capture -> decoder.

No installed program or configuration is changed. Requires the authorized test
VM's active, unlocked UID 1000 GNOME Wayland session. The explicit lock mode
requires prior authorization and restores the session; no mode sends input.
The worker's ordinary capture selection and supervisor are not exercised here.
"""
import ctypes
import json
import os
import pathlib
import platform
import socket
import select
import struct
import subprocess
import sys
import time


def main():
    assert os.geteuid() == 0 and platform.machine() == "x86_64"
    assert len(sys.argv) in (4, 5)
    worker, codec, test = [str(pathlib.Path(p).resolve(strict=True)) for p in sys.argv[1:4]]
    mode = sys.argv[4] if len(sys.argv) == 5 else "animation"
    assert mode in ("animation", "cancel", "lock", "pipewire")
    def state():
        session = subprocess.check_output(["loginctl", "show-seat", "seat0", "-p", "ActiveSession", "--value"], text=True).strip()
        info = dict(line.split("=", 1) for line in subprocess.check_output(
            ["loginctl", "show-session", session, "-p", "User", "-p", "Type", "-p", "Class",
             "-p", "Active", "-p", "LockedHint"], text=True).splitlines())
        return session, info
    before = state()
    assert before[1] == {"User": "1000", "Type": "wayland", "Class": "user", "Active": "yes", "LockedHint": "no"}
    candidates = []
    for process in pathlib.Path("/proc").iterdir():
        if not process.name.isdecimal():
            continue
        try:
            if process.stat().st_uid == 1000 and (process / "comm").read_text().strip() == "gnome-shell" and "user-1000.slice" in (process / "cgroup").read_text():
                candidates.append(process)
        except OSError:
            pass
    assert len(candidates) == 1
    process = candidates[0]
    original = next(int(fd.name) for fd in (process / "fd").iterdir() if os.readlink(fd) == "/dev/dri/card0")
    libc = ctypes.CDLL(None, use_errno=True)
    pidfd = libc.syscall(434, int(process.name), 0)
    assert pidfd >= 0, ctypes.get_errno()
    duplicate = libc.syscall(438, pidfd, original, 0)
    os.close(pidfd)
    assert duplicate >= 0, ctypes.get_errno()
    values = dict(item.split(b"=", 1) for item in (process / "environ").read_bytes().split(b"\0") if b"=" in item)
    environment = {key.decode(): value.decode() for key, value in values.items()
                   if key in (b"DISPLAY", b"WAYLAND_DISPLAY", b"XAUTHORITY", b"DBUS_SESSION_BUS_ADDRESS")}
    environment.update(PATH="/usr/bin:/bin", HOME="/home/hy", USER="hy", LOGNAME="hy",
                       XDG_RUNTIME_DIR="/run/user/1000", XDG_SESSION_TYPE="wayland", LANG="C.UTF-8")
    assert "DBUS_SESSION_BUS_ADDRESS" in environment
    render = os.open("/dev/dri/renderD128", os.O_RDWR | os.O_CLOEXEC)
    server, client = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET | socket.SOCK_CLOEXEC)
    exporter = consumer = None
    try:
        inventory = subprocess.run([worker, "inventory"], user=1000, group=1000, extra_groups=[],
            env=environment, capture_output=True, text=True, check=True, timeout=8)
        screens = json.loads(inventory.stdout)
        assert len(screens) == 1 and screens[0]["id"].startswith("gnome:")
        connector = screens[0]["id"].split(":", 1)[1]
        if mode != "pipewire":
            exporter = subprocess.Popen([worker, "export-dma", "borrowed", str(duplicate), str(render),
                str(server.fileno()), connector], pass_fds=(duplicate, render, server.fileno()),
                stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, start_new_session=True)
        consumer_command = (["/usr/bin/python3", test, worker, codec, "--dma-fd", str(client.fileno())]
            if mode == "animation" else [worker, "capture-dma", screens[0]["id"], "15", "desktop", str(client.fileno())])
        if mode == "pipewire": consumer_command = ["/usr/bin/python3", test, worker, codec]
        consumer = subprocess.Popen(consumer_command,
            pass_fds=() if mode == "pipewire" else (client.fileno(),), user=1000, group=1000, extra_groups=[], env=environment,
            stdin=subprocess.DEVNULL if mode in ("animation", "pipewire") else subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True, bufsize=0)
        server.close(); client.close()
        if exporter: assert pathlib.Path(f"/proc/{exporter.pid}").stat().st_uid == 0
        assert pathlib.Path(f"/proc/{consumer.pid}").stat().st_uid == 1000
        boundary = None
        if mode not in ("animation", "pipewire"):
            def read(count):
                data = bytearray(); deadline = time.monotonic() + 5
                while len(data) < count:
                    assert time.monotonic() < deadline, "Capture response timed out"
                    if not select.select([consumer.stdout], [], [], .05)[0]:
                        continue
                    part = os.read(consumer.stdout.fileno(), count - len(data))
                    assert part, "Capture ended before boundary test"
                    data.extend(part)
                return bytes(data)
            assert struct.unpack("<III", read(12)) == (0x31424150, screens[0]["pixel_width"], screens[0]["pixel_height"])
            for index in range(3):
                consumer.stdin.write(b"\x01"); consumer.stdin.flush()
                size, sequence, _, key = struct.unpack("<IQQI", read(24))
                assert 0 < size <= 16 * 1024 * 1024 and sequence == index and (index or key == 1)
                read(size)
            if mode == "cancel":
                consumer.terminate()
                out, err = consumer.communicate(timeout=5)
                assert consumer.returncode == -15 and not out
                boundary = {"case": "capture_sigterm", "initial_frames": 3, "bytes_after_boundary": len(out)}
            else:
                unit = f"pab-dma-lock-{os.getpid()}"
                subprocess.run(["systemd-run", "--unit=" + unit, "--on-active=45s", "--property=Type=oneshot",
                    "--property=TimeoutStartSec=10", "/usr/bin/loginctl", "unlock-session", before[0]],
                    capture_output=True, check=True, timeout=5)
                try:
                    subprocess.run(["loginctl", "lock-session", before[0]], check=True, timeout=5)
                    for _ in range(50):
                        if state()[1]["LockedHint"] == "yes": break
                        time.sleep(.1)
                    assert state()[1]["LockedHint"] == "yes"
                    consumer.stdin.write(b"\x01"); consumer.stdin.flush()
                    out, err = consumer.communicate(timeout=8)
                    assert consumer.returncode == 1 and not out
                    assert b"Display is asleep" in err or b"Lock state changed" in err
                    boundary = {"case": "locked_generation", "initial_frames": 3,
                        "bytes_after_boundary": len(out), "locked_during_check": state()[1]["LockedHint"] == "yes"}
                finally:
                    subprocess.run(["loginctl", "unlock-session", before[0]], check=True, timeout=5)
                    for _ in range(50):
                        if state() == before: break
                        time.sleep(.1)
                    assert state() == before
                    subprocess.run(["systemctl", "stop", unit + ".timer", unit + ".service"], capture_output=True, timeout=5)
        else:
            out, err = consumer.communicate(timeout=35)
        export_code = exporter.wait(timeout=5) if exporter else None
        export_error = exporter.stderr.read().decode(errors="replace") if exporter else None
        report = {"session": before[0], "inventory": screens, "exporter_uid": 0 if exporter else None,
            "consumer_uid": 1000, "exporter_exit": export_code, "exporter_error": export_error,
            "consumer_exit": consumer.returncode, "consumer_output": out.decode(errors="replace"),
            "consumer_error": err.decode(errors="replace"), "session_unchanged": state() == before, "boundary": boundary}
        print(json.dumps(report), flush=True)
        assert (mode not in ("animation", "pipewire") or consumer.returncode == 0)
        assert (export_code is None if mode == "pipewire" else export_code == 0) and report["session_unchanged"]
    finally:
        server.close(); client.close()
        for child in (consumer, exporter):
            if child:
                if child.poll() is None:
                    os.killpg(child.pid, 15)
                try:
                    child.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    os.killpg(child.pid, 9); child.wait(timeout=3)
        os.close(duplicate); os.close(render)


if __name__ == "__main__":
    main()
