"""Eight-hour native MCP resource soak on an explicitly configured device pool.

Uses an isolated, normally signed-in test operator and never installs software.
Samples are append-only. A failed operation is retained and prevents a pass;
reconnecting for later observations never erases an earlier failure.
"""
from __future__ import annotations

import argparse
import base64
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import time
import tomllib

import psutil

from mcp_client import McpClient


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--lab', type=Path, required=True)
    parser.add_argument('--config', type=Path, required=True)
    parser.add_argument('--mcp', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--seconds', type=int, default=28800)
    args = parser.parse_args()
    if args.seconds < 28800:
        parser.error('A release soak requires at least eight hours')
    lab = tomllib.loads(args.lab.read_text(encoding='utf-8-sig'))
    run = args.output.resolve()
    run.mkdir(parents=True, exist_ok=False)
    env = {k: v for k, v in os.environ.items() if not k.startswith('PAB_')}
    report = {'status': 'running', 'started_at': datetime.now(timezone.utc).isoformat(),
        'required_seconds': args.seconds, 'period_seconds': 60, 'warmup_seconds': 300,
        'thresholds': {'rss_growth_bytes': 128 * 1024 * 1024, 'handles_growth': 64, 'threads_growth': 16},
        'mcp_sha256': hashlib.sha256(args.mcp.read_bytes()).hexdigest(), 'errors': [],
        'devices': lab['device'], 'samples': 0, 'cleanup': {}}
    baselines = {}
    last = {}
    maxima = {}

    def save():
        temporary = run / 'progress.next'
        temporary.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
        temporary.replace(run / 'progress.json')

    def append(name, value):
        with (run / name).open('a', encoding='utf-8') as sink:
            sink.write(json.dumps(value, ensure_ascii=False) + '\n')

    client = McpClient(args.mcp.resolve(), run / 'mcp.log', config=args.config.resolve(), env=env)
    report['driver_pid'] = os.getpid()
    report['mcp_pid'] = client.process.pid
    driver = psutil.Process()
    started = time.monotonic()

    def connect(code):
        deadline = time.monotonic() + 55
        value = client.call('pab_connect', {'device_code': code, 'connection_mode': 'relay', 'wait_ms': 1000}, allow_error=True)
        while value.get('state') == 'connecting' and time.monotonic() < deadline:
            value = client.call('pab_connect', {'device_code': code, 'wait_ms': 1000}, allow_error=True)
        if not (value.get('connected') and value.get('connection_mode') == 'relay' and value.get('connection_path') == 'relay'):
            raise RuntimeError('Verified forced Relay connection unavailable')

    try:
        cycle = 0
        while True:
            cycle += 1
            cycle_started = time.monotonic()
            for device in lab['device']:
                code = device['code']
                device_started = time.monotonic()
                sample = {'cycle': cycle, 'device': code, 'at': datetime.now(timezone.utc).isoformat(),
                          'elapsed_seconds': round(time.monotonic() - started, 3)}
                try:
                    if cycle == 1 or cycle % 120 == 0:
                        if cycle > 1:
                            client.call('pab_disconnect', {'device_code': code})
                            sample['controlled_reconnect'] = True
                        connect(code)
                    info = client.observe(client.call('pab_system_info', {'device_code': code, 'sample_cpu': False}))
                    assert info['result']['state'] == 'completed'
                    process = info['result']['data']['info']['executor']
                    sample['executor_pid'] = process['pid']
                    sample['rss_bytes'] = process['memory_bytes']
                    assert sample['rss_bytes'] > 0
                    marker = f'PAB_SOAK_{cycle}_{code}'
                    if device['os'] == 'windows':
                        script = "$ErrorActionPreference='Stop';$p=Get-Process -Id " + str(process['pid']) + ";@{marker='" + marker + "';handles=$p.HandleCount;threads=$p.Threads.Count}|ConvertTo-Json -Compress"
                        program = r'C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe'
                        argv = ['-NoLogo', '-NoProfile', '-NonInteractive', '-EncodedCommand', base64.b64encode(script.encode('utf-16-le')).decode()]
                    else:
                        script = "import subprocess,json;pid=" + str(process['pid']) + ";fds=len(subprocess.check_output(['/usr/sbin/lsof','-n','-P','-p',str(pid)],text=True).splitlines())-1;threads=len(subprocess.check_output(['/bin/ps','-M','-p',str(pid)],text=True).splitlines())-1;print(json.dumps({'marker':" + repr(marker) + ",'handles':fds,'threads':threads}))"
                        program, argv = '/usr/bin/python3', ['-c', script]
                    accepted = client.call('pab_run_command', {'device_code': code, 'program': program,
                        'args': argv, 'wait_ms': 1000, 'timeout_ms': 15000})
                    value = client.observe(accepted)
                    assert (value.get('snapshot') or value.get('task'))['state'] == 'succeeded'
                    text = value.get('output', {}).get('stdout', {}).get('text')
                    if text is None:
                        text = client.call('pab_read_output', {'device_code': code, 'task_id': accepted['operation_ref']['task_id'], 'stream': 'stdout', 'max_bytes': 8192})['text']
                    metrics = json.loads(text)
                    assert metrics.pop('marker') == marker
                    assert metrics['threads'] > 0 and metrics['handles'] > 0
                    sample.update(metrics)
                    sample['latency_ms'] = round((time.monotonic() - device_started) * 1000)
                    sample['passed'] = True
                    if sample['elapsed_seconds'] >= 300:
                        baselines.setdefault(code, {k: sample[k] for k in ['rss_bytes', 'handles', 'threads']})
                        maxima.setdefault(code, dict(baselines[code]))
                        for key in baselines[code]:
                            maxima[code][key] = max(maxima[code][key], sample[key])
                    last[code] = sample
                except Exception as error:
                    sample['passed'] = False
                    sample['error_type'] = type(error).__name__
                    report['errors'].append({'cycle': cycle, 'device': code, 'error_type': type(error).__name__})
                    # Queries and the marker command are read-only. Recovery is
                    # observed on the next cycle; the failed sample remains.
                    try:
                        client.call('pab_disconnect', {'device_code': code}, allow_error=True)
                        connect(code)
                    except Exception:
                        pass
                append('samples.jsonl', sample)
                report['samples'] += 1
                save()
            report.update(elapsed_seconds=round(time.monotonic() - started, 3), cycle=cycle,
                          driver_rss_bytes=driver.memory_info().rss, baselines=baselines, resource_maxima=maxima)
            save()
            print(json.dumps({'cycle': cycle, 'elapsed_seconds': report['elapsed_seconds'], 'errors': len(report['errors'])}), flush=True)
            if time.monotonic() - started >= args.seconds:
                break
            time.sleep(min(max(0, 60 - (time.monotonic() - cycle_started)), args.seconds - (time.monotonic() - started)))
        assert not report['errors'], 'Failed observations remain in samples.jsonl'
        assert len(baselines) == len(lab['device']), 'Missing post-warmup baselines'
        for code, baseline in baselines.items():
            for key, limit in [('rss_bytes', 128 * 1024 * 1024), ('handles', 64), ('threads', 16)]:
                assert maxima[code][key] - baseline[key] <= limit, f'{code}: {key} growth exceeded threshold'
        report['status'] = 'passed'
    except BaseException as error:
        report['status'] = 'failed'
        report['fatal_error_type'] = type(error).__name__
        raise
    finally:
        for device in lab['device']:
            try:
                client.call('pab_disconnect', {'device_code': device['code']}, allow_error=True)
                report['cleanup'][device['code']] = True
            except Exception:
                report['cleanup'][device['code']] = False
        client.close()
        report['cleanup']['owned_mcp_exited'] = client.process.poll() is not None
        report['finished_at'] = datetime.now(timezone.utc).isoformat()
        save()
        (run / 'result.json').write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')


if __name__ == '__main__':
    main()
