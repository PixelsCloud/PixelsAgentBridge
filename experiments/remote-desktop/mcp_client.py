"""Small, sequential stdio client for isolated experiments using existing login."""

import json
import os
from pathlib import Path
import queue
import subprocess
import threading
import time


class McpClient:
    def __init__(self, executable: Path):
        self.process = subprocess.Popen(
            [str(executable.resolve())], stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            text=True, encoding="utf-8",
            creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
        )
        self.messages = queue.Queue()
        self.serial = 0
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()
        try:
            self.info = self.rpc("initialize", {
                "protocolVersion": "2025-03-26", "capabilities": {},
                "clientInfo": {"name": "remote-desktop-experiment", "version": "1"},
            })
            self._send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        except BaseException:
            self.close()
            raise

    def _read(self):
        try:
            for line in self.process.stdout:
                self.messages.put(json.loads(line))
        except Exception as error:
            self.messages.put(error)
        finally:
            self.messages.put(EOFError("MCP stdout closed"))

    def _send(self, message):
        self.process.stdin.write(json.dumps(message) + "\n")
        self.process.stdin.flush()

    def rpc(self, method, params, timeout=45):
        self.serial += 1
        identifier = self.serial
        self._send({"jsonrpc": "2.0", "id": identifier,
                    "method": method, "params": params})
        deadline = time.monotonic() + timeout
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(f"MCP response timeout: {method}; do not replay mutations")
            try:
                message = self.messages.get(timeout=remaining)
            except queue.Empty as error:
                raise TimeoutError(f"MCP response timeout: {method}") from error
            if isinstance(message, Exception):
                raise message
            if message.get("id") != identifier:
                continue
            if "error" in message:
                raise RuntimeError(message["error"])
            return message["result"]

    def tool(self, name, arguments, timeout=45):
        result = self.rpc("tools/call", {"name": name, "arguments": arguments}, timeout)
        if result.get("isError"):
            raise RuntimeError(result.get("content"))
        if "structuredContent" in result:
            return result["structuredContent"]
        return json.loads(next(c["text"] for c in result["content"] if c["type"] == "text"))

    def close(self):
        if self.process.poll() is None:
            self.process.stdin.close()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.terminate()
                try:
                    self.process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    self.process.kill()
                    self.process.wait(timeout=5)
        self.reader.join(timeout=1)

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()
