"""Exercise a fresh MCP binary against an installed remote Executor.

The target must run fixtures/port-forward-server.mjs on 127.0.0.1:38673/38674.
No local or remote production service is modified by this script.
"""

import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import os
from pathlib import Path
import queue
import socket
import subprocess
import threading
import time


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--mcp', required=True, type=Path)
parser.add_argument('--device-code', required=True)
parser.add_argument('--echo-port', type=int, default=38673)
parser.add_argument('--http-port', type=int, default=38674)
parser.add_argument('--concurrency', type=int, default=64)
parser.add_argument('--os', choices=('windows', 'macos', 'linux'), default='windows')
parser.add_argument('--relay-only', action='store_true', help='Use a debug MCP with direct QUIC paths disabled')
parser.add_argument('--expect-unsupported', action='store_true', help='Verify an older Executor rejects open')
parser.add_argument('--dns-only', action='store_true', help='Verify remote DNS failure without an echo fixture')
args = parser.parse_args()

environment = os.environ.copy()
if args.relay_only:
    environment['PAB_TEST_RELAY_ONLY'] = '1'
settings = Path(os.environ.get('PAB_INSTALLED_SETTINGS', r'C:\Program Files\PixelsAgentBridge\settings.json'))
if settings.is_file():
    config = json.loads(settings.read_text(encoding='utf-8-sig'))
    environment.setdefault('PAB_CONTROL_URL', config['control_url'])
    environment.setdefault('PAB_RELAY_URLS', config['relay_urls'])

process = subprocess.Popen([str(args.mcp.resolve())], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                           stderr=subprocess.PIPE, env=environment,
                           creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0))
responses = queue.Queue()
errors = []


def stdout_reader():
    for line in process.stdout:
        try:
            responses.put(json.loads(line))
        except ValueError:
            responses.put({'invalid_line': line[:200].decode(errors='replace')})


def stderr_reader():
    for line in process.stderr:
        if len(errors) < 30:
            errors.append(line.decode(errors='replace')[:300])


threading.Thread(target=stdout_reader, daemon=True).start()
threading.Thread(target=stderr_reader, daemon=True).start()
serial = 0


def rpc(method, params=None, timeout=35):
    global serial
    serial += 1
    message = {'jsonrpc': '2.0', 'id': serial, 'method': method}
    if params is not None:
        message['params'] = params
    process.stdin.write((json.dumps(message) + '\n').encode())
    process.stdin.flush()
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            response = responses.get(timeout=min(1, max(.01, deadline - time.monotonic())))
        except queue.Empty:
            if process.poll() is not None:
                raise RuntimeError(f'MCP exited {process.returncode}: {errors[-3:]}')
            continue
        if response.get('id') == serial:
            if 'error' in response:
                raise RuntimeError(response['error'])
            return response['result']
    raise TimeoutError(f'{method} timed out: {errors[-3:]}')


def tool(name, arguments, expect_error=False):
    response = rpc('tools/call', {'name': name, 'arguments': arguments})
    if response.get('isError'):
        if expect_error:
            return response.get('content')
        raise RuntimeError(f'{name}: {response.get("content")}')
    if expect_error:
        raise AssertionError(f'{name} unexpectedly succeeded: {response}')
    return response.get('structuredContent') or json.loads(response['content'][0]['text'])


def exchange(port, payload):
    with socket.create_connection(('127.0.0.1', port), timeout=20) as client:
        client.settimeout(30)
        client.sendall(payload)
        client.shutdown(socket.SHUT_WR)
        response = bytearray()
        while True:
            chunk = client.recv(16384)
            if not chunk:
                break
            response.extend(chunk)
    assert response == payload, f'binary mismatch {len(response)} != {len(payload)}'
    return len(response)


def exchange_duplex(port, payload):
    with socket.create_connection(('127.0.0.1', port), timeout=20) as client:
        client.settimeout(60)
        failures = []

        def send():
            try:
                client.sendall(payload)
                client.shutdown(socket.SHUT_WR)
            except Exception as error:
                failures.append(error)

        sender = threading.Thread(target=send)
        sender.start()
        response = bytearray()
        while chunk := client.recv(4096):
            response.extend(chunk)
            time.sleep(.001)
        sender.join(timeout=10)
        assert not sender.is_alive() and not failures, failures
    assert response == payload, f'duplex binary mismatch {len(response)} != {len(payload)}'
    return hashlib.sha256(response).hexdigest()


