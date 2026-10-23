#!/usr/bin/env python3
"""Public PAB TCP forwarding acceptance; app-UID toybox targets, no adb networking."""
from __future__ import annotations
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
from pathlib import Path
import secrets
import socket
import time
import uuid
from android_home import Home, normal_status
from android_public import PublicMcp
from test_android_remote import ROOT, installed_apk_sha256


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--operator-run-id', type=uuid.UUID)
    parser.add_argument('--operator-config', type=Path)
    parser.add_argument('--output-root', type=Path, default=ROOT / '.build/android/port-forward')
    parser.add_argument('--device-code', required=True)
    parser.add_argument('--apk-sha256', required=True)
    parser.add_argument('--mcp-executable', required=True, type=Path)
    args = parser.parse_args()
    assert installed_apk_sha256(args.serial) == args.apk_sha256
    if not args.operator_config and not args.operator_run_id:
        parser.error('Provide --operator-config or --operator-run-id')
    operator_config = args.operator_config or ROOT / '.build/android/public' / str(args.operator_run_id) / 'mcp.toml'
    directory = args.output_root.resolve() / str(uuid.uuid4())
    directory.mkdir(parents=True)
    remote = '/workspace/pixels-forward-' + directory.name
    result = {'status': 'failed', 'serial': args.serial, 'apk_sha256': args.apk_sha256,
        'mcp_sha256': hashlib.sha256(args.mcp_executable.read_bytes()).hexdigest(),
        'qa_script_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        'forced_relay': True, 'checks': [], 'remote_root': remote, 'cleanup_pending': False,
        'full_android_acceptance': False, 'physical_usb_unplug_tested': False, 'eight_hour_soak_tested': False}
    home = Home(args.serial)
    before = normal_status(args.serial)
    client = None
    forwards, tasks, sockets = {}, set(), []
    generation = 0

    def call(name, fields=None, *, device=False, error=False):
        fields = dict(fields or {})
        if device: fields['device_code'] = args.device_code
        value, raw = client.tool(name, fields, allow_error=True)
        with (directory / 'trace.jsonl').open('a', encoding='utf-8') as log:
            log.write(json.dumps({'tool': name, 'fields': fields, 'value': value,
                'is_error': bool(raw.get('isError')), 'time': time.time()}, ensure_ascii=False) + '\n')
        if not error: assert not raw.get('isError'), (name, value)
        return value, raw

    def connect():
        nonlocal client, generation
        if client is None:
            generation += 1
            client = PublicMcp(operator_config, args.device_code, True,
                directory / f'mcp-{generation}.log', args.mcp_executable)
        value, raw = client.connect()
        assert value.get('connected') and not raw.get('isError'), value
        assert value['target']['execution']['os_family'] == 'android'

    def passed(name, **metadata):
        result['checks'].append({'name': name, 'passed': True, **metadata})
        print(json.dumps({'check': name, 'passed': True}), flush=True)

    def file(name, fields):
        value, _ = call(name, fields, device=True)
        deadline = time.monotonic() + 90
        while value.get('result', {}).get('state') == 'running':
            assert time.monotonic() < deadline
            value, _ = call('pab_get_operation', {'operation_id': value['operation_ref']['operation_id']}, device=True)
        assert value['result']['state'] == 'completed', value
        return value

    def wait_status(forward, predicate):
        deadline = time.monotonic() + 18
        while True:
            value, _ = call('pab_port_forward_status', {'forward_id': forward['forward_id']})
            if predicate(value): return value
            assert time.monotonic() < deadline, value
            time.sleep(.1)

    def open_forward(port, host='127.0.0.1', local_port=0):
        value, _ = call('pab_port_forward_open', {'remote_host': host, 'remote_port': port,
            'local_port': local_port}, device=True)
        assert value['state'] == 'listening' and value['local_host'] == '127.0.0.1', value
        forwards[value['forward_id']] = value
        return value

    def close_forward(forward):
        value, _ = call('pab_port_forward_close', {'forward_id': forward['forward_id']})
        assert value['state'] in ('closed', 'failed') and value['active_connections'] == 0, value
        forwards.pop(forward['forward_id'], None)
        return value

    def local(forward):
        sock = socket.create_connection(('127.0.0.1', forward['local_port']), timeout=15)
        sock.settimeout(18)
        return sock

    def receive_exact(sock, size):
        result = bytearray()
        while len(result) < size:
            block = sock.recv(size - len(result))
            assert block, 'Unexpected EOF'
            result.extend(block)
        return bytes(result)

    def closed(sock):
        try: assert sock.recv(1) == b'', 'Unexpected bytes after closed relay'
        except (ConnectionResetError, ConnectionAbortedError): pass
        finally: sock.close()

    def exchange(forward, payload):
        with local(forward) as sock, ThreadPoolExecutor(max_workers=1) as pool:
            def send():
                sock.sendall(payload); sock.shutdown(socket.SHUT_WR)
            sender = pool.submit(send)
            received = bytearray()
            while block := sock.recv(64 * 1024): received.extend(block)
            sender.result()
        assert bytes(received) == payload, (len(received), len(payload))
        return hashlib.sha256(received).hexdigest()

    def server(mode):
        port = 30000 + secrets.randbelow(28000)
        value, _ = call('pab_run_command', {'program': '/system/bin/toybox',
            'args': ['nc', '-s', '127.0.0.1', '-p', str(port), '-L', '/system/bin/sh', 'target.sh', mode],
            'cwd': remote, 'timeout_ms': 300000, 'wait_ms': 0, 'request_id': str(uuid.uuid4())}, device=True)
        task = value['task']['task_ref']['task_id']; tasks.add(task)
        deadline = time.monotonic() + 12
        while True:
            value, _ = call('pab_get_task', {'task_id': task, 'wait_ms': 100}, device=True, error=True)
            snapshot = value.get('snapshot') or value.get('task')
            if snapshot and snapshot['state'] == 'running': break
            assert snapshot and snapshot['state'] not in ('failed', 'cancelled', 'interrupted', 'succeeded'), value
            assert time.monotonic() < deadline, value
        time.sleep(.15)
        return port

    try:
        home.start(); connect()
        catalog = client.rpc('tools/list', {})
        (directory / 'catalog.json').write_text(json.dumps(catalog, indent=2, ensure_ascii=False), encoding='utf-8')
        assert {'pab_port_forward_open', 'pab_port_forward_status', 'pab_port_forward_close'} <= {t['name'] for t in catalog['tools']}
        passed('existing_mcp_port_forward_contract')
        file('pab_mkdir', {'path': remote, 'parents': False, 'exist_ok': False})
        result['cleanup_pending'] = True
        fixture = '''case "$1" in
echo) exec /system/bin/cat ;;
fin) f="fin-$$.bin"; trap 'rm -f "$f"' EXIT; cat > "$f"; cat "$f" ;;
http) IFS= read -r line; while IFS= read -r header; do [ "$header" = "$(printf '\\r')" ] && break; done
body='pixels-forward-中文'; size=$(printf %s "$body" | wc -c)
printf 'HTTP/1.1 200 OK\\r\\nContent-Length: %s\\r\\nConnection: close\\r\\n\\r\\n%s' "$size" "$body" ;;
*) exit 2 ;;
esac
'''
        file('pab_file_write', {'path': remote + '/target.sh', 'content': fixture})
        echo_port = server('echo'); fin_port = server('fin'); http_port = server('http')
        echo = open_forward(echo_port); same = open_forward(echo_port)
        assert same['forward_id'] == echo['forward_id'] and same['local_port'] == echo['local_port']
        passed('loopback_listener_repeated_open_returns_same_listener')
        payload = bytes(range(256)) * 8192
        digest = exchange(echo, payload)
        status = wait_status(echo, lambda v: v['active_connections'] == 0 and v['successful_connections'] == 1)
        assert status['bytes_to_remote'] == len(payload) and status['bytes_to_local'] == len(payload), status
        assert status['connection_path'] == 'relay', status
        passed('two_mib_binary_sha256_byte_counters_and_forced_relay', sha256=digest)
        fin = open_forward(fin_port, host='localhost'); exchange(fin, payload[:150000])
        passed('dns_localhost_and_response_after_request_fin')
        web = open_forward(http_port)
        with local(web) as sock:
            sock.sendall(b'GET /pixels-forward HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n')
            response = bytearray()
            while block := sock.recv(8192): response.extend(block)
        assert response.startswith(b'HTTP/1.1 200 OK\r\n') and response.endswith('pixels-forward-中文'.encode()), response
        passed('ordinary_http_tcp_service')
        for _ in range(4):
            sock = local(echo); sock.sendall(b'hold'); assert receive_exact(sock, 4) == b'hold'; sockets.append(sock)
        with local(echo) as excess: closed(excess)
        status = wait_status(echo, lambda v: v['failed_connections'] >= 1)
        assert 'resourcelimit' in status['last_error'].lower().replace('_', '').replace(' ', ''), status
        for sock in sockets: sock.sendall(b'alive'); assert receive_exact(sock, 5) == b'alive'
        for sock in sockets: sock.shutdown(socket.SHUT_WR); closed(sock)
        sockets.clear(); wait_status(echo, lambda v: v['active_connections'] == 0)
        with ThreadPoolExecutor(max_workers=4) as pool:
            hashes = list(pool.map(lambda n: exchange(echo, bytes([n]) * 100000), range(4)))
        passed('four_parallel_fifth_rejected_existing_streams_survive_slots_reused', sha256=hashes)
        refused = open_forward(9)
        with local(refused) as sock: closed(sock)
        status = wait_status(refused, lambda v: v['failed_connections'] == 1)
        assert 'connectionrefused' in status['last_error'].lower().replace('_', '').replace(' ', ''), status
        close_forward(refused)
        missing = open_forward(9, host='pixels-' + directory.name + '.invalid'); started = time.monotonic()
        with local(missing) as sock: closed(sock)
        status = wait_status(missing, lambda v: v['failed_connections'] == 1)
        assert any(w in status['last_error'].lower().replace('_', '') for w in ('dnsfailed', 'connecttimeout')), status
        assert time.monotonic() - started < 16
        close_forward(missing); exchange(echo, b'healthy-after-errors')
        passed('refused_dns_failure_bounded_connect_original_connection_survives')
        with socket.socket() as occupied:
            occupied.bind(('127.0.0.1', 0)); occupied.listen()
            value, raw = call('pab_port_forward_open', {'remote_port': echo_port, 'local_port': occupied.getsockname()[1]}, device=True, error=True)
            assert raw.get('isError') and 'bind_failed' in json.dumps(value), value
        for port, host in [(0, '127.0.0.1'), (80, 'https://bad')]:
            value, raw = call('pab_port_forward_open', {'remote_port': port, 'remote_host': host}, device=True, error=True)
            assert raw.get('isError'), value
        passed('occupied_local_port_invalid_targets_are_explicit_errors')
        held = local(echo); held.sendall(b'close'); assert receive_exact(held, 5) == b'close'; sockets.append(held)
        close_forward(echo); closed(held); sockets.clear()
        again, _ = call('pab_port_forward_close', {'forward_id': echo['forward_id']})
        assert again['state'] == 'closed' and again['active_connections'] == 0
        rebound = open_forward(echo_port, local_port=echo['local_port']); exchange(rebound, b'rebound'); close_forward(rebound)
        passed('close_cancels_sockets_is_idempotent_and_releases_local_port')
        stopped_live = open_forward(echo_port)
        held = local(stopped_live); held.sendall(b'stop'); assert receive_exact(held, 4) == b'stop'; sockets.append(held)
        home.stop(); closed(held); sockets.clear()
        wait_status(stopped_live, lambda v: v['active_connections'] == 0)
        try:
            denied = local(stopped_live)
        except ConnectionRefusedError:
            pass  # The listener may already have closed after the transport stopped.
        else:
            with denied: closed(denied)
        close_forward(stopped_live)
        passed('normal_device_disconnect_closes_active_sockets_and_denies_new_relays')
        home.start(); connect()
        fresh_port = server('echo'); fresh = open_forward(fresh_port)
        held = local(fresh); held.sendall(b'disconnect'); assert receive_exact(held, 10) == b'disconnect'; sockets.append(held)
        call('pab_disconnect', device=True); closed(held); sockets.clear()
        for forward in list(forwards.values()):
            wait_status(forward, lambda v: v['state'] in ('failed', 'closed') and v['active_connections'] == 0)
            close_forward(forward)
        # Keep the MCP process that owns our command fixtures. Transport
        # disconnect/reconnect does not transfer command cancellation ownership.
        connect()
        recovered = open_forward(fresh_port, local_port=fresh['local_port'])
        assert recovered['forward_id'] != fresh['forward_id']
        exchange(recovered, b'explicit-new-listener-only'); close_forward(recovered)
        passed('disconnect_closes_listeners_reconnect_requires_explicit_open')
        result['status'] = 'passed'
    finally:
        for sock in sockets: sock.close()
        if client is not None:
            for forward in list(forwards.values()):
                try: close_forward(forward)
                except Exception as e: result.setdefault('cleanup_errors', []).append(type(e).__name__)
            for task in tasks:
                try:
                    value, _ = call('pab_get_task', {'task_id': task, 'wait_ms': 1000}, device=True, error=True)
                    snapshot = value.get('snapshot') or value.get('task')
                    if snapshot and snapshot['state'] not in ('succeeded', 'failed', 'cancelled', 'interrupted'):
                        call('pab_cancel_operation', {'operation_id': snapshot['request_id']}, device=True)
                        deadline = time.monotonic() + 15
                        while True:
                            value, _ = call('pab_get_task', {'task_id': task, 'wait_ms': 1000}, device=True, error=True)
                            snapshot = value.get('snapshot') or value.get('task')
                            if snapshot and snapshot['state'] in ('succeeded', 'failed', 'cancelled', 'interrupted') and value.get('complete'): break
                            assert time.monotonic() < deadline, value
                except Exception as e: result.setdefault('cleanup_errors', []).append(type(e).__name__)
            if result['cleanup_pending']:
                try:
                    home.start(); file('pab_file_delete', {'path': remote, 'recursive': True})
                    result['cleanup_pending'] = False
                except Exception as e: result.setdefault('cleanup_errors', []).append(type(e).__name__)
            try: call('pab_disconnect', device=True)
            except Exception as e: result.setdefault('cleanup_errors', []).append(type(e).__name__)
            client.close()
        try:
            if before['connection_requested']: home.start()
            else: home.stop()
            state = normal_status(args.serial)
            assert all(state.get(k) == before.get(k) for k in ('paused', 'connection_requested'))
            result['final_state'] = {k: state.get(k) for k in ('phase', 'remote_ready', 'paused', 'accessibility_ready')}
        except Exception as e: result.setdefault('cleanup_errors', []).append(type(e).__name__)
        if result.get('cleanup_errors') or result['cleanup_pending']: result['status'] = 'failed'
        (directory / 'result.json').write_text(json.dumps(result, indent=2, ensure_ascii=False), encoding='utf-8')
        print(json.dumps({'directory': str(directory), 'status': result['status']}), flush=True)
    assert result['status'] == 'passed', result.get('cleanup_errors')


if __name__ == '__main__':
    main()
