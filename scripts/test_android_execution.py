#!/usr/bin/env python3
"""Command/PTY acceptance through the public authenticated PAB relay.

ADB is limited to artifact metadata and ordinary Home controls. Tested processes,
files, cancellation and terminal input/output all use the public MCP contract.
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
from test_android_remote import ROOT, PACKAGE, adb, installed_apk_sha256


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--operator-run-id', required=True, type=uuid.UUID)
    parser.add_argument('--device-code', required=True)
    parser.add_argument('--apk-sha256', required=True)
    parser.add_argument('--mcp-executable', required=True, type=Path)
    parser.add_argument('--restart-app', action='store_true', help='Owned process-stop fault on the disposable emulator only')
    args = parser.parse_args()
    if args.restart_app: assert args.serial.startswith('emulator-'), 'App-stop fault is limited to the disposable emulator'
    assert installed_apk_sha256(args.serial) == args.apk_sha256
    directory = ROOT / '.build/android/execution' / str(uuid.uuid4())
    directory.mkdir(parents=True)
    operator = ROOT / '.build/android/public' / str(args.operator_run_id)
    remote = '/workspace/pixels-command-' + directory.name
    result = {'status': 'failed', 'serial': args.serial, 'apk_sha256': args.apk_sha256,
        'mcp_sha256': hashlib.sha256(args.mcp_executable.read_bytes()).hexdigest(),
        'forced_relay': True, 'public_server': True, 'checks': [], 'remote_root': remote,
        'cleanup_pending': False, 'path_probe': True, 'full_android_acceptance': False}
    client = None
    home = Home(args.serial)
    terminals = set()
    tasks = set()
    reconnect_number = 0

    def connect():
        nonlocal client, reconnect_number
        if client is not None:
            call('pab_disconnect')
            client.close()
        reconnect_number += 1
        client = PublicMcp(operator / 'mcp.toml', args.device_code, True,
            directory / f'mcp-{reconnect_number}.log', args.mcp_executable)
        value, raw = client.connect()
        assert value.get('connected') and not raw.get('isError'), value
        assert value['target']['execution']['os_family'] == 'android'

    def call(name, fields=None, *, error=False, device=True):
        fields = dict(fields or {})
        if device: fields['device_code'] = args.device_code
        value, raw = client.tool(name, fields, allow_error=True)
        with (directory / 'trace.jsonl').open('a', encoding='utf-8') as out:
            out.write(json.dumps({'tool': name, 'fields': fields, 'value': value,
                'is_error': bool(raw.get('isError')), 'time': time.time()}, ensure_ascii=False) + '\n')
        if not error: assert not raw.get('isError'), (name, value)
        return value, raw

    def passed(name, **metadata):
        result['checks'].append({'name': name, 'passed': True, **metadata})
        print(json.dumps({'check': name, 'passed': True}), flush=True)

    def file(name, fields):
        value, _ = call(name, fields)
        ref = value.get('operation_ref')
        deadline = time.monotonic() + 90
        while value.get('result', {}).get('state') == 'running':
            assert time.monotonic() < deadline, 'Only query the original operation'
            value, _ = call('pab_get_operation', {'operation_id': ref['operation_id']})
        assert value['result']['state'] == 'completed', value
        return value

    def wait_task(task_id):
        deadline = time.monotonic() + 45
        while True:
            value, _ = call('pab_get_task', {'task_id': task_id, 'wait_ms': 1000}, error=True)
            snapshot = value.get('snapshot') or value.get('task')
            if snapshot and snapshot['state'] in ('succeeded', 'failed', 'cancelled', 'interrupted') and value.get('complete'):
                tasks.discard(task_id)
                return snapshot
            assert time.monotonic() < deadline, 'Keep original task; do not rerun'
            time.sleep(.1)

    def command(script=None, *, wait=True, **options):
        fields = {'program': '/system/bin/sh', 'args': ['-c', script] if script is not None else [],
            'cwd': remote, 'request_id': str(uuid.uuid4()), 'wait_ms': 5000 if wait else 0, **options}
        value, raw = call('pab_run_command', fields, error=True)
        snapshot = value.get('task') or value.get('snapshot')
        if snapshot is None:
            assert raw.get('isError'), value
            return value, fields, None
        task_id = snapshot['task_ref']['task_id']
        tasks.add(task_id)
        snapshot = wait_task(task_id) if wait else snapshot
        return snapshot, fields, task_id

    def output(task_id, stream='stdout'):
        offset = 0
        data = b''
        while True:
            value, _ = call('pab_read_output', {'task_id': task_id, 'stream': stream,
                'offset': offset, 'max_bytes': 8192, 'include_base64': True})
            body = base64.b64decode(value['base64'])
            assert value['offset'] == offset, 'Output unexpectedly expired or was truncated'
            data += body
            offset = value['next_offset']
            if not body or value.get('complete') is True: return data

    def run_ok(script, **options):
        snapshot, fields, task = command(script, **options)
        assert task and snapshot['state'] == 'succeeded', snapshot
        assert snapshot['capability'] == {'name': 'process.exec', 'version': 2 if (
            fields.get('env') or fields.get('stdin_text') is not None or fields.get('timeout_ms') is not None) else 1}
        return output(task), fields, task

    def terminal_open():
        value, _ = call('pab_open_terminal', {'cols': 80, 'rows': 24})
        assert value['shell'] == '/system/bin/sh' and value['startup']['arguments'] == ['-i']
        assert value['execution_identity']['account_id'].startswith('uid:')
        terminals.add(value['session_id'])
        return value['session_id']

    def terminal_input(session, text):
        call('pab_terminal_input', {'session_id': session, 'data': text}, device=False)

    def terminal_until(session, marker):
        deadline = time.monotonic() + 12
        text = ''
        while time.monotonic() < deadline:
            value, _ = call('pab_terminal_read', {'session_id': session}, device=False)
            assert base64.b64decode(value['base64']).decode('utf-8', errors='replace') == value['text']
            text += value['text']
            if marker in text: return text
            assert not value['ended'], text
            time.sleep(.1)
        raise AssertionError('PTY did not compute expected output: ' + repr(text))

    def stopped(pid):
        assert pid.isdigit() and int(pid) > 1
        body, _, _ = run_ok(f"if [ -r /proc/{pid}/stat ]; then cat /proc/{pid}/stat; elif kill -0 {pid} 2>/dev/null; then printf 'ALIVE_UNREADABLE'; else printf 'GONE'; fi")
        # A zombie is no longer executing; the Android init reaper owns orphan reaping.
        assert body == b'GONE' or body.split(b')', 1)[1].lstrip().startswith(b'Z '), body
        return body.decode('utf-8', errors='replace')

    try:
        home.start(); home.set_paused(True); connect()
        catalog = client.rpc('tools/list', {})
        encoded = json.dumps(catalog, ensure_ascii=False, indent=2).encode('utf-8')
        (directory / 'catalog.json').write_bytes(encoded)
        result['catalog_sha256'] = hashlib.sha256(encoded).hexdigest()
        result['catalog_tools'] = len(catalog['tools'])
        for name, fields in [('pab_run_command', {'program': '/system/bin/sh', 'args': ['-c', 'id']}),
                             ('pab_open_terminal', {})]:
            denied, raw = call(name, fields, error=True)
            assert raw.get('isError') and 'control_paused' in json.dumps(denied), denied
        passed('normal_pause_pre_dispatch_gate')
        home.set_paused(False)
        file('pab_mkdir', {'path': remote, 'parents': False, 'exist_ok': False})
        result['cleanup_pending'] = True

        body, _, uid_task = run_ok("id; printf '中文😀\\n'; printf '错误流\\n' >&2; read -r line; printf 'INPUT:%s ENV:%s\\n' \"$line\" \"$PIXELS_COMMAND_QA\"",
            stdin_text='远程输入\n', env={'PIXELS_COMMAND_QA': '环境值'})
        assert '中文😀' in body.decode() and 'INPUT:远程输入 ENV:环境值' in body.decode(), body
        assert b'uid=' in body and b'uid=0(' not in body, body
        identity = body.splitlines()[0].decode()
        snapshot = wait_task(uid_task)
        result['execution_identity'] = snapshot['execution_context']['identity']
        assert ('uid=' + result['execution_identity']['account_id'].removeprefix('uid:') + '(') in identity
        _, _, last_task = command("printf '错误流\\n' >&2")
        assert output(last_task, 'stderr').decode() == '错误流\n'
        passed('actual_app_uid_unicode_env_stdin_and_separate_stderr', identity=identity)

        literal = '中文 ; $(printf SHOULD_NOT_RUN) *'
        body, _, _ = run_ok(None, args=['-c', 'printf %s "$1"', 'argv-zero', literal])
        assert body.decode() == literal
        passed('argv_is_not_shell_interpolated')

        body, _, _ = run_ok(None, program='id', args=[])
        assert body.splitlines()[0].decode() == identity
        failed, _, task = command(None, program='id', args=[], env={'PATH': '/pixels-qa-missing-path'})
        assert task and failed['state'] == 'failed' and failed['error']['code'] == 'process_start_failed', failed
        passed('bare_program_uses_command_environment_path')
        available, _, _ = run_ok('for name in sh id ls cat grep sed awk find stat sleep toybox curl python python3; do command -v "$name" || :; done')
        result['observed_programs'] = available.decode().splitlines()

        script = '#!/system/bin/sh\nprintf "SCRIPT:%s\\n" "$1"\nprintf "%s" "$1" > result.txt\n'
        file('pab_file_write', {'path': remote + '/脚本.sh', 'content': script})
        body, _, _ = run_ok(None, args=['脚本.sh', '共享文件中文'])
        assert body.decode() == 'SCRIPT:共享文件中文\n'
        assert file('pab_file_read', {'path': remote + '/result.txt'})['text'] == '共享文件中文'
        outside, _, no_task = command('pwd', cwd='/shared-not-a-posix-directory')
        assert no_task is None and 'command_cwd_requires_workspace' in json.dumps(outside)
        passed('workspace_script_and_file_tool_interoperability_saf_cwd_rejected')

        failed, _, task = command("printf 'failure\\n' >&2; exit 7")
        assert failed['state'] == 'failed' and failed['error']['exit_code'] == 7, failed
        assert output(task, 'stderr') == b'failure\n'
        failed, _, _ = command(None, program='/system/bin/pixels-does-not-exist', args=[])
        assert failed['error']['code'] == 'process_start_failed', failed
        passed('nonzero_exit_and_missing_executable')

        big, _, task = command("i=0; while [ \"$i\" -lt 3000 ]; do printf '中文😀-%s\\r\\n' \"$i\"; i=$((i+1)); done")
        assert big['state'] == 'succeeded', big
        expected = ''.join(f'中文😀-{i}\r\n' for i in range(3000)).encode()
        assert output(task) == expected
        passed('multi_chunk_unicode_output_offsets_exact_bytes', bytes=len(expected))

        occupied = []
        for _ in range(4):
            _, request, task = command('sleep 120', wait=False)
            assert task is not None
            occupied.append((request, task))
        denied, _, extra = command('printf SHOULD_NOT_START', wait=False)
        assert extra is None and 'command_busy' in json.dumps(denied), denied
        call('pab_system_info')
        for request, task in occupied:
            call('pab_cancel_operation', {'operation_id': request['request_id']})
            assert wait_task(task)['state'] == 'cancelled'
        passed('four_job_budget_and_system_lane_remains_responsive')

        timed, _, _ = command('sleep 120 & printf %s "$!" > timeout.pid; wait', timeout_ms=600)
        assert timed['state'] == 'failed' and timed['error']['code'] == 'execution_timeout', timed
        timeout_pid = file('pab_file_read', {'path': remote + '/timeout.pid'})['text']
        stopped(timeout_pid)
        passed('deadline_stops_command_process_group', child_pid=timeout_pid)

        running, request, task = command('sleep 120 & printf %s "$!" > cancel.pid; wait', wait=False)
        deadline = time.monotonic() + 8
        while True:
            value, _ = call('pab_file_read', {'path': remote + '/cancel.pid'}, error=True)
            if value.get('text'): break
            assert time.monotonic() < deadline
            time.sleep(.1)
        call('pab_cancel_operation', {'operation_id': request['request_id']})
        assert wait_task(task)['state'] == 'cancelled'
        stopped(value['text'])
        passed('explicit_cancel_original_task_and_child_cleanup', task_id=task)

        _, original, task = command('printf x >> once.txt; sleep 2; printf done', wait=False)
        connect()
        duplicate, _ = call('pab_run_command', original)
        duplicate_task = (duplicate.get('task') or duplicate.get('snapshot'))['task_ref']['task_id']
        assert duplicate_task == task
        assert wait_task(task)['state'] == 'succeeded'
        assert file('pab_file_read', {'path': remote + '/once.txt'})['text'] == 'x'
        conflict, raw = call('pab_run_command', {**original, 'args': ['-c', 'printf BAD >> once.txt']}, error=True)
        assert raw.get('isError') and 'different' in json.dumps(conflict).lower(), conflict
        passed('reconnect_dedup_and_parameter_conflict_no_replay', task_id=task, request_id=original['request_id'])

        first = terminal_open()
        terminal_input(first, "test -t 0 && test -t 1 && printf 'TTY_%s\\n' \"$((6*7))\"\n")
        terminal_until(first, 'TTY_42')
        second = terminal_open()
        terminal_input(second, "printf 'SECOND_%s\\n' \"$((7*7))\"\n")
        terminal_until(second, 'SECOND_49')
        denied, raw = call('pab_open_terminal', {}, error=True)
        assert raw.get('isError') and 'terminal_capacity' in json.dumps(denied), denied
        passed('real_tty_and_two_independent_sessions_capacity')
        call('pab_terminal_resize', {'session_id': first, 'cols': 100, 'rows': 35}, device=False)
        terminal_input(first, "stty size; printf 'SIZE_%s\\n' \"$((10*10))\"\n")
        text = terminal_until(first, 'SIZE_100')
        assert '35 100' in text, text
        terminal_input(first, 'cd ' + remote.removeprefix('/workspace/') + "; QA_VALUE=中文; printf '%s' \"$QA_VALUE\" > pty.txt; printf 'STATE_%s\\n' \"$((8*8))\"\n")
        terminal_until(first, 'STATE_64')
        terminal_input(first, "printf '%s:%s\\n' \"$QA_VALUE\" \"$((9*9))\"\n")
        terminal_until(first, '中文:81')
        assert file('pab_file_read', {'path': remote + '/pty.txt'})['text'] == '中文'
        passed('terminal_resize_actual_stty_and_persistent_cwd_variables')

        terminal_input(first, "sleep 120 & printf '%s' \"$!\" > pty-background.pid; printf 'BG_%s\\n' \"$((11*11))\"; sleep 120\n")
        terminal_until(first, 'BG_121')
        background = file('pab_file_read', {'path': remote + '/pty-background.pid'})['text']
        call('pab_terminal_close', {'session_id': first}, device=False)
        terminals.discard(first)
        stopped(background)
        terminal_input(second, "printf 'ALIVE_%s\\n' \"$((12*12))\"\n")
        terminal_until(second, 'ALIVE_144')
        call('pab_terminal_close', {'session_id': second}, device=False)
        terminals.discard(second)
        passed('terminal_close_cleans_background_job_other_session_survives')

        third = terminal_open()
        terminal_input(third, "sleep 120 & printf %s \"$!\" > " + remote.removeprefix('/workspace/') + "/disconnect.pid; printf 'DISC_%s\\n' \"$((13*13))\"\n")
        terminal_until(third, 'DISC_169')
        child = file('pab_file_read', {'path': remote + '/disconnect.pid'})['text']
        connect(); terminals.discard(third)
        time.sleep(.3); stopped(child)
        passed('disconnect_closes_connection_owned_terminal_without_reopen')

        if args.restart_app:
            from android_accessibility import set_enabled
            _, original, task = command('printf x >> restart-once.txt; printf %s "$$" > restart.pid; sleep 120; printf SHOULD_NOT_RUN', wait=False)
            deadline = time.monotonic() + 8
            while True:
                value, _ = call('pab_file_read', {'path': remote + '/restart.pid'}, error=True)
                if value.get('text'): break
                assert time.monotonic() < deadline
                time.sleep(.1)
            old_child = value['text']
            old_main = normal_status(args.serial)['pid']
            assert file('pab_file_read', {'path': remote + '/restart-once.txt'})['text'] == 'x'
            call('pab_disconnect'); client.close(); client = None
            # Preserve the real app data and account. Only the owned process is
            # faulted; accessibility is restored through normal system settings.
            set_enabled(args.serial, False)
            adb(args.serial, 'shell', 'am', 'force-stop', PACKAGE)
            adb(args.serial, 'shell', 'am', 'start', '-f', '0x14000000', '-n', f'{PACKAGE}/com.pixels.agent.MainActivity')
            set_enabled(args.serial, True)
            home.start(); connect()
            assert normal_status(args.serial)['pid'] != old_main
            original_result, _ = call('pab_run_command', original, error=True)
            recovered = original_result.get('task') or original_result.get('snapshot')
            assert recovered and recovered['task_ref']['task_id'] == task, original_result
            # wait_ms=0 may return the MCP's historical Running snapshot before
            # its follower reconciles the restarted device. Query the same task.
            assert wait_task(task)['state'] == 'interrupted'
            assert file('pab_file_read', {'path': remote + '/restart-once.txt'})['text'] == 'x'
            stopped(old_child)
            passed('actual_emulator_app_restart_keeps_original_fact_without_replay', old_main_pid=old_main, task_id=task, request_id=original['request_id'])

        _, _, task = command('sleep 120; printf SHOULD_NOT_COMPLETE', wait=False)
        fourth = terminal_open()
        home.start(); home.set_paused(True)
        assert wait_task(task)['state'] == 'cancelled'
        denied, raw = call('pab_terminal_read', {'session_id': fourth}, error=True, device=False)
        assert raw.get('isError'), denied
        terminals.discard(fourth)
        home.set_paused(False)
        run_ok("printf 'RECOVERED'")
        passed('pause_stops_active_work_and_fresh_work_recovers')
        result['status'] = 'passed'
    finally:
        if client is not None:
            for task in list(tasks):
                # Query original task; accepted work is never resubmitted by cleanup.
                try:
                    value, _ = call('pab_get_task', {'task_id': task}, error=True)
                    snapshot = value.get('snapshot') or value.get('task')
                    if snapshot and not snapshot['state'] in ('succeeded','failed','cancelled','interrupted'):
                        call('pab_cancel_operation', {'operation_id': snapshot['request_id']}, error=True)
                except Exception as error:
                    result.setdefault('cleanup_errors', []).append(type(error).__name__)
            for session in list(terminals):
                try: call('pab_terminal_close', {'session_id': session}, error=True, device=False)
                except Exception as error: result.setdefault('cleanup_errors', []).append(type(error).__name__)
            if result['cleanup_pending']:
                try:
                    home.start(); home.set_paused(False)
                    file('pab_file_delete', {'path': remote, 'recursive': True})
                    result['cleanup_pending'] = False
                except Exception as error: result.setdefault('cleanup_errors', []).append(type(error).__name__)
            try: call('pab_disconnect')
            except Exception as error: result.setdefault('cleanup_errors', []).append(type(error).__name__)
            client.close()
        try:
            home.start(); home.set_paused(True)
            state = normal_status(args.serial)
            result['final_state'] = {key: state.get(key) for key in ('phase','remote_ready','paused','accessibility_ready')}
        except Exception as error:
            result.setdefault('cleanup_errors', []).append(type(error).__name__)
        if result.get('cleanup_errors') or result['cleanup_pending']: result['status'] = 'failed'
        (directory / 'result.json').write_text(json.dumps(result, indent=2, ensure_ascii=False), encoding='utf-8')
        print(json.dumps({'directory': str(directory), 'status': result['status']}), flush=True)


if __name__ == '__main__':
    main()
