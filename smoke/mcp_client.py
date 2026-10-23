"""Own one stdio MCP process for live smoke checks without shell interpolation.

Credentials are supplied through the application's normal configuration. This
module never copies credential databases or changes another MCP session.
"""
from __future__ import annotations

import json
import os
from pathlib import Path
import queue
import subprocess
import threading
import time


class McpToolError(RuntimeError):
    """Keep the original result for journaling without printing its contents."""

    def __init__(self, name, value):
        self.value = value
        super().__init__(f'{name} rejected: {value.get("error", {}).get("code", "see private evidence")}')


class McpClient:
    def __init__(self, executable: Path, log: Path, *, config: Path | None = None,
                 env: dict[str, str] | None = None):
        log.parent.mkdir(parents=True, exist_ok=True)
        self.log = log.open('w', encoding='utf-8')
        command = [str(executable)]
        if config is not None:
            command.extend(['--config', str(config)])
        self.process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=self.log, text=True, encoding='utf-8', bufsize=1, env=env or os.environ.copy())
        self.responses: queue.Queue[dict | None] = queue.Queue()
        self.sequence = 0
        self.lock = threading.Lock()
        self.closed = False

        def consume():
            try:
                for line in self.process.stdout:
                    self.responses.put(json.loads(line))
            finally:
                self.responses.put(None)

        self.reader = threading.Thread(target=consume, daemon=True)
        self.reader.start()
        try:
            self.initialized = self.rpc('initialize', {
                'protocolVersion': '2025-11-25', 'capabilities': {},
                'clientInfo': {'name': 'pab-release-smoke', 'version': '1'}})
            self._send({'jsonrpc': '2.0', 'method': 'notifications/initialized'})
        except BaseException:
            self.close()
            raise

    def _send(self, value):
        self.process.stdin.write(json.dumps(value, ensure_ascii=False) + '\n')
        self.process.stdin.flush()

    def rpc(self, method, params, timeout=45):
        with self.lock:
            self.sequence += 1
            identifier = self.sequence
            self._send({'jsonrpc': '2.0', 'id': identifier, 'method': method, 'params': params})
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                try:
                    value = self.responses.get(timeout=max(.01, deadline - time.monotonic()))
                except queue.Empty:
                    break
                if value is None:
                    raise RuntimeError('MCP exited before replying; inspect its private log')
                if value.get('id') == identifier:
                    if 'error' in value:
                        raise RuntimeError(f'MCP protocol error for {method}')
                    return value['result']
            raise TimeoutError(f'MCP {method} timed out; do not replay accepted mutations')

    def call(self, name, arguments, *, allow_error=False, timeout=45):
        raw = self.rpc('tools/call', {'name': name, 'arguments': arguments}, timeout)
        value = raw.get('structuredContent')
        if value is None:
            value = json.loads(next(block['text'] for block in raw['content'] if block['type'] == 'text'))
        if raw.get('isError') and not allow_error:
            raise McpToolError(name, value)
        return value

    def observe(self, accepted, *, timeout=60):
        """Poll only the original accepted operation; never retry its mutation."""
        value = accepted
        operation = value.get('operation_ref')
        if not operation:
            return value
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            result = value.get('result', {})
            snapshot = value.get('snapshot') or value.get('task') or {}
            # An application call can finish with partial/unknown effects. Its
            # immutable unconfirmed result must be inspected, never replayed or
            # polled forever as if the helper were still doing useful work.
            terminal = result.get('state') in ('completed', 'failed', 'cancelled', 'unconfirmed')
            # A command can exit before its output streams finish draining.
            # An explicit incomplete result must keep observing that same task.
            terminal |= (snapshot.get('state') in ('succeeded', 'failed', 'cancelled')
                         and value.get('complete') is not False)
            if terminal or value.get('complete') is True or value.get('error'):
                return value
            query = {
                'device_code': operation['device_code'], 'operation_id': operation['operation_id'],
                'wait_ms': 1000, 'wait_until': 'complete'}
            if operation.get('kind', '').startswith('document_'):
                query['kind'] = operation['kind']
            value = self.call('pab_get_operation', query, allow_error=True)
        raise TimeoutError(f'Operation {operation["operation_id"]} remains unconfirmed')

    def read_stdout(self, value, *, timeout=15, max_bytes=65536):
        """Read the original task through EOF; previews and empty chunks are not EOF."""
        operation = value.get('operation_ref', {})
        if not operation.get('task_id') or not operation.get('device_code'):
            raise ValueError('A resolved command task is required')
        if type(max_bytes) is not int or not 1 <= max_bytes <= 16 * 1024 * 1024 or timeout <= 0:
            raise ValueError('Invalid bounded output read')
        deadline = time.monotonic() + timeout
        offset, chunks = 0, []
        while time.monotonic() < deadline:
            remaining = deadline - time.monotonic()
            chunk = self.call('pab_read_output', {
                'device_code': operation['device_code'], 'task_id': operation['task_id'],
                'stream': 'stdout', 'offset': offset, 'max_bytes': min(65536, max_bytes - offset + 1),
                'wait_ms': max(1, min(1000, int(remaining * 1000)))}, timeout=min(45, remaining + 2))
            next_offset = chunk['next_offset']
            if (chunk.get('gap') or chunk['offset'] != offset or type(next_offset) is not int
                    or next_offset < offset or chunk.get('decoding_replacements', 0)):
                raise ValueError('Original task output is incomplete or invalid')
            if next_offset > max_bytes:
                raise ValueError('Original task output exceeds the configured bound')
            chunks.append(chunk['text'])
            offset = next_offset
            if chunk['range']['complete'] and offset >= chunk['range']['available_to']:
                return ''.join(chunks)
        raise TimeoutError('Original task output did not reach EOF')

    def close(self):
        if self.closed:
            return
        self.closed = True
        if self.process.poll() is None:
            self.process.stdin.close()
            try:
                self.process.wait(timeout=8)
            except subprocess.TimeoutExpired:
                self.process.terminate()
                try:
                    self.process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    self.process.kill()
                    self.process.wait(timeout=5)
        self.reader.join(timeout=2)
        self.process.stdout.close()
        self.log.close()

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()
