#!/usr/bin/env python3
"""Accept package management over authenticated public relay on the USB phone.

ADB is limited to own normal home/pause controls and artifact metadata. No ADB
package install/uninstall, data clear, app-ops grants or privileged shell changes.
"""
from __future__ import annotations
import argparse
import hashlib
import json
from pathlib import Path
import re
import time
import uuid

from android_home import Home, normal_status
from android_accessibility import set_enabled
from android_public import PublicMcp
from android_scenarios import AndroidTasks, ControlMissingBeforeDispatch
from test_android_remote import ROOT, PACKAGE, adb, installed_apk_sha256

FIXTURE = 'com.pixels.acceptance.packagefixture'
INSTALLERS = {'com.miui.packageinstaller', 'com.android.packageinstaller',
              'com.google.android.packageinstaller', 'com.android.permissioncontroller'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', choices=['e2b3b128'], required=True)
    parser.add_argument('--operator-run-id', required=True, type=uuid.UUID)
    parser.add_argument('--device-code', required=True)
    parser.add_argument('--apk-sha256', required=True)
    parser.add_argument('--mcp-executable', required=True, type=Path)
    parser.add_argument('--prepare', action='store_true', help='Inspect and open normal install-source settings if needed')
    parser.add_argument('--fault-process', action='store_true', help='Kill only the verified Pixels process while an owned uninstall awaits confirmation')
    parser.add_argument('--continue-run', type=uuid.UUID, help='Observe the original install from a stopped owned cohort; never dispatch it again')
    parser.add_argument('--removal-only', action='store_true', help='Verify removal of the already installed owned v2 fixture without installing another APK')
    parser.add_argument('--removal-token', type=uuid.UUID, help='Expected fixture data token from the prior successful update; required for removal-only')
    parser.add_argument('--continue-removal-run', type=uuid.UUID, help='Resume the original uninstall from an interrupted removal-only cohort')
    parser.add_argument('--confirmation-timeout', type=int, default=90)
    args = parser.parse_args()
    assert not (args.continue_run and args.removal_only)
    assert not args.removal_only or args.removal_token is not None
    assert not args.continue_removal_run or args.removal_only
    assert installed_apk_sha256(args.serial) == args.apk_sha256
    root = ROOT / '.build/android/app-management'
    run = root / str(uuid.uuid4()); run.mkdir(parents=True)
    artifacts = json.loads((root / 'fixtures/artifacts.json').read_text(encoding='utf-8'))
    assert artifacts['package_id'] == FIXTURE
    result = {'status': 'failed', 'serial': args.serial, 'apk_sha256': args.apk_sha256,
              'mcp_sha256': hashlib.sha256(args.mcp_executable.read_bytes()).hexdigest(),
              'forced_relay': True, 'public_server': True, 'checks': [], 'whole_android_accepted': False,
              'fixture_package': FIXTURE, 'fixture_removed': False, 'test_files_cleanup_pending': False,
              'removal_only': args.removal_only}
    home = Home(args.serial); client = None; tasks = None
    directory = '/workspace/package-qa-' + run.name
    selection = None
    continued_request = None
    continued_removal = None
    if args.continue_removal_run:
        previous = root / str(args.continue_removal_run)
        prior = json.loads((previous / 'result.json').read_text(encoding='utf-8'))
        assert prior['status'] == 'failed' and prior['serial'] == args.serial and prior['apk_sha256'] == args.apk_sha256
        assert prior['removal_only'] and prior['checks'][-1]['name'] == 'existing_owned_v2_fixture_observed'
        records = [json.loads(line) for line in (previous / 'trace.jsonl').read_text(encoding='utf-8').splitlines()]
        continued_removal = next(row['operation_ref']['operation_id'] for row in reversed(records) if row['tool'] == 'pab_uninstall_app' and row['state'] == 'completed')
        result['continued_removal_from'] = str(args.continue_removal_run)
    if args.continue_run:
        previous = root / str(args.continue_run)
        prior = json.loads((previous / 'result.json').read_text(encoding='utf-8'))
        assert prior['status'] == 'failed' and prior['serial'] == args.serial and prior['apk_sha256'] == args.apk_sha256
        assert [check['name'] for check in prior['checks']][-1] == 'system_install_cancelled_without_installation'
        records = [json.loads(line) for line in (previous / 'trace.jsonl').read_text(encoding='utf-8').splitlines()]
        continued_request = next(row['request_id'] for row in reversed(records) if row['tool'] == 'pab_install_app' and row['state'] == 'completed' and row['request_id'])
        directory = '/workspace/package-qa-' + str(args.continue_run)
        result['continued_from'] = str(args.continue_run)

    def passed(name, **fields):
        result['checks'].append({'name': name, 'passed': True, **fields})
        (run / 'result.json').write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding='utf-8')
        print(json.dumps({'check': name, 'passed': True}), flush=True)

    def call(name, fields=None, error=None):
        fields = fields or {}
        value, raw = client.tool(name, {'device_code': args.device_code, **fields}, allow_error=True)
        deadline = time.monotonic() + 120
        while value.get('result', {}).get('state') == 'running':
            assert time.monotonic() < deadline, 'Query original, never redispatch'
            value, raw = client.tool('pab_get_operation', {'device_code': args.device_code,
                'operation_id': value['operation_ref']['operation_id'], 'wait_ms': 1000}, allow_error=True)
        reply = value.get('result', {})
        code = reply.get('error') or value.get('error')
        if isinstance(code, dict): code = code.get('code')
        with (run / 'trace.jsonl').open('a', encoding='utf-8') as stream:
            stream.write(json.dumps({'tool': name, 'request_id': fields.get('request_id'),
                'operation_ref': value.get('operation_ref'), 'state': reply.get('state'), 'error': code}) + '\n')
        if error:
            assert code == error, (name, code, value)
        else:
            assert not raw.get('isError'), (name, value)
            if 'result' in value: assert reply['state'] == 'completed', (name, reply)
        return value

    def package(name, fields=None, error=None):
        value = call(name, {'execution': selection, **(fields or {})}, error)
        return None if error else value['result']['data']['snapshot']['result']

    def state(change):
        return package('pab_get_app_change', {'change_ref': change})

    def list_fixture():
        return call('pab_list_apps', {'execution': selection, 'search': FIXTURE})['result']['data']['snapshot']['snapshot']['apps']

    def transfer(source, path):
        value = call('pab_upload_file', {'source': str(source), 'destination': path, 'wait_ms': 1000})
        operation = value['operation_ref']['operation_id']
        deadline = time.monotonic() + 90
        while True:
            value = call('pab_get_operation', {'operation_id': operation, 'wait_ms': 1000})
            record = value.get('operation', {})
            terminal = value.get('state') or record.get('state') or record.get('operation', {}).get('state')
            if terminal in ('completed', 'failed', 'cancelled', 'unconfirmed', 'interrupted'):
                assert terminal == 'completed', value
                return
            assert time.monotonic() < deadline

    def system_confirmation(change, accept):
        deadline = time.monotonic() + args.confirmation_timeout
        clicked = set()
        resumed = False
        account_reported = False
        while time.monotonic() < deadline:
            fact = state(change)
            if fact['state'] in ('succeeded', 'cancelled', 'failed', 'unconfirmed'):
                (run / ('change-' + change + '.json')).write_text(json.dumps(fact, ensure_ascii=False, indent=2), encoding='utf-8')
                if not accept:
                    assert clicked, 'System aborted before requested UI cancellation; not proof of user cancellation'
                return fact
            screen = tasks.query()
            pkg = screen.get('package_name')
            saved = screen if pkg != 'com.xiaomi.account' else {'package_name': pkg, 'identity_verification_required': True}
            (run / 'last-system-screen.json').write_text(json.dumps(saved, ensure_ascii=False, indent=2), encoding='utf-8')
            if pkg == 'com.xiaomi.account':
                if not account_reported:
                    print(json.dumps({'waiting': 'owner_system_account_verification', 'change_ref': change}), flush=True)
                    account_reported = True
                time.sleep(1)
                continue
            if pkg in INSTALLERS:
                # Only act on the owned fixture's installer after its pinned request.
                names = [n.get('name') for n in screen['elements'] if n.get('name')]
                warning = '已了解此应用未经安全检测'
                if accept and warning in names and tasks.has(screen, 'Pixels Package Fixture') and ('fixture_warning',) not in clicked:
                    try:
                        tasks.act(warning)
                    except ControlMissingBeforeDispatch:
                        continue
                    clicked.add(('fixture_warning',)); time.sleep(.5)
                    continue
                options = ('安装', '继续安装', '继续更新', '继续', '更新', 'Install', 'Continue', 'Update', '卸载', 'Uninstall', '确定', 'OK', '完成', 'Done') if accept else ('取消', '取消安装', '取消更新', 'Cancel', '完成', 'Done')
                visible = tuple(sorted(names))
                label = next((s for s in options if any(n.get('name') == s and n.get('enabled') for n in screen['elements']) and (pkg, s, visible) not in clicked), None)
                if label:
                    node = tasks.named(screen, label)
                    if not node['clickable']:
                        bounds = node['bounds']
                        containers = [item for item in screen['elements'] if item['clickable'] and item['enabled'] and item['role'] in ('android.widget.Button', 'android.widget.FrameLayout') and item['bounds']['left'] <= bounds['left'] and item['bounds']['right'] >= bounds['right'] and item['bounds']['top'] <= bounds['top'] and item['bounds']['bottom'] >= bounds['bottom']]
                        assert len(containers) == 1, 'Installer label must identify one observed button'
                        node = containers[0]
                    action, _ = tasks.call({'action': 'click', 'element_ref': node['element_ref']}, allow_pre_dispatch_errors=('target_stale',))
                    if action['state'] == 'failed':
                        # Explicitly rejected before dispatch: obtain a fresh
                        # observation and reselect this same visible UI control.
                        continue
                    clicked.add((pkg, label, visible)); time.sleep(.5)
                # The installer may first expose only its loading/toast window.
                # Wait for an observed button instead of cancelling that window
                # with Back before the confirmation has actually appeared.
                else: time.sleep(.5)
            elif fact['state'] == 'waiting_user_action' and not clicked and not resumed:
                package('pab_resume_app_change', {'change_ref': change})
                resumed = True
                time.sleep(.5)
            else: time.sleep(.5)
        raise RuntimeError('System confirmation unresolved; preserve change_ref and query, never replay: ' + change)

    def launch_and_token(version):
        call('pab_launch_app', {'execution': selection, 'application': {'kind': 'id', 'id': FIXTURE}})
        deadline = time.monotonic() + 20
        launch_confirmed = False
        while time.monotonic() < deadline:
            screen = tasks.query()
            if screen['package_name'] == FIXTURE and tasks.has(screen, 'Retained:'):
                break
            if (not launch_confirmed and screen['package_name'] == 'com.miui.securitycenter'
                    and tasks.has(screen, 'Pixels Package Fixture') and tasks.has(screen, 'Pixels Android')
                    and tasks.named(screen, '始终允许')):
                tasks.act('始终允许')
                launch_confirmed = True
            time.sleep(.3)
        else:
            (run / 'last-launch-screen.json').write_text(json.dumps(screen, ensure_ascii=False, indent=2), encoding='utf-8')
            raise AssertionError('Installed fixture did not become visible after its launch')
        assert screen['package_name'] == FIXTURE and tasks.has(screen, f'Version: {version}')
        text = '\n'.join(n.get('name') or '' for n in screen['elements'])
        return re.search(r'Retained: ([0-9a-f-]{36})', text)[1]

    try:
        home.start(); home.set_paused(True)
        operator = ROOT / '.build/android/public' / str(args.operator_run_id)
        client = PublicMcp(operator / 'mcp.toml', args.device_code, True, run / 'mcp.log', args.mcp_executable)
        connected, raw = client.connect(); assert connected.get('connected') and not raw.get('isError')
        tasks = AndroidTasks(client, PACKAGE, run / 'mobile.jsonl')
        catalog = client.rpc('tools/list', {})
        (run / 'catalog.json').write_text(json.dumps(catalog, ensure_ascii=False, indent=2), encoding='utf-8')
        assert len(catalog['tools']) == 116
        call('pab_system_info', error='control_paused')
        home.set_paused(False)
        rows = call('pab_list_execution_contexts')['result']['data']['entries']
        selection = next(row['selection'] for row in rows if row['mode'] == 'desktop_user')
        home.set_paused(True)
        package('pab_inspect_app_package', {'path': '/workspace/missing.apk'}, error='control_paused')
        home.set_paused(False)
        passed('normal_pause_gate_and_package_tool_catalog')
        if continued_request:
            original = state(continued_request)
            assert original['package_id'] == FIXTURE and original['state'] in ('waiting_user_action', 'succeeded'), original
        elif args.removal_only:
            assert list_fixture(), 'Removal-only requires the owned v2 fixture'
        else:
            assert not list_fixture(), 'Independent fixture already installed: investigate before changing it'
        call('pab_mkdir', {'path': directory, 'parents': False})
        result['test_files_cleanup_pending'] = True
        infos = []
        for artifact in artifacts['artifacts']:
            source = Path(artifact['path']); assert hashlib.sha256(source.read_bytes()).hexdigest() == artifact['sha256']
            path = directory + '/' + source.name
            transfer(source, path)
            info = package('pab_inspect_app_package', {'path': path})
            assert info['state'] == 'inspected' and info['change_ref'] is None
            assert info['package_id'] == FIXTURE and info['package']['sha256'] == artifact['sha256']
            assert info['package']['version_code'] == artifact['version_code']
            infos.append(info)
        assert infos[0]['package']['signer_sha256'] == infos[1]['package']['signer_sha256'] != infos[2]['package']['signer_sha256']
        passed('authorized_apk_inspection_hash_versions_signers', artifacts=[i['package'] for i in infos])
        permission = infos[0]['can_request_installs']
        result['install_source_permission'] = permission
        if args.prepare:
            if not permission:
                package('pab_install_app', {'path': directory + '/fixture-v1.apk', 'package_id': FIXTURE,
                    'sha256': infos[0]['package']['sha256']}, error='install_source_permission_required')
                tasks.call({'action': 'launch_app', 'package_name': PACKAGE})
                # Use the normal home link, preserving Android's explicit local permission flow.
                for _ in range(12):
                    screen = tasks.query()
                    label = next((s for s in ('管理安装应用权限', 'Manage permission to install apps') if tasks.named(screen, s)), None)
                    if label: tasks.act(label); break
                    scroll = tasks.query(role='android.widget.ScrollView')['elements']; assert len(scroll) == 1
                    tasks.call({'action': 'scroll', 'element_ref': scroll[0]['element_ref'], 'direction': 'forward'})
                result['status'] = 'permission_required'
            else: result['status'] = 'prepared'
            return
        assert permission, 'Use normal install-source permission settings before acceptance'
        call('pab_file_write', {'path': directory + '/invalid.apk', 'content': 'not an APK', 'overwrite': False})
        package('pab_inspect_app_package', {'path': directory + '/invalid.apk'}, error='invalid_apk')
        package('pab_inspect_app_package', {'path': '/data/data/' + PACKAGE + '/settings.apk'}, error='mount_not_found')
        package('pab_get_app_change', {'change_ref': str(uuid.uuid4())}, error='app_change_not_found')
        package('pab_uninstall_app', {'package_id': PACKAGE, 'version_code': 1}, error='pixels_package_protected')
        package('pab_uninstall_app', {'package_id': 'com.android.settings', 'version_code': 1}, error='system_package_protected')
        passed('private_path_missing_fact_and_protected_packages_rejected')
        base = {'path': directory + '/fixture-v1.apk', 'package_id': FIXTURE, 'sha256': infos[0]['package']['sha256']}
        package('pab_install_app', {**base, 'sha256': '0' * 64}, error='apk_hash_mismatch')
        package('pab_install_app', {**base, 'package_id': 'com.pixels.acceptance.other'}, error='apk_package_mismatch')
        passed('install_hash_and_package_pins_enforced')
        if args.removal_only:
            token = launch_and_token(2)
            assert token == str(args.removal_token), 'Owned fixture data must match the prior accepted update'
            passed('existing_owned_v2_fixture_observed', retained_token=token)
        elif continued_request:
            request = continued_request
            install = state(request)
        else:
            cancelled = package('pab_install_app', base)
            assert system_confirmation(cancelled['change_ref'], False)['state'] == 'cancelled'
            assert not list_fixture()
            passed('system_install_cancelled_without_installation', change_ref=cancelled['change_ref'])
            request = str(uuid.uuid4())
            install = package('pab_install_app', {**base, 'request_id': request})
        if not args.removal_only:
            assert system_confirmation(install['change_ref'], True)['state'] == 'succeeded'
            token = launch_and_token(1)
            if continued_request:
                # A fresh connection has a different execution selection; the full
                # request digest must reject reusing its old ID with this new body.
                conflict = call('pab_install_app', {'execution': selection, **base, 'request_id': request}, error='operation_error')
                assert conflict['error']['message'] == 'the request ID already exists with a different device or command'
                assert conflict['error']['retry_action'] == 'query_original_operation'
            else:
                duplicate = package('pab_install_app', {**base, 'request_id': request})
                assert duplicate['change_ref'] == install['change_ref']
            assert state(install['change_ref'])['state'] == 'succeeded'
            passed('install_os_success_observed_launch_and_prior_request_conflict' if continued_request else 'install_os_success_observed_launch_and_request_dedup', change_ref=install['change_ref'], retained_token=token)
            package('pab_install_app', base, error='apk_version_not_newer')
            package('pab_install_app', {'path': directory + '/fixture-v3.apk', 'package_id': FIXTURE,
                'sha256': infos[2]['package']['sha256']}, error='apk_update_signer_mismatch')
            update_fields = {'path': directory + '/fixture-v2.apk', 'package_id': FIXTURE,
                'sha256': infos[1]['package']['sha256'], 'request_id': str(uuid.uuid4())}
            update = package('pab_install_app', update_fields)
            assert system_confirmation(update['change_ref'], True)['state'] == 'succeeded'
            assert launch_and_token(2) == token
            assert package('pab_install_app', update_fields)['change_ref'] == update['change_ref']
            package('pab_install_app', base, error='apk_version_not_newer')
            passed('cover_update_retains_application_data_and_rejects_signer_or_downgrade', change_ref=update['change_ref'])
        package('pab_uninstall_app', {'package_id': FIXTURE, 'version_code': 1}, error='installed_version_mismatch')
        removal = state(continued_removal) if continued_removal else package('pab_uninstall_app', {'package_id': FIXTURE, 'version_code': 2})
        if continued_removal:
            assert removal['state'] == 'waiting_user_action' and removal['package_id'] == FIXTURE
            package('pab_resume_app_change', {'change_ref': removal['change_ref']})
            deadline = time.monotonic() + 15
            while True:
                resumed_screen = tasks.query()
                if resumed_screen['package_name'] in INSTALLERS and tasks.has(resumed_screen, 'Pixels Package Fixture'):
                    break
                assert time.monotonic() < deadline, 'The original system confirmation must actually appear'
                time.sleep(.3)
            (run / 'resumed-system-screen.json').write_text(json.dumps(resumed_screen, ensure_ascii=False, indent=2), encoding='utf-8')
            passed('original_uninstall_confirmation_resumed_after_prior_process_fault', change_ref=removal['change_ref'], prior_run=str(args.continue_removal_run))
        elif args.fault_process:
            deadline = time.monotonic() + 15
            while state(removal['change_ref'])['state'] == 'submitted':
                assert time.monotonic() < deadline
                time.sleep(.2)
            assert state(removal['change_ref'])['state'] == 'waiting_user_action'
            home.set_paused(True)
            package('pab_resume_app_change', {'change_ref': removal['change_ref']}, error='control_paused')
            before = normal_status(args.serial)['pid']
            assert adb(args.serial, 'shell', 'run-as', PACKAGE, 'cat', f'/proc/{before}/cmdline').rstrip('\0') == PACKAGE
            client.tool('pab_disconnect', {}, allow_error=True); client.close(); client = None
            # Fault injection only, not package-management acceptance through ADB.
            adb(args.serial, 'shell', 'run-as', PACKAGE, 'kill', '-9', str(before))
            time.sleep(1)
            adb(args.serial, 'shell', 'am', 'start', '-n', PACKAGE + '/com.pixels.agent.MainActivity')
            recovery = set_enabled(args.serial, True)
            home.start(); home.set_paused(False)
            client = PublicMcp(operator / 'mcp.toml', args.device_code, True, run / 'process-recovery.log', args.mcp_executable)
            connected, raw = client.connect(); assert connected.get('connected') and not raw.get('isError')
            tasks = AndroidTasks(client, PACKAGE, run / 'mobile-recovery.jsonl')
            selection = next(row['selection'] for row in call('pab_list_execution_contexts')['result']['data']['entries'] if row['mode'] == 'desktop_user')
            assert normal_status(args.serial)['pid'] != before
            assert state(removal['change_ref'])['state'] == 'waiting_user_action'
            package('pab_resume_app_change', {'change_ref': removal['change_ref']})
            deadline = time.monotonic() + 15
            while True:
                resumed_screen = tasks.query()
                if resumed_screen['package_name'] in INSTALLERS and tasks.has(resumed_screen, 'Pixels Package Fixture'):
                    break
                assert time.monotonic() < deadline, 'Resume must reveal the original owned system confirmation'
                time.sleep(.3)
            (run / 'resumed-system-screen.json').write_text(json.dumps(resumed_screen, ensure_ascii=False, indent=2), encoding='utf-8')
            passed('pause_blocks_resume_and_process_restart_preserves_original_uninstall_confirmation', change_ref=removal['change_ref'], accessibility_recovery=recovery)
        assert system_confirmation(removal['change_ref'], False)['state'] == 'cancelled'
        assert launch_and_token(2) == token
        passed('uninstall_cancel_preserves_installed_version_and_data', change_ref=removal['change_ref'])
        removal = package('pab_uninstall_app', {'package_id': FIXTURE, 'version_code': 2})
        assert system_confirmation(removal['change_ref'], True)['state'] == 'succeeded'
        assert not list_fixture()
        result['fixture_removed'] = True
        passed('exact_version_uninstall_os_success_and_absence', change_ref=removal['change_ref'])
        # Reconnect creates a fresh execution context while durable package facts remain caller-bound.
        client.tool('pab_disconnect', {}, allow_error=True); client.close()
        client = PublicMcp(operator / 'mcp.toml', args.device_code, True, run / 'reconnect.log', args.mcp_executable)
        connected, raw = client.connect(); assert connected.get('connected') and not raw.get('isError')
        selection = next(row['selection'] for row in call('pab_list_execution_contexts')['result']['data']['entries'] if row['mode'] == 'desktop_user')
        assert state(removal['change_ref'])['state'] == 'succeeded'
        if not args.removal_only:
            assert state(install['change_ref'])['state'] == 'succeeded'
        passed('reconnect_queries_durable_original_facts_without_replay')
        result['status'] = 'passed'
    finally:
        if client is not None:
            try:
                if result['test_files_cleanup_pending']:
                    call('pab_file_delete', {'path': directory, 'recursive': True})
                    result['test_files_cleanup_pending'] = False
                client.tool('pab_disconnect', {}, allow_error=True)
            except Exception as failure:
                result['cleanup_error'] = type(failure).__name__
                result['status'] = 'failed'
            finally: client.close()
        try:
            home.set_paused(True)
            if result['status'] == 'permission_required':
                # The normal link was exercised over MCP above. After pausing, reopen
                # the same system setup page for the owner's explicit permission choice.
                adb(args.serial, 'shell', 'am', 'start', '-a', 'android.settings.MANAGE_UNKNOWN_APP_SOURCES', '-d', 'package:' + PACKAGE)
            result['ending_status'] = normal_status(args.serial)
        except Exception as failure:
            result.update(status='failed', home_cleanup_error=type(failure).__name__)
        (run / 'result.json').write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding='utf-8')
        print(json.dumps({'status': result['status'], 'checks': len(result['checks']), 'result': str(run / 'result.json')}), flush=True)


if __name__ == '__main__':
    main()