def run():
    rpc('initialize', {'protocolVersion': '2025-03-26', 'capabilities': {},
                       'clientInfo': {'name': 'port-forward-acceptance', 'version': '1'}})
    process.stdin.write(b'{"jsonrpc":"2.0","method":"notifications/initialized"}\n')
    process.stdin.flush()
    tool('pab_connect', {'device_code': args.device_code})
    opening = {'device_code': args.device_code, 'remote_port': args.echo_port}
    if args.expect_unsupported:
        error = tool('pab_port_forward_open', opening, expect_error=True)
        assert 'unsupported' in str(error).lower(), error
        print(json.dumps({'stage': 'old_executor', 'device_code': args.device_code,
                          'unsupported': True}), flush=True)
        return
    if args.dns_only:
        missing_dns = tool('pab_port_forward_open', {'device_code': args.device_code,
                           'remote_host': 'pixels-acceptance.invalid', 'remote_port': 80})
        try:
            with socket.create_connection(('127.0.0.1', missing_dns['local_port']), timeout=15) as client:
                client.settimeout(20)
                assert client.recv(1) == b''
        except ConnectionResetError:
            pass
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            dns_status = tool('pab_port_forward_status', {'forward_id': missing_dns['forward_id']})
            if dns_status['failed_connections']:
                break
            time.sleep(.1)
        assert dns_status['failed_connections'] == 1, dns_status
        assert 'DnsFailed' in dns_status['last_error'], dns_status
        assert dns_status['state'] == 'listening', dns_status
        closed_dns = tool('pab_port_forward_close', {'forward_id': missing_dns['forward_id']})
        assert closed_dns['state'] == 'closed'
        print(json.dumps({'stage': 'remote_dns_failure', 'device_code': args.device_code,
                          'status': dns_status}), flush=True)
        return
    forward = tool('pab_port_forward_open', opening)
    forward_id = forward['forward_id']
    assert forward['state'] == 'listening' and forward['local_host'] == '127.0.0.1'
    assert tool('pab_port_forward_open', opening)['forward_id'] == forward_id
    port = forward['local_port']
    print(json.dumps({'stage': 'open', 'forward': forward}), flush=True)
    isolated_request = b''.join((json.dumps(message) + '\n').encode() for message in (
        {'jsonrpc': '2.0', 'id': 1, 'method': 'initialize', 'params': {
            'protocolVersion': '2025-03-26', 'capabilities': {},
            'clientInfo': {'name': 'isolated-session', 'version': '1'}}},
        {'jsonrpc': '2.0', 'method': 'notifications/initialized'},
        {'jsonrpc': '2.0', 'id': 2, 'method': 'tools/call', 'params': {
            'name': 'pab_port_forward_status', 'arguments': {'forward_id': forward_id}}},
    ))
    other = subprocess.run([str(args.mcp.resolve())], input=isolated_request,
                           capture_output=True, env=environment, timeout=20,
                           creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0))
    isolated = [json.loads(line) for line in other.stdout.splitlines() if line.startswith(b'{')]
    answer = next(response for response in isolated if response.get('id') == 2)
    assert answer['result']['isError'], answer
    assert 'not_found' in str(answer['result']['content']), answer
    print(json.dumps({'stage': 'session_isolation', 'isolated': True}), flush=True)

    payload = os.urandom(200_003)
    exchange(port, payload)
    with ThreadPoolExecutor(max_workers=args.concurrency) as pool:
        futures = [pool.submit(exchange, port, bytes([i % 251]) * (1024 + i))
                   for i in range(args.concurrency)]
        assert sum(f.result() for f in futures) == sum(1024 + i for i in range(args.concurrency))
    status = tool('pab_port_forward_status', {'forward_id': forward_id})
    assert status['successful_connections'] == args.concurrency + 1, status
    assert status['failed_connections'] == 0, status
    expected_bytes = len(payload) + sum(1024 + i for i in range(args.concurrency))
    assert status['bytes_to_remote'] == expected_bytes, status
    assert status['bytes_to_local'] == expected_bytes, status
    print(json.dumps({'stage': 'echo', 'sha256': hashlib.sha256(payload).hexdigest(),
                      'status': status}), flush=True)
    large_payload = os.urandom(2_000_003)
    large_hash = exchange_duplex(port, large_payload)
    large_status = tool('pab_port_forward_status', {'forward_id': forward_id})
    assert large_status['bytes_to_remote'] == expected_bytes + len(large_payload), large_status
    assert large_status['bytes_to_local'] == expected_bytes + len(large_payload), large_status
    print(json.dumps({'stage': 'backpressure', 'sha256': large_hash,
                      'bytes': len(large_payload)}), flush=True)

    alias = tool('pab_port_forward_open', {'device_code': args.device_code,
                 'remote_host': 'localhost', 'remote_port': args.echo_port})
    exchange(alias['local_port'], b'hostname-resolution-over-remote-executor\0')
    tool('pab_port_forward_close', {'forward_id': alias['forward_id']})
    print(json.dumps({'stage': 'remote_dns', 'hostname': 'localhost'}), flush=True)

    http_forward = tool('pab_port_forward_open', {'device_code': args.device_code,
                               'remote_port': args.http_port})
    with socket.create_connection(('127.0.0.1', http_forward['local_port']), timeout=15) as client:
        client.settimeout(15)
        client.sendall(b'GET /acceptance HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n')
        response = bytearray()
        while chunk := client.recv(4096):
            response.extend(chunk)
    assert b'pixels-port-forward:/acceptance' in response, response[-300:]
    curl = subprocess.run(['curl', '--fail', '--silent', '--show-error',
                           f'http://127.0.0.1:{http_forward["local_port"]}/curl'],
                          capture_output=True, timeout=20)
    assert curl.returncode == 0 and curl.stdout == b'pixels-port-forward:/curl', curl.stderr
    print(json.dumps({'stage': 'http', 'path': http_forward['connection_path'],
                      'response_sha256': hashlib.sha256(response).hexdigest(),
                      'curl': True}), flush=True)

    missing = tool('pab_port_forward_open', {'device_code': args.device_code,
                   'remote_port': args.http_port + 1})
    try:
        with socket.create_connection(('127.0.0.1', missing['local_port']), timeout=10) as client:
            client.settimeout(10)
            assert client.recv(1) == b''
    except ConnectionResetError:
        pass
    time.sleep(.2)
    failed = tool('pab_port_forward_status', {'forward_id': missing['forward_id']})
    assert failed['failed_connections'] == 1, failed
    assert failed['state'] == 'listening', failed
    print(json.dumps({'stage': 'refused', 'status': failed}), flush=True)

    held = [socket.create_connection(('127.0.0.1', port), timeout=15) for _ in range(64)]
    try:
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            saturated = tool('pab_port_forward_status', {'forward_id': forward_id})
            if saturated['active_connections'] == 64:
                break
            time.sleep(.1)
        assert saturated['active_connections'] == 64, saturated
        with socket.create_connection(('127.0.0.1', port), timeout=10) as extra:
            extra.settimeout(10)
            try:
                assert extra.recv(1) == b''
            except ConnectionResetError:
                pass
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            saturated = tool('pab_port_forward_status', {'forward_id': forward_id})
            if saturated['failed_connections'] >= 1:
                break
            time.sleep(.1)
        assert saturated['active_connections'] == 64, saturated
        assert saturated['failed_connections'] == 1, saturated
        # Existing streams and unrelated tools must remain responsive at the cap.
        command = ('powershell.exe', ['-NoProfile', '-Command', 'Write-Output port-forward-alive']) \
            if args.os == 'windows' else ('/bin/echo', ['port-forward-alive'])
        tool('pab_run_command', {'device_code': args.device_code, 'program': command[0],
             'args': command[1], 'wait_ms': 5000})
        print(json.dumps({'stage': 'connection_limit', 'active': saturated['active_connections'],
                          'failed': saturated['failed_connections']}), flush=True)
    finally:
        for client in held:
            client.close()
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        released = tool('pab_port_forward_status', {'forward_id': forward_id})
        if released['active_connections'] == 0:
            break
        time.sleep(.1)
    assert released['active_connections'] == 0, released

    with socket.socket() as blocker:
        blocker.bind(('127.0.0.1', 0))
        blocker.listen()
        tool('pab_port_forward_open', {'device_code': args.device_code,
             'remote_port': args.echo_port, 'local_port': blocker.getsockname()[1]}, expect_error=True)
    tool('pab_port_forward_open', {'device_code': args.device_code,
         'remote_port': 65536}, expect_error=True)
    baseline = [tool('pab_port_forward_status', {'forward_id': item['forward_id']})
                for item in (forward, http_forward, missing)]
    print(json.dumps({'stage': 'before_listener_limit',
                      'states': [item['state'] for item in baseline]}), flush=True)
    assert all(item['state'] == 'listening' for item in baseline), baseline
    extra_forwards = []
    for index in range(5):
        extra_forwards.append(tool('pab_port_forward_open', {'device_code': args.device_code,
                              'remote_port': 39000 + index}))
    tool('pab_port_forward_open', {'device_code': args.device_code,
         'remote_port': 39005}, expect_error=True)
    print(json.dumps({'stage': 'listener_limit', 'active_listeners': 8}), flush=True)

    for item in [forward, http_forward, missing, *extra_forwards]:
        closed = tool('pab_port_forward_close', {'forward_id': item['forward_id']})
        assert closed['state'] == 'closed', closed
        assert tool('pab_port_forward_close', {'forward_id': item['forward_id']})['state'] == 'closed'
        with socket.socket() as check:
            check.bind(('127.0.0.1', item['local_port']))
    print(json.dumps({'stage': 'closed', 'device_code': args.device_code}), flush=True)

    dropped = tool('pab_port_forward_open', opening)
    tool('pab_disconnect', {'device_code': args.device_code})
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        disconnected = tool('pab_port_forward_status', {'forward_id': dropped['forward_id']})
        if disconnected['state'] == 'failed':
            break
        time.sleep(.1)
    assert disconnected['state'] == 'failed', disconnected
    with socket.socket() as check:
        check.bind(('127.0.0.1', dropped['local_port']))
    tool('pab_connect', {'device_code': args.device_code})
    assert tool('pab_port_forward_status', {'forward_id': dropped['forward_id']})['state'] == 'failed'
    print(json.dumps({'stage': 'disconnect', 'status': disconnected}), flush=True)


try:
    run()
finally:
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
