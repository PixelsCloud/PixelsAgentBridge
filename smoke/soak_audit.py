"""Audit the real soak journal, including restarts and missing observation time.

The optional bounded watcher only reads an existing soak run and writes audit
evidence. It never restarts devices, changes services or shortens the soak.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import time


def audit(report, samples, *, finished):
    failures = []
    required = report.get('required_seconds', 0)
    period = report.get('period_seconds', 60)
    codes = [d['code'] for d in report.get('devices', [])]
    if required < 28800 or not codes or len(set(codes)) != len(codes):
        failures.append('invalid_duration_or_device_inventory')
    if report.get('errors'):
        failures.append('driver_recorded_failed_observations')
    groups = {code: [] for code in codes}
    seen = set()
    for row in samples:
        code, cycle = row.get('device'), row.get('cycle')
        if code not in groups or (code, cycle) in seen:
            failures.append('unknown_device_or_duplicate_cycle')
            continue
        seen.add((code, cycle))
        groups[code].append(row)
        if row.get('passed') is not True:
            failures.append('failed_sample_preserved')
    observations = {}
    for code, rows in groups.items():
        if not rows:
            if finished:
                failures.append(code + ':missing_samples')
            continue
        times = [r['elapsed_seconds'] for r in rows]
        pids = sorted({r.get('executor_pid') for r in rows if r.get('executor_pid')})
        if len(pids) != 1 or any(not r.get('executor_pid') for r in rows):
            failures.append(code + ':executor_restarted_or_identity_missing')
        gaps = [b - a for a, b in zip(times, times[1:])]
        if any(gap <= 0 or gap > max(180, period * 3) for gap in gaps):
            failures.append(code + ':observation_gap')
        duration = times[-1] - times[0]
        observations[code] = {'samples': len(rows), 'executor_pids': pids,
            'observed_span_seconds': round(duration, 3), 'maximum_gap_seconds': max(gaps, default=0)}
        if finished and (duration < required - period or len(rows) < required / period * .9):
            failures.append(code + ':insufficient_observed_time')
    if finished:
        if report.get('status') != 'passed' or report.get('elapsed_seconds', 0) < required:
            failures.append('driver_not_passed_or_duration_incomplete')
        if len(samples) != report.get('samples'):
            failures.append('journal_count_differs_from_final_report')
        cleanup = report.get('cleanup', {})
        if not all(cleanup.get(code) is True for code in codes) or cleanup.get('owned_mcp_exited') is not True:
            failures.append('owned_session_cleanup_unconfirmed')
    return {'status': 'failed' if failures else ('passed' if finished else 'running'),
        'finished': finished, 'required_seconds': required, 'failures': sorted(set(failures)),
        'samples': len(samples), 'devices': observations}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--run', type=Path, required=True)
    parser.add_argument('--watch', action='store_true')
    parser.add_argument('--max-wait-seconds', type=int, default=33000)
    args = parser.parse_args()
    assert 1 <= args.max_wait_seconds <= 86400
    deadline = time.monotonic() + args.max_wait_seconds
    while True:
        try:
            finished = (args.run / 'result.json').is_file()
            report = json.loads((args.run / ('result.json' if finished else 'progress.json')).read_text(encoding='utf-8'))
            raw = (args.run / 'samples.jsonl').read_bytes()
            lines = raw.splitlines(keepends=True)
            # A running writer can still be completing its latest append.
            if lines and not lines[-1].endswith(b'\n') and not finished:
                lines.pop()
            rows = [json.loads(line) for line in lines]
            result = audit(report, rows, finished=finished)
        except (FileNotFoundError, json.JSONDecodeError, UnicodeError):
            if args.watch and time.monotonic() < deadline:
                time.sleep(2)
                continue
            raise
        temporary = args.run / 'audit.tmp.json'
        temporary.write_text(json.dumps(result, indent=2), encoding='utf-8')
        temporary.replace(args.run / 'audit.json')
        print(json.dumps({'audit': result['status'], 'finished': finished, 'samples': len(rows)}), flush=True)
        if result['status'] == 'failed':
            return 1
        if finished:
            return 0
        if not args.watch:
            return 2  # A partial observation can never be mistaken for a pass.
        if time.monotonic() >= deadline:
            result['status'] = 'failed'
            result['failures'].append('watcher_deadline_without_final_result')
            (args.run / 'audit.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
            return 1
        time.sleep(min(30, deadline - time.monotonic()))


if __name__ == '__main__':
    raise SystemExit(main())
