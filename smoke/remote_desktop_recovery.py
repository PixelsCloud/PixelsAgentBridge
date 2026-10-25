"""Audit recorded UDP impairment and subsequent native presentation recovery.

Injection bounds must both come from the viewer's monotonic trace. Host wall
clock times are retained separately and are never subtracted from viewer times.
"""
import argparse
import json
from pathlib import Path

try:
    from .remote_desktop_stability import summarize, percentile
except ImportError:
    from remote_desktop_stability import summarize, percentile


def audit(events, injections, expected_faults=3, recovery_seconds=20):
    presented = sorted((e for e in events if e.get('event') == 'presented'),
                       key=lambda e: e['elapsed_us'])
    pairs = [(a['elapsed_us']/1e6, b['elapsed_us']/1e6,
              (b['elapsed_us']-a['elapsed_us'])/1000)
             for a, b in zip(presented, presented[1:])]
    snapshots = [(e['elapsed_us']/1e6, e['trace']['network']) for e in events
                 if e.get('event') == 'frame_trace' and (e.get('trace') or {}).get('network')]
    failures = []
    rows = []
    if len(injections) != expected_faults:
        failures.append('wrong number of recorded injections')
    previous_end = 0
    for index, injection in enumerate(injections):
        result = injection.get('result') or {}
        start = injection.get('before_receiver_elapsed_s')
        end = injection.get('after_receiver_elapsed_s')
        requested = injection.get('requested_ms')
        errors = []
        if not all(isinstance(v, (int, float)) for v in (start, end, requested)) or not (0 <= start < end) or not (100 <= requested <= 3000):
            failures.append(f'fault {index+1}: invalid receiver-clock bounds or duration')
            continue
        if start < previous_end:
            errors.append('fault windows overlap')
        previous_end = end + recovery_seconds
        if not result.get('applied') or not result.get('rules_removed'):
            errors.append('injection or rule cleanup not confirmed')
        affected = [gap for a, b, gap in pairs if a <= end and b >= start]
        largest = max(affected, default=0)
        before = [s for at, s in snapshots if at <= start]
        after = [s for at, s in snapshots if end <= at <= end+5]
        loss_delta = None
        path_change = False
        if before and after:
            a, b = before[-1], after[-1]
            path_change = a.get('path_id') != b.get('path_id') or a.get('remote_addr') != b.get('remote_addr')
            if not path_change:
                delta = b.get('lost_packets', 0)-a.get('lost_packets', 0)
                loss_delta = delta if delta >= 0 else None
        # A rule-created success alone is not a proven interruption. Require
        # both a visible gap and transport loss detection/path change evidence.
        observed = largest >= max(80, requested*.5) and (path_change or (loss_delta or 0) > 0)
        if not observed:
            errors.append('impairment not substantiated by both presentation and transport evidence')
        recovered = [e for e in presented if end <= e['elapsed_us']/1e6 < end+recovery_seconds]
        rate = len(recovered)/recovery_seconds
        gaps = [gap for a, b, gap in pairs if end <= a and b < end+recovery_seconds]
        duration_complete = bool(presented and presented[-1]['elapsed_us']/1e6 >= end+recovery_seconds)
        if not duration_complete:
            errors.append('post-fault observation is too short')
        if rate < 28.5 or not gaps or max(gaps) > 250 or percentile(gaps, .95) > 67:
            errors.append('post-fault cadence did not meet normal performance thresholds')
        rows.append(dict(fault=index+1, requested_ms=requested,
                         before_receiver_s=start, after_receiver_s=end,
                         largest_observed_gap_ms=largest, sender_loss_delta=loss_delta,
                         selected_path_changed=path_change, impairment_observed=observed,
                         post_fault_fps=rate, post_fault_gap_p95_ms=percentile(gaps, .95),
                         post_fault_max_gap_ms=max(gaps, default=None), errors=errors))
        failures.extend(f'fault {index+1}: {message}' for message in errors)
    return dict(status='failed' if failures else 'passed', failures=failures, faults=rows,
                note='Bounds include remote-control call overhead, not exact blackout duration. Sender loss detection is not exact physical packet loss. Native continuity does not test the embedded App recovery or input route.')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('frames', type=Path)
    parser.add_argument('--injections', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--expected-faults', type=int, default=3)
    args = parser.parse_args()
    if not 1 <= args.expected_faults <= 10:
        parser.error('expected faults must be within 1..10')
    raw = args.frames.read_bytes()
    encoding = 'utf-16' if raw.startswith((b'\xff\xfe', b'\xfe\xff')) else 'utf-8-sig'
    events = [json.loads(line) for line in raw.decode(encoding).splitlines() if line.strip()]
    result = audit(events, json.loads(args.injections.read_text(encoding='utf-8-sig')), args.expected_faults)
    evidence = summarize(args.frames)
    if not (evidence['completed'] and evidence['measurement_complete'] and evidence['display_evidence_sufficient']):
        result['failures'].append('incomplete session, telemetry or native presentation evidence')
        result['status'] = 'failed'
    result['session_evidence'] = {k: evidence[k] for k in ('completed', 'measurement_complete', 'native_observation_ratio', 'errors', 'paths')}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2)+'\n', encoding='utf-8')
    print(json.dumps(result))
    raise SystemExit(int(result['status'] != 'passed'))


if __name__ == '__main__':
    main()
