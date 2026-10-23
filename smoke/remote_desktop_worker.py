"""Check timed/legacy native frame IPC using synthetic pixels, never the desktop.

Run on an interactive Windows/macOS host with a freshly built media worker.
Only the owned worker is stopped; no installed service or user session changes.
"""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import threading

MAGIC = 0x31424150


def check(worker, software, worker_args=()):
    executable = [str(worker), *worker_args]
    inventory = subprocess.run([*executable, "inventory"], capture_output=True, timeout=15, check=True)
    screens = json.loads(inventory.stdout)
    if not screens:
        raise RuntimeError("An interactive display is required")
    mode = "synthetic-software" if software else "synthetic"
    process = subprocess.Popen([*executable, "capture", screens[0]["id"], "15", mode],
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    tail = bytearray()

    def drain():
        while chunk := process.stderr.read(1024):
            tail.extend(chunk)
            del tail[:-4096]

    reader = threading.Thread(target=drain, daemon=True)
    reader.start()
    deadline = threading.Timer(30, process.kill)
    deadline.start()

    def read(size):
        value = bytearray()
        while len(value) < size:
            chunk = process.stdout.read(size - len(value))
            if not chunk:
                raise RuntimeError("Native IPC ended early: " + tail.decode("utf-8", errors="replace"))
            value.extend(chunk)
        return bytes(value)

    def unpack(fmt):
        return struct.unpack(fmt, read(struct.calcsize(fmt)))

    frames = []
    try:
        magic, width, height = unpack("<III")
        assert magic == MAGIC and 0 < width <= 8192 and 0 < height <= 8192
        pts = 0
        # Alternate layouts on one encoder, including after a budget change.
        for index, command in enumerate([3, 1, 3, 1, 3, 3]):
            if index == 3:
                process.stdin.write(b"\x02" + struct.pack("<I", 750000))
                process.stdin.flush()
                assert unpack("<I")[0] in (MAGIC, MAGIC + 1)
            process.stdin.write(bytes([command]))
            process.stdin.flush()
            size, sequence, timestamp, key = unpack("<IQQI")
            assert 0 < size <= 16 * 1024 * 1024 and sequence == index and timestamp >= pts
            assert key in (0, 1) and (index != 0 or key == 1)
            timing = unpack("<QQ") if command == 3 else None
            if timing:
                assert 0 <= timing[0] < 30_000_000 and 0 < timing[1] < 30_000_000
            packet = read(size)
            assert packet.startswith((b"\x00\x00\x01", b"\x00\x00\x00\x01"))
            frames.append({"id": sequence + 1, "bytes": size, "pts_us": timestamp,
                           "wait_us": timing[0] if timing else None,
                           "encode_us": timing[1] if timing else None,
                           "synthetic_sha256": hashlib.sha256(packet).hexdigest()})
            pts = timestamp
        process.stdin.write(b"\x00")
        process.stdin.flush()
        assert process.wait(timeout=5) == 0
        return {"status": "passed", "synthetic": True, "software_requested": software,
                "width": width, "height": height, "frames": frames}
    finally:
        deadline.cancel()
        if process.poll() is None:
            process.kill()
        process.wait(timeout=5)
        reader.join(timeout=2)
        process.stdin.close()
        process.stdout.close()
        process.stderr.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--worker", type=Path, required=True)
    parser.add_argument("--software", action="store_true")
    parser.add_argument("--worker-arg", action="append", default=[], help="Prefix argument for an embedded worker, e.g. --worker-arg=--media-worker")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    result = check(args.worker.resolve(strict=True), args.software, args.worker_arg)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"status": result["status"], "frames": len(result["frames"])}))


if __name__ == "__main__":
    main()
