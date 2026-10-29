#!/usr/bin/env python3
"""Public MCP acceptance of Android system queries and application operations.

ADB only opens the ordinary home, changes its normal pause control, and verifies
artifact/device metadata. All exercised capabilities use authenticated PAB relay.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
from pathlib import Path
import time
import uuid

from android_home import Home, normal_status
from android_public import PublicMcp
from android_scenarios import AndroidTasks
from test_android_remote import ROOT, PACKAGE, adb, installed_apk_sha256


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--operator-run-id', required=True, type=uuid.UUID)
    parser.add_argument('--device-code', required=True)
    parser.add_argument('--apk-sha256', required=True)
    parser.add_argument('--mcp-executable', required=True, type=Path)
    args = parser.parse_args()
    assert installed_apk_sha256(args.serial) == args.apk_sha256
    operator = ROOT / '.build/android/public' / str(args.operator_run_id)
    run = ROOT / '.build/android/system' / str(uuid.uuid4())
    run.mkdir(parents=True)
    result = {'status': 'failed', 'serial': args.serial, 'apk_sha256': args.apk_sha256,
        'mcp_sha256': hashlib.sha256(args.mcp_executable.read_bytes()).hexdigest(),
        'forced_relay': True, 'public_server': True, 'checks': [], 'final_acceptance': False}
    home = Home(args.serial)
    previous_paused = normal_status(args.serial)['paused']
    def fixture_viewer(enabled):
        adb(args.serial, 'shell', 'am', 'broadcast', '-n', f'{PACKAGE}/com.pixels.agent.fixture.ProductionProbeReceiver',
            '--es', 'production_request_id', str(uuid.uuid4()), '--es', 'probe_action', 'file-view-fixture',
            '--ez', 'fixture_enabled', str(enabled).lower())
        time.sleep(.3)
    client = None
    tasks = None
    own_path = '/workspace/pixels-system-' + run.name + '.txt'
    html_path = '/workspace/pixels-system-' + run.name + '.html'
    result['test_path'] = own_path
    result['test_paths'] = [own_path, html_path]
    result['test_file_cleanup_pending'] = False

    def call(name, fields=None, expected_error=None):
        fields = fields or {}
        value, raw = client.tool(name, {'device_code': args.device_code, **fields}, allow_error=True)
        deadline = time.monotonic() + 120
        while value.get('result', {}).get('state') == 'running':
            assert time.monotonic() < deadline, 'Query original operation; never replay'
            reference = value['operation_ref']
            value, raw = client.tool('pab_get_operation', {'device_code': args.device_code,
                'operation_id': reference['operation_id'], 'wait_ms': 1000}, allow_error=True)
        reply = value.get('result', {})
        error = reply.get('error') or value.get('error')
        code = error.get('code') if isinstance(error, dict) else error
        with (run / 'operation-trace.jsonl').open('a', encoding='utf-8') as out:
            out.write(json.dumps({'tool': name, 'request_id': fields.get('request_id'),
                'operation_ref': value.get('operation_ref'), 'state': reply.get('state'),
                'error': code, 'is_error': bool(raw.get('isError'))}) + '\n')
        if expected_error is None:
            assert not raw.get('isError') and reply.get('state') == 'completed', (name, code, reply.get('state'))
        else:
            assert code == expected_error, (name, code)
        return reply

    def passed(name, **metadata):
        result['checks'].append({'name': name, 'passed': True, **metadata})
        print(json.dumps({'check': name, 'passed': True}), flush=True)

    def observe_package(package):
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            sample, _ = tasks.call({'action': 'query', 'root_ref': None, 'text_contains': None,
                'role': None, 'max_nodes': 128, 'max_depth': 32}, allow_pre_dispatch_errors=('target_stale',))
            if sample['state'] != 'completed':
                time.sleep(.1)
                continue
            screen = sample['observation']
            if screen.get('package_name') == package:
                return screen
            time.sleep(.1)
        raise RuntimeError('Accepted activity did not become observable: ' + package)

    try:
        home.start()
        home.set_paused(False)
        client = PublicMcp(operator / 'mcp.toml', args.device_code, True, run / 'mcp.log', args.mcp_executable)
        tasks = AndroidTasks(client, PACKAGE, run / 'mobile-trace.jsonl')
        catalog = client.rpc('tools/list', {})
        encoded = json.dumps(catalog, ensure_ascii=False, indent=2).encode('utf-8')
        (run / 'catalog.json').write_bytes(encoded)
        result['catalog_sha256'] = hashlib.sha256(encoded).hexdigest()
        result['catalog_tools'] = len(catalog['tools'])
        connected, raw = client.connect()
        assert connected.get('connected') and not raw.get('isError')
        # The current product exposes connection controls, not a pause button.
        # Disconnection gates are covered by test_android_execution.py.
        result['not_applicable'] = ['removed_pause_button']
        info = call('pab_system_info', {'sample_cpu': True})['data']['info']
        sdk = int(adb(args.serial, 'shell', 'getprop', 'ro.build.version.sdk').strip())
        assert info['os_name'] == 'Android' and info['mobile']['sdk'] == sdk
        assert info['memory_total_bytes'] >= info['memory_available_bytes'] > 0
        assert info['mobile']['battery_percent'] is None or 0 <= info['mobile']['battery_percent'] <= 100
        assert info['executor']['pid'] == normal_status(args.serial)['pid']
        passed('system_memory_battery_network', sdk=sdk, model=info['mobile']['model'])
        disk = call('pab_list_disks')['data']['entries'][0]
        assert disk['mount_point'] == '/workspace' and disk['total_bytes'] >= disk['available_bytes'] > 0
        passed('workspace_filesystem_capacity')
        processes = call('pab_list_processes', {'sample_cpu': True})['data']['entries']
        assert len(processes) == 1 and processes[0]['pid'] == info['executor']['pid']
        process = call('pab_get_process', {'pid': processes[0]['pid']})['data']['process']
        assert process['user_id'].startswith('uid:') and process['termination_identity'] is None
        call('pab_get_process', {'pid': 1}, expected_error='process_not_visible_to_app')
        passed('process_scope_and_foreign_pid_denial')
        networks = call('pab_list_network_interfaces')['data']['entries']
        assert networks and all(n['name'] and n['addresses'] for n in networks)
        assert all(n['received_bytes'] is None or n['received_bytes'] >= 0 for n in networks)
        passed('visible_network_interfaces', count=len(networks))
        for name in ('localhost', 'example.com'):
            dns = call('pab_resolve_dns', {'name': name, 'record_type': 'A', 'timeout_ms': 10000})['data']['result']
            assert dns['records'] and all(r['ttl_seconds'] is None for r in dns['records'])
        ipv6 = call('pab_resolve_dns', {'name': 'example.com', 'record_type': 'AAAA', 'timeout_ms': 10000})['data']['result']
        assert all(r['record_type'] == 'AAAA' and r['ttl_seconds'] is None for r in ipv6['records'])
        call('pab_resolve_dns', {'name': 'example.com', 'record_type': 'TXT'}, expected_error='android_dns_record_type_unavailable')
        passed('native_dns_A_AAAA_and_unavailable_record_type', ipv6_records=len(ipv6['records']))
        rows = call('pab_list_execution_contexts')['data']['entries']
        service = next(row for row in rows if row['mode'] == 'service')
        interactive = next(row for row in rows if row['mode'] == 'desktop_user')
        assert service['selection']['mode'] == 'service' and interactive['unavailable_reason'] is None
        selection = interactive['selection']
        passed('verified_android_activity_context')
        apps = call('pab_list_apps', {'execution': selection, 'search': PACKAGE})['data']['snapshot']['snapshot']
        own = next(app for app in apps['apps'] if app['app_id'] == PACKAGE)
        assert own['details']['enabled'] and own['details']['launchable'] and own['details']['version_code'] >= 20
        running = call('pab_list_apps', {'execution': selection, 'scope': 'running'})['data']['snapshot']['snapshot']['apps']
        assert len(running) == 1 and running[0]['app_id'] == PACKAGE
        limited = call('pab_list_apps', {'execution': selection, 'limit': 1})
        assert limited['truncated'] and limited['returned_count'] == 1
        passed('visible_app_details_running_scope_and_limits')
        call('pab_list_apps', {'execution': {'mode': 'desktop_user', 'context_ref': str(uuid.uuid4())}},
            expected_error='execution_context_expired_or_wrong_caller')
        call('pab_launch_app', {'execution': selection, 'application': {'kind': 'id', 'id': PACKAGE}, 'new_instance': True},
            expected_error='android_new_instance_unavailable')
        call('pab_launch_app', {'execution': selection, 'application': {'kind': 'id', 'id': 'com.pixels.absent.a' + uuid.uuid4().hex}},
            expected_error='application_not_visible_or_launchable')
        passed('application_context_and_launch_rejections')
        request = str(uuid.uuid4())
        launch = {'execution': selection, 'application': {'kind': 'id', 'id': 'com.android.settings'}, 'request_id': request}
        action = call('pab_launch_app', launch)['data']['snapshot']['result']
        assert action['request_accepted'] and action['window_ready'] is None
        observe_package('com.android.settings')
        tasks.call({'action': 'launch_app', 'package_name': PACKAGE})
        call('pab_launch_app', launch)
        observe_package(PACKAGE)
        passed('launch_observed_and_duplicate_not_dispatched', request_id=request)
        text = 'Pixels system QA ' + run.name + '\n中文文件预览\n'
        expected = hashlib.sha256(text.encode()).hexdigest()
        call('pab_file_write', {'path': own_path, 'content': text, 'overwrite': False})
        result['test_file_cleanup_pending'] = True
        fixture_viewer(True)
        for _ in range(3):
            opened = call('pab_open_file', {'execution': selection, 'path': own_path,
                'application': {'kind': 'id', 'id': PACKAGE}})['data']['snapshot']['result']
            assert opened['request_accepted']
            tasks.wait(expected)
            screen = tasks.query()
            assert tasks.has(screen, expected) and tasks.has(screen, 'read_only=true')
        call('pab_open_file', {'execution': selection, 'path': '/data/data/' + PACKAGE + '/no_backup/settings.toml',
            'application': {'kind': 'id', 'id': PACKAGE}}, expected_error='mount_not_found')
        passed('authorized_file_snapshot_opened_and_write_denied', sha256=expected)
        html_apps = call('pab_list_apps', {'execution': selection, 'search': 'com.android.htmlviewer'})['data']['snapshot']['snapshot']['apps']
        assert any(app['app_id'] == 'com.android.htmlviewer' and app['details']['enabled'] for app in html_apps)
        marker = 'Pixels external file ' + run.name
        call('pab_file_write', {'path': html_path, 'content': '<html><meta charset="utf-8"><body><h1>' + marker + '</h1></body></html>', 'overwrite': False})
        call('pab_open_file', {'execution': selection, 'path': html_path,
            'application': {'kind': 'id', 'id': 'com.android.htmlviewer'}})
        observe_package('com.android.htmlviewer')
        tasks.wait(marker)
        tasks.call({'action': 'navigate', 'destination': 'back'})
        tasks.call({'action': 'launch_app', 'package_name': PACKAGE})
        observe_package(PACKAGE)
        passed('external_application_reads_granted_content_snapshot')
        sources = call('pab_system_log_sources')['data']['snapshot']['sources']
        assert len(sources) == 1 and sources[0]['source_id'] == 'android:current-app:main'
        logs = call('pab_system_log_read', {'sources': [sources[0]['source_id']], 'message_contains': 'file_snapshot_opened', 'limit': 1})['data']['snapshot']
        assert len(logs['events']) == 1 and logs['integrity'] == 'partial' and logs['has_more']
        event = logs['events'][0]
        chunk = call('pab_system_log_event_read', {'event_ref': event['event_ref']})['data']['snapshot']
        data = base64.b64decode(chunk['data_base64'])
        assert len(data) == event['total_bytes'] and hashlib.sha256(data).hexdigest() == event['sha256'] == chunk['sha256']
        next_page = call('pab_system_log_read', {'cursor': logs['cursor'], 'limit': 16})['data']['snapshot']
        assert len(next_page['events']) >= 2 and not next_page['has_more']
        call('pab_system_log_read', {'mode': 'follow'}, expected_error='android_log_follow_unavailable')
        call('pab_system_log_event_read', {'event_ref': str(uuid.uuid4())}, expected_error='log_event_expired_or_wrong_caller')
        passed('app_log_source_snapshot_cursor_event_hash_and_limits')
        call('pab_file_delete', {'path': own_path})
        call('pab_file_delete', {'path': html_path})
        call('pab_file_stat', {'path': own_path}, expected_error='not_found')
        result['test_file_cleanup_pending'] = False
        passed('remote_file_cleanup_and_existing_file_tools')
        assert installed_apk_sha256(args.serial) == args.apk_sha256
        result['status'] = 'passed'
    finally:
        try:
            fixture_viewer(False)
            disabled = adb(args.serial, 'shell', 'dumpsys', 'package', PACKAGE).partition('disabledComponents:')[2]
            result['debug_viewer_disabled'] = 'com.pixels.agent.fixture.FileViewFixtureActivity' in disabled
            if not result['debug_viewer_disabled']:
                result['status'] = 'failed'
        except Exception as error:
            result.update(status='failed', fixture_cleanup_error=type(error).__name__)
        if client is not None:
            client.close()
        try:
            home.set_paused(previous_paused)
            result['ending_status'] = normal_status(args.serial)
        except Exception as error:
            result.update(status='failed', pause_cleanup_error=type(error).__name__)
        (run / 'result.json').write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding='utf-8')
        print(json.dumps({'status': result['status'], 'checks': len(result['checks']), 'result': str(run / 'result.json')}), flush=True)


if __name__ == '__main__':
    main()
