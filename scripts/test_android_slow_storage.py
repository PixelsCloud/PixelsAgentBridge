#!/usr/bin/env python3
"""Normal SAF consent plus public relay acceptance of the debug slow provider."""
from __future__ import annotations
import argparse
import hashlib
import json
import math
from pathlib import Path
import socket
import secrets
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
    parser.add_argument('--restart-service', action='store_true', help='Exercise ordinary connection stop/start on the disposable emulator')
    args = parser.parse_args()
    if args.restart_service: assert args.serial.startswith('emulator-')
    assert installed_apk_sha256(args.serial) == args.apk_sha256
    directory = ROOT / '.build/android/file-enhancement' / ('slow-' + str(uuid.uuid4()))
    directory.mkdir(parents=True)
    operator = ROOT / '.build/android/public' / str(args.operator_run_id)
    provider = PACKAGE + '/com.pixels.agent.fixture.SlowDocumentsProvider'
    title = 'Pixels 慢存储验收'
    result = {'status': 'failed', 'serial': args.serial, 'apk_sha256': args.apk_sha256,
        'forced_relay': True, 'real_system_consent': False, 'checks': [],
        'mcp_sha256': hashlib.sha256(args.mcp_executable.read_bytes()).hexdigest(),
        'qa_script_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        'grant_cleanup_pending': False, 'fixture_cleanup_pending': True}
    home = Home(args.serial)
    client, mount = None, None
    remote = '/workspace/pixels-slow-' + directory.name[-8:]
    tasks, terminals, forwards = {}, set(), set()
    latencies = []

    def call(name, fields=None, *, failure=False, device=True):
        fields = dict(fields or {})
        if device: fields['device_code'] = args.device_code
        start = time.monotonic()
        value, raw = client.tool(name, fields, allow_error=True)
        elapsed = time.monotonic() - start
        with (directory / 'trace.jsonl').open('a', encoding='utf-8') as output:
            output.write(json.dumps({'tool': name, 'fields': fields, 'value': value,
                'is_error': bool(raw.get('isError')), 'time': time.time(), 'elapsed_s': elapsed}, ensure_ascii=False) + '\n')
        if not failure: assert not raw.get('isError'), (name, value)
        return value

    def wait(value, *, failure=False):
        ref = value['operation_ref']; deadline = time.monotonic() + 70
        while value['result']['state'] in ('running', 'cancel_requested'):
            assert time.monotonic() < deadline, 'Only query original operation'
            time.sleep(.15)
            value = call('pab_get_operation', {'operation_id': ref['operation_id']}, failure=failure)
        if not failure: assert value['result']['state'] == 'completed', value
        return value

    def file(name, fields, *, failure=False):
        return wait(call(name, fields, failure=failure), failure=failure)

    def passed(name, **details):
        result['checks'].append({'name': name, 'passed': True, **details})
        print(json.dumps({'check': name, 'passed': True}), flush=True)

    def listing():
        value = call('pab_list_directory', {'path': '/', 'limit': 64})
        return {e['name'] for e in value.get('page', value)['entries'] if e['name'].startswith('shared-')}

    def fixture_root_target():
        deadline = time.monotonic() + 15
        while True:
            screen = mobile.query()
            assert screen['package_name'] in ('com.android.documentsui', 'com.google.android.documentsui')
            labels = [n for n in screen['elements'] if n.get('name') == title]
            if labels or time.monotonic() >= deadline: break
            time.sleep(.4)  # The roots cache populates asynchronously after PACKAGE_CHANGED.
        assert len(labels) == 1, screen
        label = labels[0]
        box = label['bounds']
        candidates = [n for n in screen['elements'] if n['clickable'] and n['enabled']
            and n['bounds']['left'] <= box['left'] and n['bounds']['right'] >= box['right']
            and n['bounds']['top'] <= box['top'] and n['bounds']['bottom'] >= box['bottom']]
        def area(n):
            b = n['bounds']; return (b['right'] - b['left']) * (b['bottom'] - b['top'])
        assert candidates, screen
        smallest = min(map(area, candidates))
        targets = [n for n in candidates if area(n) == smallest]
        assert len(targets) == 1, screen
        return targets[0]['element_ref']

    def open_fixture_root():
        for _ in range(3):
            time.sleep(.6)
            target = fixture_root_target()
            outcome, _ = mobile.call({'action': 'click', 'element_ref': target},
                allow_pre_dispatch_errors=('target_stale',))
            if outcome['state'] == 'completed': return
        raise RuntimeError('Fixture directory kept changing before any input was dispatched')

    def revoke():
        home.start(); home.set_paused(False)
        # No ambiguity with other people's grant controls is permitted.
        assert listing() == {mount[1:]}, 'Own fixture must be the sole grant for this UI driver'
        for attempt in range(28):
            screen = mobile.query()
            if mobile.has(screen, title) and mobile.has(screen, mount):
                buttons = [n for n in screen['elements'] if n.get('name') in ('撤销目录授权', 'Revoke directory access') and n['enabled']]
                if len(buttons) == 1:
                    mobile.act(buttons[0]['name']); break
            scrolls = mobile.query(role='android.widget.ScrollView')['elements']
            assert len(scrolls) == 1
            mobile.call({'action': 'scroll', 'element_ref': scrolls[0]['element_ref'],
                'direction': 'forward' if attempt < 14 else 'backward'}, allow_pre_dispatch_errors=('target_stale', 'unsupported_action'))
        else: raise AssertionError('Own grant revoke control unavailable')
        assert mount[1:] not in listing()
        result['grant_cleanup_pending'] = False

    try:
        home.start(); home.set_paused(False)
        adb(args.serial, 'shell', 'am', 'broadcast', '-n', PACKAGE + '/com.pixels.agent.fixture.ProductionProbeReceiver',
            '--es', 'production_request_id', str(uuid.uuid4()), '--es', 'probe_action', 'slow-storage-fixture',
            '--ez', 'fixture_enabled', 'true')
        time.sleep(2)
        client = PublicMcp(operator / 'mcp.toml', args.device_code, True,
            directory / 'mcp.log', args.mcp_executable)
        connected, raw = client.connect(); assert connected.get('connected') and not raw.get('isError')
        before = listing(); assert not before, 'Do not mix fixture consent with existing user grants'
        mobile = AndroidTasks(client, PACKAGE, directory / 'mobile-trace.jsonl')
        launch = adb(args.serial, 'shell', 'am', 'start', '-W', '-n', PACKAGE + '/com.pixels.agent.fixture.SlowStorageConsentActivity')
        assert 'Error:' not in launch and 'Permission Denial' not in launch, launch
        deadline = time.monotonic() + 15
        while True:
            screen = mobile.query()
            if screen.get('package_name') in ('com.android.documentsui', 'com.google.android.documentsui') or time.monotonic() >= deadline: break
            time.sleep(.3)
        assert screen['package_name'] in ('com.android.documentsui', 'com.google.android.documentsui'), screen
        if not mobile.has(screen, title):
            drawer = [n for n in screen['elements'] if n.get('name') in ('显示根目录', 'Show roots', 'Open navigation drawer')]
            assert len(drawer) == 1, screen
            mobile.act(drawer[0]['name']); open_fixture_root()
        deadline = time.monotonic() + 10
        while True:
            screen = mobile.query()
            use = [n['name'] for n in screen['elements'] if n.get('name') in ('使用此文件夹', 'Use this folder', 'USE THIS FOLDER', '选择', 'SELECT', 'Select') and n['enabled']]
            if use or time.monotonic() >= deadline: break
            time.sleep(.3)
        assert mobile.has(screen, title), screen
        assert len(use) == 1, screen
        mobile.act(use[0]); time.sleep(.3)
        screen = mobile.query()
        if screen.get('package_name') in ('com.android.documentsui', 'com.google.android.documentsui'):
            assert mobile.has(screen, title), screen
            allow = [n['name'] for n in screen['elements'] if n.get('name') in ('允许', 'Allow', 'ALLOW') and n['enabled']]
            assert len(allow) == 1, screen
            mobile.act(allow[0])
        time.sleep(.5)
        added = listing() - before; assert len(added) == 1, added
        mount = '/' + added.pop(); result.update(mount=mount, real_system_consent=True, grant_cleanup_pending=True)
        home.start()
        path = mount + '/slow.txt'
        started = time.monotonic()
        delayed = call('pab_file_read', {'path': path})
        assert delayed['result']['state'] == 'running', delayed
        # The five-second wait starts after durable admission on the device;
        # retain end-to-end latency separately from the control-lane SLA below.
        first_reply_s = time.monotonic() - started
        for _ in range(6):
            now = time.monotonic(); screen = mobile.query(); elapsed = time.monotonic() - now
            assert elapsed < 5 and screen['package_name'] == PACKAGE, (elapsed, screen)
            latencies.append(elapsed)
        original = wait(delayed)
        assert original['operation_ref']['operation_id'] == delayed['operation_ref']['operation_id']
        assert original['text'] == 'Pixels slow provider: 原请求正文😀\n', original
        passed('twenty_second_provider_does_not_block_observation_original_body_retrievable',
            maximum_s=max(latencies), p95_s=sorted(latencies)[math.ceil(.95 * len(latencies)) - 1],
            first_reply_s=first_reply_s,
            elapsed_s=time.monotonic() - started, operation_id=delayed['operation_ref']['operation_id'])
        home.set_paused(True)
        denied = call('pab_get_operation', {'operation_id': delayed['operation_ref']['operation_id']}, failure=True)
        assert denied['result']['state'] == 'completed' and denied['text'] is None and denied['result_data_error']['code'] == 'control_paused', denied
        home.set_paused(False)
        assert call('pab_get_operation', {'operation_id': delayed['operation_ref']['operation_id']})['text'] == original['text']
        passed('cached_body_requires_current_control_authorization_original_fact_preserved')

        file('pab_mkdir', {'path': remote}); result['remote_cleanup_pending'] = True
        file('pab_file_write', {'path': remote + '/echo.sh', 'content': 'exec /system/bin/cat\n'})
        port = 30000 + secrets.randbelow(25000)
        running = call('pab_run_command', {'program': '/system/bin/toybox', 'args': ['nc', '-s', '127.0.0.1', '-p', str(port), '-L', '/system/bin/sh', 'echo.sh'],
            'cwd': remote, 'timeout_ms': 120000, 'wait_ms': 0})
        task = running['task']['task_ref']['task_id']; tasks[task] = running['task']['request_id']
        terminal = call('pab_open_terminal', {'cols': 80, 'rows': 24})['session_id']; terminals.add(terminal)
        forward = call('pab_port_forward_open', {'remote_host': '127.0.0.1', 'remote_port': port})
        forwards.add(forward['forward_id']); time.sleep(.5)
        with socket.create_connection(('127.0.0.1', forward['local_port']), timeout=5) as held:
            held.settimeout(5); held.sendall(b'alive'); assert held.recv(5) == b'alive'
            active = call('pab_file_read', {'path': path})
            queued = call('pab_file_read', {'path': mount + '/fast.txt'})
            assert active['result']['state'] == queued['result']['state'] == 'running'
            cancelled = call('pab_cancel_operation', {'operation_id': queued['operation_ref']['operation_id']})
            assert cancelled['result']['state'] in ('cancelled', 'cancel_requested'), cancelled
            held.sendall(b'still'); assert held.recv(5) == b'still'
            call('pab_terminal_input', {'session_id': terminal, 'data': 'echo slow-storage-alive\n'}, device=False)
            tty = call('pab_terminal_read', {'session_id': terminal}, device=False)
            assert not tty['ended'], tty
            now = time.monotonic(); home.set_paused(True); pause_duration = time.monotonic() - now
            assert pause_duration < 5, pause_duration
            try: assert held.recv(1) == b''
            except (ConnectionResetError, ConnectionAbortedError): pass
            stopped = call('pab_terminal_read', {'session_id': terminal}, failure=True, device=False)
            # Android can remove the owned session as soon as pause closes it;
            # older timing also exposes an ended session before removal.
            assert stopped.get('ended') or any(code in json.dumps(stopped) for code in
                ('paused', 'closed', 'task_not_found_for_caller')), stopped
            terminal = None
        paused_active = wait(active, failure=True)
        assert paused_active['result']['state'] in ('failed', 'cancelled') and paused_active.get('text') is None, paused_active
        assert wait(cancelled, failure=True)['result']['state'] == 'cancelled'
        home.set_paused(False)
        cancelled_active = call('pab_file_read', {'path': path})
        cancelled_active = call('pab_cancel_operation', {'operation_id': cancelled_active['operation_ref']['operation_id']})
        assert wait(cancelled_active, failure=True)['result']['state'] == 'cancelled'
        passed('slow_io_queue_cancel_inflight_cancel_pause_stops_live_tcp_and_pty', pause_s=pause_duration)
        pipe = file('pab_file_read', {'path': mount + '/pipe.log', 'mode': 'tail'}, failure=True)
        assert pipe['result']['error']['code'] == 'log_seek_unavailable', pipe
        regular = file('pab_file_read', {'path': mount + '/fast.txt', 'mode': 'tail'})
        assert regular['text'] == 'Pixels fast provider\n', regular
        passed('saf_seekable_log_supported_pipe_explicitly_rejected')
        fast = file('pab_file_read', {'path': mount + '/fast.txt'})
        patch = {'path': mount + '/fast.txt', 'expected_hash': fast['result']['metadata']['sha256'],
            'edits': [{'find': 'fast', 'replace': 'changed'}]}
        preview = file('pab_file_patch', {**patch, 'dry_run': True})
        assert preview['result']['patch_preview']['changed']
        refused = file('pab_file_patch', patch, failure=True)
        assert refused['result']['error']['code'] == 'access_denied', refused
        assert file('pab_file_read', {'path': mount + '/fast.txt'})['text'] == fast['text']
        cached_search = file('pab_file_search', {'path': mount, 'mode': 'content', 'query': 'Pixels fast',
            'exclude': ['slow*', 'pipe.log']})
        assert cached_search['result']['search']['matches'][0]['preview']
        passed('readonly_preview_cannot_authorize_publication')
        if args.restart_service:
            abandoned = call('pab_file_read', {'path': path})
            assert abandoned['result']['state'] == 'running', abandoned
            now = time.monotonic(); home.click('disconnect')
            deadline = now + 5
            while normal_status(args.serial)['phase'] != 'DISCONNECTED':
                assert time.monotonic() < deadline, 'Connection stop waited for blocked provider IO'
                time.sleep(.1)
            stop_s = time.monotonic() - now
            home.start()
            restart_s = time.monotonic() - now
            connected, raw = client.connect(); assert connected.get('connected') and not raw.get('isError')
            fact = wait(abandoned, failure=True)
            assert fact['result']['state'] in ('interrupted', 'failed', 'cancelled', 'unconfirmed') and fact.get('text') is None, fact
            assert file('pab_file_read', {'path': mount + '/fast.txt'})['text'] == 'Pixels fast provider\n'
            # Let the uncooperative old SDK read return; it must never deliver a
            # body into the new service or retain its exact snapshot stage.
            deadline = time.monotonic() + 21
            while time.monotonic() < deadline:
                mobile.query(); time.sleep(.5)
            late = call('pab_get_operation', {'operation_id': abandoned['operation_ref']['operation_id']}, failure=True)
            assert late['result']['state'] != 'completed' and late.get('text') is None, late
            assert adb(args.serial, 'shell', 'run-as', PACKAGE, 'ls', '-A', 'no_backup/file-staging').strip() == ''
            passed('service_restart_during_slow_read_no_replay_no_late_body_and_stage_released', stop_s=stop_s, restart_s=restart_s)
        slow_listing = file('pab_file_search', {'path': mount, 'query': 'fast'})['result']['search']
        assert slow_listing['truncated'] and not slow_listing.get('next_cursor'), slow_listing
        assert file('pab_file_read', {'path': mount + '/fast.txt'})['text'] == 'Pixels fast provider\n'
        until = time.monotonic() + 21
        while time.monotonic() < until:
            now = time.monotonic(); mobile.query(); elapsed = time.monotonic() - now
            assert elapsed < 5, elapsed
            latencies.append(elapsed); time.sleep(.5)
        passed('slow_directory_query_is_bounded_incomplete_without_cursor_other_lane_recovers')
        # Revoke normally, then fetch the ORIGINAL completed result rather than reading again.
        revoke()
        denied = call('pab_get_operation', {'operation_id': delayed['operation_ref']['operation_id']}, failure=True)
        assert denied['result']['state'] == 'completed' and denied['text'] is None and denied['result_data_error']['code'] == 'mount_not_found', denied
        denied_search = call('pab_get_operation', {'operation_id': cached_search['operation_ref']['operation_id']}, failure=True)
        assert denied_search['result']['state'] == 'completed' and denied_search['result_data_error']['code'] == 'mount_not_found', denied_search
        assert all(not item.get('preview') and not item.get('context') for item in (denied_search['result'].get('search') or {}).get('matches', [])), denied_search
        passed('revoked_grant_cannot_retrieve_cached_body_original_fact_preserved')
        result['status'] = 'passed'
    except Exception as error:
        result['error'] = repr(error)
        raise
    finally:
        try:
            if client:
                home.set_paused(False)
                for forward in forwards: call('pab_port_forward_close', {'forward_id': forward}, failure=True, device=False)
                for terminal in terminals: call('pab_terminal_close', {'session_id': terminal}, failure=True, device=False)
                for request in tasks.values(): call('pab_cancel_operation', {'operation_id': request}, failure=True)
                if result.get('remote_cleanup_pending'):
                    file('pab_file_delete', {'path': remote, 'recursive': True}); result['remote_cleanup_pending'] = False
                if result['grant_cleanup_pending']: revoke()
                home.set_paused(True)
                result['final_state'] = normal_status(args.serial)
                call('pab_disconnect', {}, failure=True, device=False)
            if not result['grant_cleanup_pending']:
                adb(args.serial, 'shell', 'am', 'broadcast', '-n', PACKAGE + '/com.pixels.agent.fixture.ProductionProbeReceiver',
                    '--es', 'production_request_id', str(uuid.uuid4()), '--es', 'probe_action', 'slow-storage-fixture',
                    '--ez', 'fixture_enabled', 'false')
                time.sleep(.3)
                process = PACKAGE + ':storageFixture'
                pids = adb(args.serial, 'shell', 'pidof', process, check=False).split()
                for pid in pids:
                    assert pid.isdigit() and int(pid) > 1
                    actual = adb(args.serial, 'shell', 'run-as', PACKAGE, 'cat', f'/proc/{pid}/cmdline').rstrip('\x00')
                    assert actual == process, actual
                    adb(args.serial, 'shell', 'run-as', PACKAGE, 'kill', '-TERM', pid)
                adb(args.serial, 'shell', 'run-as', PACKAGE, 'rm', '-f', 'cache/slow-provider-fixture.txt')
                result['fixture_cleanup_pending'] = False
                home.start(); home.set_paused(True)
                result['final_state'] = normal_status(args.serial)
        except Exception as error:
            result['cleanup_error'] = repr(error); result['status'] = 'failed'
        if client: client.close()
        (directory / 'result.json').write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding='utf-8')
        print(json.dumps({'result': str(directory / 'result.json'), 'status': result['status']}), flush=True)
    assert result['status'] == 'passed', result.get('cleanup_error')


if __name__ == '__main__':
    main()
