"""Run bounded native MCP checks on explicitly configured test devices.

Uses saved device pairing through the normal MCP configuration. Creates only
unique temporary paths and owned terminals. It never installs applications.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path, PurePosixPath, PureWindowsPath
import time
import tomllib
import uuid

if __package__:
    from .mcp_client import McpClient, McpToolError
else:
    from mcp_client import McpClient, McpToolError


def temporary_parent(family):
    # macOS /tmp is a symlink; file operations intentionally reject link parents.
    if family == 'windows':
        return PureWindowsPath(r'C:\Windows\Temp')
    if family in ('macos', 'linux'):
        return PurePosixPath('/private/tmp' if family == 'macos' else '/tmp')
    raise ValueError('Unsupported native test OS: ' + family)


def owned_temporary_path(family, root):
    parent = temporary_parent(family)
    target = type(parent)(root)
    if target.parent != parent or not target.name.startswith('pab-smoke-'):
        raise ValueError('Refusing cleanup outside the owned temporary directory')
    uuid.UUID(target.name.removeprefix('pab-smoke-'))
    return target


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--lab', type=Path, required=True)
    parser.add_argument('--mcp', type=Path, required=True)
    parser.add_argument('--config', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--mode', choices=['auto', 'relay'], default='relay')
    options = parser.parse_args()
    lab = tomllib.loads(options.lab.read_text(encoding='utf-8-sig'))
    run = options.output.resolve()
    run.mkdir(parents=True, exist_ok=False)
    report = {'status': 'running', 'run_id': str(uuid.uuid4()), 'checks': [], 'calls': [],
              'cleanup': [], 'mode': options.mode, 'mcp_sha256': hashlib.sha256(options.mcp.read_bytes()).hexdigest()}
    env = {key: value for key, value in os.environ.items() if not key.startswith('PAB_')}

    def save():
        (run / 'result.json').write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')

    with McpClient(options.mcp.resolve(), run / 'mcp.log', config=options.config, env=env) as client:
        tools = client.rpc('tools/list', {})['tools']
        (run / 'tools.json').write_text(json.dumps(tools, indent=2), encoding='utf-8')

        def call(name, args, *, observe=True, allow_error=False):
            entry = {'tool': name, 'arguments': args, 'value': None}
            report['calls'].append(entry)
            save()
            try:
                value = client.call(name, args, allow_error=allow_error)
                entry['value'] = value
                save()
                if observe:
                    value = client.observe(value)
            except McpToolError as error:
                entry['value'] = error.value
                save()
                raise
            entry['value'] = value
            save()
            if not allow_error and (value.get('error') or value.get('result', {}).get('state') in ('failed', 'cancelled', 'unconfirmed')):
                raise AssertionError(f'{name} did not complete; see result.json')
            return value

        def passed(device, name):
            report['checks'].append({'device': device, 'name': name, 'passed': True})
            save()
            print(json.dumps({'device': device, 'check': name, 'passed': True}), flush=True)

        def command(code, os_family, script, *, execution=None):
            if os_family == 'windows':
                args = {'program': r'C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe',
                        'args': ['-NoLogo', '-NoProfile', '-NonInteractive', '-EncodedCommand',
                                 base64.b64encode(script.encode('utf-16-le')).decode()]}
            else:
                args = {'program': '/usr/bin/python3', 'args': ['-c', script]}
            if execution:
                args['execution'] = execution
            return call('pab_run_command', {'device_code': code, **args, 'wait_ms': 1000, 'timeout_ms': 15000})

        def output(value):
            if value.get('output', {}).get('stdout', {}).get('text') is not None:
                return value['output']['stdout']['text']
            task_id = value['operation_ref']['task_id']
            return call('pab_read_output', {'device_code': value['operation_ref']['device_code'],
                'task_id': task_id, 'stream': 'stdout', 'max_bytes': 65536})['text']

        try:
            catalog = json.loads((Path(__file__).parent / 'tool-coverage.json').read_text())['tools']
            assert {tool['name'] for tool in tools} == set(catalog), 'MCP/catalog mismatch'
            passed('all', 'complete_tool_catalog')
            for device in lab['device']:
                code, family = device['code'], device['os']
                root = str(temporary_parent(family) / ('pab-smoke-' + str(uuid.uuid4())))
                terminal = None
                created = False
                cleanup_ok = False
                try:
                    deadline = time.monotonic() + 50
                    connected = call('pab_connect', {'device_code': code, 'connection_mode': options.mode,
                                                   'wait_ms': 1000}, observe=False)
                    while not connected.get('connected') and connected.get('state') == 'connecting' and time.monotonic() < deadline:
                        connected = call('pab_connect', {'device_code': code, 'wait_ms': 1000}, observe=False)
                    assert connected.get('connected'), 'Device did not connect'
                    if options.mode == 'relay':
                        assert connected.get('connection_mode') == 'relay', 'Relay mode not retained'
                        assert connected.get('connection_path') == 'relay', 'Actual path is not Relay'
                    passed(code, 'verified_connection_mode_and_actual_path')

                    info = call('pab_system_info', {'device_code': code, 'sample_cpu': False})
                    assert info['result']['data']['info']['memory_total_bytes'] > 0
                    for name in ['pab_list_disks', 'pab_list_network_interfaces', 'pab_list_execution_contexts']:
                        value = call(name, {'device_code': code})
                        assert value['result']['returned_count'] > 0
                    contexts = value['result']['data']['entries']
                    passed(code, 'native_system_disk_network_and_execution_identity')

                    marker = 'smoke-中文-🙂-' + report['run_id']
                    script = "[Console]::OutputEncoding=[Text.UTF8Encoding]::new();Write-Output '" + marker + "'" if family == 'windows' else 'print(' + repr(marker) + ')'
                    assert marker in output(command(code, family, script))
                    user = next((entry for entry in contexts if entry['mode'] == 'user' and entry.get('selection')), None)
                    assert user, 'No authorized native user context'
                    assert marker in output(command(code, family, script, execution=user['selection']))
                    passed(code, 'service_and_user_unicode_command')

                    if family == 'windows':
                        script = "$ErrorActionPreference='Stop';$p='" + root + "';if(Test-Path -LiteralPath $p){throw 'already exists'};[IO.Directory]::CreateDirectory($p)|Out-Null;Write-Output 'created'"
                    else:
                        script = 'import os;os.mkdir(' + repr(root) + ");print('created')"
                    assert 'created' in output(command(code, family, script))
                    created = True
                    file = root + '/中文-🙂.txt'
                    content = 'Pixels smoke 中文 🙂\nsecond line\n'
                    digest = hashlib.sha256(content.encode()).hexdigest()
                    call('pab_file_write', {'device_code': code, 'path': file, 'content': content})
                    hashed = call('pab_file_hash', {'device_code': code, 'path': file})
                    assert digest in json.dumps(hashed), 'Written content hash differs'
                    read = call('pab_file_read', {'device_code': code, 'path': file, 'expected_hash': digest})
                    assert 'Pixels smoke' in json.dumps(read, ensure_ascii=False)
                    passed(code, 'unicode_file_roundtrip_and_sha256')

                    stale = call('pab_file_write', {'device_code': code, 'path': file, 'content': 'must not be written',
                        'overwrite': True, 'expected_hash': '0' * 64}, allow_error=True)
                    assert stale.get('result', {}).get('state') == 'failed' or stale.get('error'), 'Stale write was accepted'
                    assert digest in json.dumps(call('pab_file_hash', {'device_code': code, 'path': file}))
                    passed(code, 'optimistic_write_rejects_stale_hash_without_mutation')

                    stat = call('pab_file_stat', {'device_code': code, 'path': file})
                    assert stat['result']['metadata']['size'] == len(content.encode('utf-8'))
                    patch = {'device_code': code, 'path': file, 'expected_hash': digest,
                        'edits': [{'find': 'second line', 'replace': 'changed line', 'expected_matches': 1}],
                        'request_id': str(uuid.uuid4())}
                    first = call('pab_file_patch', patch)
                    second = call('pab_file_patch', patch)
                    assert first['operation_ref']['operation_id'] == second['operation_ref']['operation_id']
                    content = content.replace('second line', 'changed line')
                    digest = hashlib.sha256(content.encode('utf-8')).hexdigest()
                    assert digest in json.dumps(call('pab_file_hash', {'device_code': code, 'path': file}))
                    passed(code, 'native_file_stat_and_idempotent_exact_patch')
                    for expected, matches in [('0' * 64, 1), (digest, 2)]:
                        failed = call('pab_file_patch', {'device_code': code, 'path': file,
                            'expected_hash': expected, 'edits': [{'find': 'changed line', 'replace': 'must not be published',
                            'expected_matches': matches}]}, allow_error=True)
                        assert failed.get('error') or failed.get('result', {}).get('state') == 'failed'
                        assert digest in json.dumps(call('pab_file_hash', {'device_code': code, 'path': file}))
                    passed(code, 'patch_stale_hash_and_match_count_conflicts_preserve_bytes')

                    copy = root + '/copy.txt'
                    moved = root + '/moved.txt'
                    call('pab_file_copy', {'device_code': code, 'path': file, 'destination': copy})
                    call('pab_file_move', {'device_code': code, 'path': copy, 'destination': moved})
                    assert digest in json.dumps(call('pab_file_hash', {'device_code': code, 'path': moved}))
                    search = call('pab_file_search', {'device_code': code, 'path': root, 'mode': 'content', 'query': 'Pixels smoke'})
                    assert 'moved.txt' in json.dumps(search, ensure_ascii=False)
                    zip_path = root + '/sample.zip'
                    call('pab_archive_create', {'device_code': code, 'path': zip_path, 'sources': [file, moved]})
                    call('pab_archive_extract', {'device_code': code, 'path': zip_path, 'destination': root + '/extracted'})
                    listing = call('pab_list_directory', {'device_code': code, 'path': root + '/extracted', 'limit': 20})
                    assert 'moved.txt' in json.dumps(listing, ensure_ascii=False)
                    passed(code, 'copy_move_search_and_archive_roundtrip')

                    opened = call('pab_open_terminal', {'device_code': code, 'cols': 91, 'rows': 27,
                                                       'execution': user['selection']}, observe=False)
                    terminal = opened['session_id']
                    call('pab_terminal_resize', {'session_id': terminal, 'cols': 105, 'rows': 31}, observe=False)
                    left, right = uuid.uuid4().int % 1_000_000_000, uuid.uuid4().int % 1_000_000_000
                    token = f'PTY_{left + right}_105_31'
                    # The expected output never appears verbatim in the input:
                    # terminal echo alone cannot satisfy this assertion. Verify
                    # the shell's observed size after the native PTY resize.
                    line = (f"echo ('PTY_' + ({left} + {right}) + '_' + $Host.UI.RawUI.WindowSize.Width + '_' + $Host.UI.RawUI.WindowSize.Height)"
                            if family == 'windows' else
                            f"(set -- $(stty size); printf 'PTY_%s_%s_%s\\n' \"$(({left}+{right}))\" \"$2\" \"$1\")")
                    assert token not in line
                    call('pab_terminal_input', {'session_id': terminal, 'data': line + '\r'}, observe=False)
                    deadline = time.monotonic() + 10
                    stream = ''
                    while token not in stream and time.monotonic() < deadline:
                        stream += call('pab_terminal_read', {'session_id': terminal}, observe=False)['text']
                        time.sleep(.1)
                    assert token in stream, 'PTY output missing'
                    call('pab_terminal_close', {'session_id': terminal}, observe=False)
                    terminal = None
                    passed(code, 'native_user_pty_resize_input_output_and_close')
                finally:
                    if terminal is not None:
                        call('pab_terminal_close', {'session_id': terminal}, observe=False, allow_error=True)
                    if created:
                        # Delete only the exact UUID path created by this run.
                        owned_temporary_path(family, root)
                        deleted = call('pab_file_delete', {'device_code': code, 'path': root, 'recursive': True,
                            'max_entries': 100, 'max_bytes': 10 * 1024 * 1024}, allow_error=True)
                        cleanup_ok = deleted.get('result', {}).get('state') == 'completed'
                    else:
                        cleanup_ok = True
                    call('pab_disconnect', {'device_code': code}, observe=False, allow_error=True)
                    report['cleanup'].append({'device': code, 'temporary_path': root, 'confirmed': cleanup_ok})
                    save()
                    assert cleanup_ok, 'Remote cleanup unconfirmed'
            report['status'] = 'passed'
        except BaseException as error:
            report['status'] = 'failed'
            report['error'] = str(error)
            raise
        finally:
            save()


if __name__ == '__main__':
    main()
