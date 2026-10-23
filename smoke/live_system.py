"""Verify system queries, native log byte hashes and an owned process lifecycle."""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import time
import tomllib

from mcp_client import McpClient, McpToolError


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ('lab', 'config', 'mcp', 'output'):
        parser.add_argument('--' + key, type=Path, required=True)
    args = parser.parse_args()
    devices = tomllib.loads(args.lab.read_text(encoding='utf-8-sig'))['device']
    args.output.mkdir(parents=True, exist_ok=False)
    report = {'status': 'running', 'calls': [], 'checks': [], 'cleanup': []}

    def save():
        (args.output / 'result.json').write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')

    env = {k: v for k, v in os.environ.items() if not k.startswith('PAB_')}
    with McpClient(args.mcp.resolve(), args.output / 'mcp.log', config=args.config.resolve(), env=env) as c:
        def call(tool, values, *, allow_error=False, observe=True):
            entry = {'tool': tool, 'arguments': values, 'value': None}
            report['calls'].append(entry)
            save()
            try:
                value = c.call(tool, values, allow_error=allow_error)
                entry['value'] = value
                save()
                if observe:
                    value = c.observe(value)
            except McpToolError as error:
                entry['value'] = error.value
                save()
                raise
            entry['value'] = value
            save()
            if not allow_error:
                assert not value.get('error'), tool
                assert value.get('result', {}).get('state') not in ('failed', 'cancelled', 'unconfirmed'), tool
            return value

        def check(code, name):
            report['checks'].append({'device': code, 'name': name, 'passed': True})
            save()
            print(code + ' ' + name + ': passed', flush=True)

        try:
            for device in devices:
                code, family = device['code'], device['os']
                assert family in ('windows', 'macos')
                task = None
                try:
                    value = call('pab_connect', {'device_code': code, 'wait_ms': 1000})
                    deadline = time.monotonic() + 50
                    while value.get('state') == 'connecting' and time.monotonic() < deadline:
                        value = call('pab_connect', {'device_code': code, 'wait_ms': 1000})
                    assert value.get('connected')
                    sessions = call('pab_list_sessions', {'device_code': code, 'limit': 20})
                    assert sessions['result']['data']['type'] == 'sessions'
                    dns = call('pab_resolve_dns', {'device_code': code, 'name': 'localhost', 'record_type': 'A'})
                    assert any(r['value'] == '127.0.0.1' for r in dns['result']['data']['result']['records'])
                    check(code, 'native_login_sessions_and_target_dns')

                    selected = device.get('log_source', 'eventlog:System' if family == 'windows' else 'unified:local')
                    inventory = call('pab_system_log_sources', {'device_code': code, 'limit': 128})['result']['data']['snapshot']
                    sources = inventory['sources']
                    for _ in range(63):
                        if any(s['source_id'] == selected for s in sources) or not inventory.get('next_source'):
                            break
                        inventory = call('pab_system_log_sources', {'device_code': code, 'limit': 128,
                            'after': inventory['next_source']})['result']['data']['snapshot']
                        sources.extend(inventory['sources'])
                    assert any(s['source_id'] == selected and s['availability'] == 'readable' for s in sources)
                    now = int(time.time() * 1000)
                    page = call('pab_system_log_read', {'device_code': code, 'sources': [selected],
                        'start_unix_ms': now - (3600 if family == 'windows' else 60) * 1000, 'end_unix_ms': now, 'limit': 4})['result']['data']['snapshot']
                    # Scanning pages can legitimately be empty; poll only their cursor with a bound.
                    for _ in range(12):
                        if page['events'] or not page.get('has_more'):
                            break
                        time.sleep(.2)
                        page = call('pab_system_log_read', {'device_code': code, 'cursor': page['cursor'], 'limit': 4})['result']['data']['snapshot']
                    assert page['events'], 'No event available to test raw reconstruction'
                    event = page['events'][0]
                    assert 0 < event['total_bytes'] <= 1024 * 1024, 'event byte budget'
                    data = bytearray()
                    while len(data) < event['total_bytes']:
                        chunk = call('pab_system_log_event_read', {'device_code': code, 'event_ref': event['event_ref'],
                            'offset': len(data), 'max_bytes': 4096})['result']['data']['snapshot']
                        assert chunk['sha256'] == event['sha256'] and chunk['offset'] == len(data)
                        raw = base64.b64decode(chunk['data_base64'], validate=True)
                        assert raw and chunk['next_offset'] == len(data) + len(raw)
                        data.extend(raw)
                    assert len(data) == event['total_bytes'] and hashlib.sha256(data).hexdigest() == event['sha256']
                    check(code, 'native_log_sources_read_and_exact_raw_event_hash')

                    if family == 'windows':
                        script = "$ErrorActionPreference='Stop';$l=[Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback,0);$l.Start();[pscustomobject]@{pid=$PID;port=$l.LocalEndpoint.Port}|ConvertTo-Json -Compress;Start-Sleep -Seconds 120;$l.Stop()"
                        command = {'program': r'C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe',
                            'args': ['-NoProfile', '-NonInteractive', '-EncodedCommand', base64.b64encode(script.encode('utf-16-le')).decode()]}
                    else:
                        command = {'program': '/usr/bin/python3', 'args': ['-u', '-c',
                            "import os,socket,time,json;s=socket.socket();s.bind(('127.0.0.1',0));s.listen();print(json.dumps({'pid':os.getpid(),'port':s.getsockname()[1]}),flush=True);time.sleep(120)"]}
                    accepted = call('pab_run_command', {'device_code': code, **command,
                        'timeout_ms': 150000, 'wait_ms': 1000}, observe=False)
                    task = accepted['operation_ref']
                    stdout = accepted.get('output', {}).get('stdout', {}).get('text', '')
                    deadline = time.monotonic() + 15
                    while not stdout.strip() and time.monotonic() < deadline:
                        stdout = call('pab_read_output', {'device_code': code, 'task_id': task['task_id'],
                            'stream': 'stdout', 'wait_ms': 1000})['text']
                    owned = json.loads(stdout)
                    pid = owned['pid']
                    process = call('pab_get_process', {'device_code': code, 'pid': pid})['result']['data']['process']
                    identity = process['termination_identity']
                    assert identity, 'native termination identity'
                    listed = call('pab_list_processes', {'device_code': code, 'pid': pid})['result']['data']['entries']
                    assert len(listed) == 1 and listed[0]['pid'] == pid
                    connections = call('pab_list_network_connections', {'device_code': code, 'pid': pid,
                        'protocol': 'tcp', 'state': 'listen', 'local_port': owned['port']})['result']['data']['entries']
                    assert any(pid in r['pids'] and r['local_port'] == owned['port'] for r in connections), 'owned loopback listener'
                    check(code, 'owned_process_inventory_and_exact_listener_filter')
                    wrong = call('pab_terminate_process', {'device_code': code, 'pid': pid, 'identity': identity + '-wrong',
                        'force': True, 'timeout_ms': 1000}, allow_error=True)
                    assert 'process_identity_mismatch' in json.dumps(wrong), 'wrong identity was accepted'
                    assert call('pab_get_process', {'device_code': code, 'pid': pid})['result']['data']['process']['termination_identity'] == identity
                    stopped = call('pab_terminate_process', {'device_code': code, 'pid': pid, 'identity': identity,
                        'force': family == 'windows', 'timeout_ms': 1000})['result']['data']['result']
                    assert stopped['outcome'] == 'completed'
                    terminal = c.observe(accepted)
                    assert (terminal.get('snapshot') or terminal.get('task'))['state'] in ('failed', 'cancelled', 'succeeded')
                    report['cleanup'].append({'device': code, 'task_id': task['task_id'], 'process_exit_confirmed': True})
                    task = None
                    check(code, 'wrong_native_identity_rejected_then_owned_process_exit_confirmed')
                finally:
                    if task:
                        value = call('pab_cancel_operation', {'device_code': code, 'operation_id': task['operation_id']}, allow_error=True)
                        report['cleanup'].append({'device': code, 'task_id': task['task_id'], 'cancel_result': value})
                    call('pab_disconnect', {'device_code': code})
                    save()
            report['status'] = 'passed'
        except BaseException as error:
            report['status'] = 'failed'
            report['error'] = repr(error)
            raise
        finally:
            save()


if __name__ == '__main__':
    main()
