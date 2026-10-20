#!/usr/bin/env python3
"""Collect real 8h phone telemetry and own-package ANR/crash events while charging.

Only the normal ConnectionService owns the connection. This host monitor starts the
existing debug resource sampler and turns the screen off; it neither unlocks the
phone nor grants permissions. Manual recovery and resource review remain pending.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import threading
import time
import uuid

from analyze_android_soak import analyze
from android_home import normal_status
from android_public import PublicMcp
from android_scenarios import AndroidTasks
from test_android_remote import ROOT, PACKAGE, adb, installed_apk_sha256


def health_event(line, pid, phone_start):
    match = re.match(r'^\s*(\d+\.\d+)\s+(\d+)\s+\d+\s+\w\s+([^: ]+)\s*:', line)
    if not match or float(match[1]) < phone_start:
        return None
    timestamp, event_pid, tag = float(match[1]), int(match[2]), match[3]
    category = None
    if tag == 'am_anr' and re.search(r'(?<![\w.])' + re.escape(PACKAGE) + r'(?![\w.])', line):
        category = 'anr'
    elif event_pid == pid and tag == 'AndroidRuntime' and 'FATAL EXCEPTION' in line:
        category = 'java_crash'
    elif event_pid == pid and tag == 'libc' and 'Fatal signal' in line:
        category = 'native_crash'
    if category:
        return {'timestamp_unix':timestamp, 'pid':event_pid, 'category':category,
            'source_line_sha256':hashlib.sha256(line.encode()).hexdigest()}
    return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--run-id', required=True, type=uuid.UUID)
    parser.add_argument('--public-run-id', required=True, type=uuid.UUID)
    parser.add_argument('--seconds', type=int, default=28890)
    parser.add_argument('--setup-check', action='store_true',
        help='At most five minutes, leave screen on, never contribute formal 8h evidence')
    args = parser.parse_args()
    assert 60 <= args.seconds <= 86400
    assert not args.setup_check or args.seconds <= 300
    awake = None
    if os.name == 'nt':
        import ctypes
        awake = ctypes.windll.kernel32.SetThreadExecutionState
        awake.argtypes = [ctypes.c_uint]
        awake.restype = ctypes.c_uint
        assert awake(0x80000001), 'Could not keep the diagnostic host awake'
    directory = ROOT / '.build/android/acceptance' / str(args.run_id)
    directory.mkdir()
    public = ROOT / '.build/android/public' / str(args.public_run_id)
    session = json.loads((public / 'session.json').read_text(encoding='utf-8'))
    digest = installed_apk_sha256(args.serial)
    assert digest == session['apk_sha256']
    assert not adb(args.serial, 'reverse', '--list').strip()
    status = normal_status(args.serial)
    assert status['remote_ready'] and status['accessibility_ready'] and not status['paused']
    result = {'status':'failed', 'provenance':{'apk_sha256':digest, 'serial':args.serial,
        'public_server':True}, 'run_id':str(args.run_id), 'requested_seconds':args.seconds,
        'started_at_unix':time.time(), 'original_status':status, 'final_acceptance':False,
        'charging_test':True, 'manual_unlock_pending':not args.setup_check, 'resource_review_pending':True,
        'setup_check_only':args.setup_check,
        'diagnostics':'USB attached; filtered logcat; operator kept awake; no phone wake lock'}
    client = PublicMcp(public / 'mcp.toml', session['device_code'], True,
        directory / 'baseline-mcp.log', public / 'pab-mcp.exe')
    try:
        connected, raw = client.connect()
        assert connected['connected'] and not raw.get('isError')
        tasks = AndroidTasks(client, PACKAGE, directory / 'baseline-operation-trace.jsonl')
        tasks.fixture()
        before = tasks.count()
        marker = '后台采样基线-' + args.run_id.hex[:8]
        tasks.act('fixture_input', 'set_text', text=marker)
        tasks.act('fixture_save')
        tasks.wait(marker)
        assert tasks.count() == before + 1
        result['baseline'] = {'save_count':before + 1, 'original_request_id':tasks.last_input_request,
            'device_ref':connected['device_ref']}
    finally:
        client.close()
    (directory / 'baseline.json').write_text(json.dumps(result, indent=2), encoding='utf-8')

    pid = status['pid']
    phone_start = float(adb(args.serial, 'shell', 'date', '+%s').strip())
    exit_before = adb(args.serial, 'shell', 'dumpsys', 'activity', 'exit-info', PACKAGE)
    (directory / 'exit-info-before.txt').write_text(exit_before, encoding='utf-8')
    events = []
    stderr = (directory / 'logcat-stderr.log').open('w', encoding='utf-8')
    stream = subprocess.Popen(['adb', '-s', args.serial, 'logcat', '-v', 'epoch',
        '-b', 'events', '-b', 'crash', '-s', 'am_anr', 'AndroidRuntime', 'libc'],
        stdout=subprocess.PIPE, stderr=stderr, text=True, encoding='utf-8', errors='replace')

    def read_events():
        for line in stream.stdout:
            event = health_event(line, pid, phone_start)
            if event:
                events.append(event)
                with (directory / 'health-events.jsonl').open('a', encoding='utf-8') as output:
                    output.write(json.dumps(event) + '\n')

    reader = threading.Thread(target=read_events, daemon=True)
    reader.start()
    sampler_started = False
    sampling_start = time.monotonic()
    try:
        adb(args.serial, 'shell', 'am', 'broadcast', '-n',
            f'{PACKAGE}/com.pixels.agent.fixture.ProductionProbeReceiver',
            '--es', 'production_request_id', str(args.run_id), '--es', 'probe_action', 'soak-start')
        sampler_started = True
        deadline = time.monotonic() + 5
        while True:
            text = adb(args.serial, 'shell', 'run-as', PACKAGE, 'cat', f'files/soak-{args.run_id}.jsonl', check=False)
            try:
                initial = [json.loads(line) for line in text.splitlines()]
                if len(initial) >= 2 and initial[0]['apk_sha256'] == digest:
                    assert initial[0]['pid'] == pid
                    assert initial[1]['plugged'] > 0, 'Record standby charging explicitly'
                    break
            except (ValueError, KeyError):
                pass
            assert time.monotonic() < deadline, 'Resource sampler did not produce real telemetry'
            time.sleep(.2)
        if not args.setup_check:
            adb(args.serial, 'shell', 'input', 'keyevent', 'KEYCODE_SLEEP')
        result['screen_off_requested'] = not args.setup_check
        while True:
            assert stream.poll() is None, 'The ANR/crash diagnostic stream ended early'
            elapsed = time.monotonic() - sampling_start
            progress = {'phase':'sampling', 'elapsed_seconds':round(elapsed, 2),
                'requested_seconds':args.seconds, 'health_event_count':len(events),
                'apk_sha256':digest, 'manual_unlock_pending':not args.setup_check}
            (directory / 'progress.json').write_text(json.dumps(progress, indent=2), encoding='utf-8')
            print(json.dumps(progress), flush=True)
            if elapsed >= args.seconds:
                break
            time.sleep(min(60, args.seconds - elapsed))
        result['status'] = 'sampling_complete'
    finally:
        passive_pid = adb(args.serial, 'shell', 'pidof', PACKAGE, check=False).strip()
        if sampler_started and passive_pid == str(pid):
            adb(args.serial, 'shell', 'am', 'broadcast', '-n',
                f'{PACKAGE}/com.pixels.agent.fixture.ProductionProbeReceiver',
                '--es', 'production_request_id', str(args.run_id), '--es', 'probe_action', 'soak-stop', check=False)
            time.sleep(.3)
        log_stream_alive = stream.poll() is None
        stream.terminate()
        try:
            stream.wait(timeout=5)
        except subprocess.TimeoutExpired:
            stream.kill()
            stream.wait(timeout=5)
        reader.join(timeout=2)
        stderr.close()
        raw = adb(args.serial, 'shell', 'run-as', PACKAGE, 'cat', f'files/soak-{args.run_id}.jsonl', check=False)
        (directory / 'phone-soak.jsonl').write_text(raw, encoding='utf-8')
        try:
            analysis = analyze([json.loads(line) for line in raw.splitlines()])
        except (ValueError, KeyError, TypeError):
            analysis = {'telemetry_valid':False, 'errors':['malformed_telemetry']}
        if analysis.get('apk_sha256') != digest or installed_apk_sha256(args.serial) != digest:
            analysis['telemetry_valid'] = False
            analysis.setdefault('errors', []).append('apk_changed')
        (directory / 'soak-analysis.json').write_text(json.dumps(analysis, indent=2), encoding='utf-8')
        exit_after = adb(args.serial, 'shell', 'dumpsys', 'activity', 'exit-info', PACKAGE)
        (directory / 'exit-info-after.txt').write_text(exit_after, encoding='utf-8')
        result['health'] = {'pid_unchanged':passive_pid == str(pid),
            'diagnostic_stream_complete':log_stream_alive,
            'anr_count':sum(event['category'] == 'anr' for event in events),
            'crash_count':sum(event['category'] != 'anr' for event in events),
            'exit_history_reviewed':False}
        result['telemetry_valid'] = analysis.get('telemetry_valid') is True
        if args.setup_check and analysis.get('errors') == ['less_than_eight_hours']:
            result['status'] = 'setup_check_passed'
            result['manual_unlock_pending'] = False
        result['finished_at_unix'] = time.time()
        (directory / 'monitor-result.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
        if awake:
            awake(0x80000000)
    return 0 if (result['status'] == 'sampling_complete' and result['telemetry_valid']
        or args.setup_check and result['status'] == 'setup_check_passed') else 1


if __name__ == '__main__':
    raise SystemExit(main())
