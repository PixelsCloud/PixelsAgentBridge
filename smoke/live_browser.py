"""Exercise a Pixels-owned browser on an explicit Windows/macOS test account.

Serves only the synthetic fixture on target loopback. Does not request handoff,
enter real credentials, change installed apps or touch ordinary browser profiles.
The separate human-confirmed handoff case remains mandatory for BROWSER-01.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path, PurePosixPath, PureWindowsPath
import re
import time
import tomllib
import uuid

from mcp_client import McpClient, McpToolError


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ('lab', 'config', 'mcp', 'output'):
        parser.add_argument('--' + key, type=Path, required=True)
    args = parser.parse_args()
    devices = tomllib.loads(args.lab.read_text(encoding='utf-8-sig'))['device']
    assert devices and all(d['os'] in ('windows', 'macos') and d.get('user') for d in devices)
    args.output.mkdir(parents=True, exist_ok=False)
    report = {'status': 'running', 'calls': [], 'checks': [], 'cleanup': []}

    def save():
        (args.output / 'result.json').write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')

    env = {k: v for k, v in os.environ.items() if not k.startswith('PAB_')}
    with McpClient(args.mcp.resolve(), args.output / 'mcp.log', config=args.config.resolve(), env=env) as c:
        def call(tool, values, *, observe=True):
            entry = {'tool': tool, 'arguments': values, 'value': None}
            report['calls'].append(entry)
            save()
            try:
                value = c.call(tool, values)
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
            assert not value.get('error') and value.get('result', {}).get('state') not in ('failed', 'cancelled', 'unconfirmed'), tool
            return value

        def check(code, name):
            report['checks'].append({'device': code, 'name': name, 'passed': True})
            save()
            print(code + ' ' + name + ': passed', flush=True)

        try:
            for device in devices:
                code = device['code']
                windows = device['os'] == 'windows'
                target_path = PureWindowsPath if windows else PurePosixPath
                server = None
                session = None
                root = None
                artifact_roots = set()
                try:
                    connected = call('pab_connect', {'device_code': code, 'wait_ms': 1000})
                    deadline = time.monotonic() + 50
                    while connected.get('state') == 'connecting' and time.monotonic() < deadline:
                        connected = call('pab_connect', {'device_code': code, 'wait_ms': 1000})
                    assert connected.get('connected')
                    contexts = call('pab_list_execution_contexts', {'device_code': code, 'user': device['user']})['result']['data']['entries']
                    desktop = [r for r in contexts if r['mode'] == 'desktop_user' and r.get('selection')]
                    users = [r for r in contexts if r['mode'] == 'user' and r.get('selection')]
                    assert len(desktop) == len(users) == 1, 'Select one exact available test account'
                    execution, user = desktop[0]['selection'], users[0]['selection']
                    home = target_path(users[0]['identity']['home'])
                    browser_args = {'device_code': code, 'execution': execution}

                    def browser(verb, **values):
                        v = call('pab_browser_' + verb, {**browser_args, **({'session_ref': session} if session else {}), **values})
                        b = v['result']['data']['snapshot']['result']
                        assert b['status'] in ('ready', 'connected', 'completed', 'closed'), verb + ': ' + b['status']
                        # Runtime links can be relative to its working directory.
                        # Reconstruct only known UUID artifact directories in this
                        # selected user's app data, never arbitrary linked paths.
                        for block in b['data'].get('content', []):
                            content = block.get('text', '')
                            for area, token in re.findall(r'AppData[\\/](Local|Roaming)[\\/]PixelsAgentBridge[\\/]browser-artifacts[\\/]([0-9a-f-]{36})[\\/]', content) if windows else []:
                                uuid.UUID(token)
                                artifact_roots.add(str(home / 'AppData' / area / 'PixelsAgentBridge/browser-artifacts' / token))
                            for token in re.findall(r'Library/Application Support/PixelsAgentBridge/browser-artifacts/([0-9a-f-]{36})/', content) if not windows else []:
                                uuid.UUID(token)
                                artifact_roots.add(str(home / 'Library/Application Support/PixelsAgentBridge/browser-artifacts' / token))
                        report['artifact_roots'] = sorted(artifact_roots)
                        save()
                        return b

                    assert browser('setup_info')['status'] == 'ready'
                    session = browser('attach')['session_ref']
                    root = str((home / 'AppData/Local/Temp' if windows else PurePosixPath('/tmp')) / ('pab-browser-' + str(uuid.uuid4())))
                    call('pab_mkdir', {'device_code': code, 'path': root, 'execution': user})
                    call('pab_file_write', {'device_code': code, 'path': root + '/upload.txt', 'content': 'Pixels synthetic upload', 'execution': user})
                    html = base64.b64encode((Path(__file__).parent / 'fixtures/browser.html').read_bytes()).decode()
                    source = 'browser_server.ps1' if windows else 'browser_server.py'
                    script = (Path(__file__).parent / 'fixtures' / source).read_text(encoding='utf-8').replace('__HTML_BASE64__', html)
                    command = ({'program': r'C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe',
                        'args': ['-NoProfile', '-NonInteractive', '-EncodedCommand', base64.b64encode(script.encode('utf-16-le')).decode()]}
                        if windows else {'program': '/usr/bin/python3', 'args': ['-u', '-c', script]})
                    accepted = call('pab_run_command', {'device_code': code, **command,
                        'execution': user, 'wait_ms': 1000, 'timeout_ms': 620000}, observe=False)
                    server = accepted['operation_ref']
                    stdout = accepted.get('output', {}).get('stdout', {}).get('text', '')
                    deadline = time.monotonic() + 15
                    while not stdout.strip() and time.monotonic() < deadline:
                        stdout = call('pab_read_output', {'device_code': code, 'task_id': server['task_id'], 'stream': 'stdout', 'wait_ms': 1000})['text']
                    port = json.loads(stdout)['port']
                    url = f'http://127.0.0.1:{port}/browser.html'
                    browser('open', url=url)

                    def text(result):
                        return '\n'.join(b.get('text', '') for b in result['data'].get('content', []))

                    def snapshot():
                        value = text(browser('snapshot'))
                        links = re.findall(r'\[Snapshot\]\(([^)]+)\)', value)
                        if links:
                            linked = target_path(links[-1])
                            parent = [r for r in artifact_roots if target_path(r).name == linked.parent.name]
                            assert len(parent) == 1 and linked.name.endswith('.yml'), 'unexpected snapshot path'
                            path = target_path(parent[0]) / linked.name
                            return call('pab_file_read', {'device_code': code, 'path': str(path), 'execution': user})['text']
                        return value

                    def target(role, label=None):
                        value = snapshot()
                        pattern = role + (r' "' + re.escape(label) + r'"' if label else '') + r'[^\n]*?\[ref=([^\]]+)\]'
                        matches = re.findall(pattern, value)
                        assert len(matches) == 1, 'fixture reference not unique: ' + role + ' ' + str(label)
                        return matches[0]

                    def artifact(result, suffix, magic):
                        links = re.findall(r'\[[^\]]+\]\(([^)]+)\)', text(result))
                        candidates = [target_path(p) for p in links if target_path(p).suffix == suffix]
                        assert len(candidates) == 1, 'one expected browser artifact'
                        linked = candidates[0]
                        parents = [r for r in artifact_roots if target_path(r).name == linked.parent.name]
                        assert len(parents) == 1, 'artifact is not in this browser session'
                        path = target_path(parents[0]) / linked.name
                        meta = call('pab_file_stat', {'device_code': code, 'path': str(path), 'execution': user})['result']['metadata']
                        assert 0 < meta['size'] <= 16 * 1024 * 1024, 'artifact byte budget'
                        destination = args.output.resolve() / (str(uuid.uuid4()) + suffix)
                        call('pab_download_file', {'device_code': code, 'source': str(path), 'destination': str(destination),
                            'execution': user, 'overwrite': False, 'wait_ms': 1000})
                        data = destination.read_bytes()
                        assert len(data) == meta['size'] and data.startswith(magic), 'browser artifact contents'
                        report.setdefault('artifacts', []).append({'path': str(destination), 'sha256': hashlib.sha256(data).hexdigest()})
                        save()

                    initial = snapshot()
                    assert 'Pixels smoke fixture' in initial and 'Not saved' in initial
                    check(code, 'owned_browser_loopback_fixture_and_accessibility_snapshot')
                    browser('fill', target=target('textbox', 'Name'), text='Pixels 中文 😀')
                    browser('select_option', target=target('combobox', 'Platform'), values=['android'])
                    browser('set_checked', target=target('checkbox', 'Enabled'), checked=True)
                    browser('click', target=target('button', 'Save'))
                    assert 'Pixels 中文 😀 / android / true' in snapshot()
                    check(code, 'unicode_fill_select_checkbox_and_submit_content')
                    name_ref = target('textbox', 'Name')
                    platform_ref = target('combobox', 'Platform')
                    check_ref = target('checkbox', 'Enabled')
                    browser('fill_form', fields=[
                        {'name': 'Name', 'type': 'textbox', 'target': name_ref, 'value': 'Form value'},
                        {'name': 'Platform', 'type': 'combobox', 'target': platform_ref, 'value': 'Windows'},
                        {'name': 'Enabled', 'type': 'checkbox', 'target': check_ref, 'value': 'false'}])
                    browser('click', target=target('button', 'Save'))
                    assert 'Form value / windows / false' in snapshot()
                    assert 'Form value' in text(browser('extract', target=target('textbox', 'Name'), kind='value'))
                    assert 'Android' in text(browser('extract', target=target('table'), kind='table'))
                    assert 'Android' in text(browser('find', text='Android'))
                    browser('hover', target=target('button', 'Save'))
                    browser('fill', target=target('textbox', 'Name'), text='temporary')
                    browser('press_key', key='ControlOrMeta+A')
                    browser('press_key', key='Backspace')
                    assert '""' in text(browser('extract', target=target('textbox', 'Name'), kind='value'))
                    check(code, 'form_fill_extract_table_find_hover_and_keyboard')

                    browser('click', target=target('button', 'Prompt'))
                    browser('handle_dialog', accept=True, prompt_text='Pixels prompt result')
                    assert 'Pixels prompt result' in snapshot()

                    def generic(label):
                        matches = re.findall(r'(?m)^.*\[ref=([^\]]+)\][^\n]*: ' + re.escape(label) + r'$', snapshot())
                        assert len(matches) == 1, 'fixture generic reference: ' + label
                        return matches[0]

                    source, destination = generic('Drag source'), generic('Drop target')
                    browser('drag', start_target=source, end_target=destination)
                    assert 'Pixels drag' in snapshot()
                    browser('drop', target=generic('Pixels drag'), data={'text/plain': 'Explicit drop result'})
                    assert 'Explicit drop result' in snapshot()
                    check(code, 'prompt_dialog_and_both_drag_drop_methods')

                    browser('click', target=target('button', 'Upload'))
                    browser('file_upload', paths=[root + '/upload.txt'])
                    assert 'upload.txt' in snapshot()
                    browser('click', target=target('link', 'Download fixture'))
                    downloads = browser('downloads', filename='pixels-smoke.txt')['data']['downloads']
                    assert len(downloads) == 1 and downloads[0]['sha256'] == hashlib.sha256(b'Pixels smoke download').hexdigest()
                    check(code, 'owned_fixture_upload_and_tracked_download_hash')

                    browser('scroll', direction='down', pages=1)
                    assert 'Bottom marker' in snapshot()
                    browser('scroll', direction='up', pages=1)
                    browser('press_key', key='ControlOrMeta+Home')
                    browser('fill', target=target('textbox', 'Name'), text='Pointer result')
                    geometry = snapshot()
                    box = re.search(r'button "Save"[^\n]*\[box=(\d+),(\d+),(\d+),(\d+)\]', geometry)
                    assert box, 'current Save geometry'
                    x, y, width, height = map(int, box.groups())
                    artifact(browser('screenshot'), '.jpeg', b'\xff\xd8')
                    browser('pointer', action='move', x=x + width // 2, y=y + height // 2)
                    browser('pointer', action='click', x=x + width // 2, y=y + height // 2, button='left')
                    assert 'Pointer result / windows / false' in snapshot()
                    browser('pointer', action='wheel', delta_y=100, delta_x=0)
                    artifact(browser('pdf_save'), '.pdf', b'%PDF-')
                    assert 'Pixels smoke fixture ready' in text(browser('console_messages', level='info', all=True))
                    assert '127.0.0.1' in text(browser('network_requests', static=True, filter='127.0.0.1'))
                    check(code, 'scroll_screenshot_pointer_submit_pdf_and_page_diagnostics')

                    tabs = text(browser('tabs', action='new', url=url + '#second'))
                    assert '#second' in tabs
                    assert '#second' in text(browser('tabs', action='list'))
                    browser('tabs', action='select', index=1)
                    browser('navigate', url=url + '#one')
                    browser('navigate', url=url + '#two')
                    assert 'Page URL: ' + url + '#one' in text(browser('navigate', direction='back'))
                    assert 'Page URL: ' + url + '#two' in text(browser('navigate', direction='forward'))
                    browser('navigate', direction='reload')
                    browser('wait', text='Not saved')
                    assert 'Not saved' in snapshot()
                    browser('get_session')
                    check(code, 'tab_selection_navigation_history_reload_and_session_state')
                finally:
                    clean = {'device': code}
                    if session:
                        clean['browser_closed'] = browser('close')['status'] == 'closed'
                    if server:
                        stopped = call('pab_cancel_operation', {'device_code': code, 'operation_id': server['operation_id']})
                        clean['server_stopped'] = stopped.get('complete') or (stopped.get('snapshot') or {}).get('state') in ('cancelled', 'failed', 'succeeded')
                    for path in ([root] if root else []) + sorted(artifact_roots):
                        call('pab_file_delete', {'device_code': code, 'path': path, 'execution': user,
                            'recursive': True, 'max_entries': 100, 'max_depth': 3, 'max_bytes': 16 * 1024 * 1024})
                    clean['fixtures_removed'] = True
                    report['cleanup'].append(clean)
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
