"""Windowed diagnostics for a completed native remote-desktop soak run."""
import argparse
import json
from pathlib import Path

try:
    from .remote_desktop_stability import summarize, performance_failures, percentile
except ImportError:
    from remote_desktop_stability import summarize, performance_failures, percentile


def windows(events, warmup_seconds=5, window_seconds=60):
    """Assign gaps to their ending window, including gaps crossing a boundary.

    Each window's rate uses its entire scheduled span, rather than the distance
    between its first/last notifications. This retains leading/trailing stalls.
    """
    opened = next((e for e in events if e.get('event') == 'opened'), {})
    duration = opened.get('seconds', 0)
    if not isinstance(duration, (float, int)) or duration <= warmup_seconds:
        raise ValueError('opened event needs a duration greater than warmup')
    count = int((duration - warmup_seconds + window_seconds - 1) // window_seconds)
    rows = []
    for i in range(count):
        start = warmup_seconds + i * window_seconds
        end = min(start + window_seconds, duration)
        rows.append(dict(start_s=start, end_s=end, frames=0, gaps_ms=[],
                         udp_samples=0, udp_completed=0, udp_rtt_ms=[]))
    previous = None
    for event in events:
        elapsed = event.get('elapsed_us')
        if not isinstance(elapsed, (int, float)):
            continue
        seconds = elapsed / 1e6
        if event.get('event') == 'presented':
            gap = None if previous is None else (elapsed - previous) / 1000
            previous = elapsed
        else:
            gap = None
        if not warmup_seconds <= seconds < duration:
            continue
        row = rows[min(int((seconds - warmup_seconds) // window_seconds), count - 1)]
        if event.get('event') == 'presented':
            row['frames'] += 1
            if gap is not None:
                row['gaps_ms'].append(gap)
        if event.get('event') == 'network_probe':
            row['udp_samples'] += 1
            rtt = event.get('rtt_us')
            if isinstance(rtt, (int, float)):
                row['udp_completed'] += 1
                row['udp_rtt_ms'].append(rtt / 1000)
    for row in rows:
        gaps = row.pop('gaps_ms')
        rtts = row.pop('udp_rtt_ms')
        row.update(fps=row['frames'] / (row['end_s'] - row['start_s']),
                   gap_p95_ms=percentile(gaps, .95), max_gap_ms=max(gaps, default=None),
                   gaps_over_100ms=sum(g > 100 for g in gaps),
                   gaps_over_1s=sum(g > 1000 for g in gaps),
                   udp_rtt_p95_ms=percentile(rtts, .95),
                   udp_max_rtt_ms=max(rtts, default=None))
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('input', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--minimum-seconds', type=int, default=1800)
    args = parser.parse_args()
    if args.minimum_seconds < 30 or args.minimum_seconds > 3600:
        parser.error('minimum seconds must be within 30..3600')
    summary = summarize(args.input)
    failures = performance_failures(summary)
    declared = (summary.get('opened') or {}).get('seconds', 0)
    # Allow only warmup and a one-second endpoint allowance, not a truncated run.
    if declared < args.minimum_seconds or summary['measured_present_span_seconds'] < args.minimum_seconds - 6:
        failures.append('insufficient soak duration')
    raw = args.input.read_bytes()
    encoding = 'utf-16' if raw.startswith((b'\xff\xfe', b'\xfe\xff')) else 'utf-8-sig'
    events = [json.loads(line) for line in raw.decode(encoding).splitlines() if line.strip()]
    rows = windows(events)
    summary.update(status='failed' if failures else 'passed', failures=failures,
                   windows=rows, minimum_seconds=args.minimum_seconds,
                   note='Window rates are diagnostic; the unchanged performance gate uses whole-run FPS, gaps and native observations. Native presentation does not prove panel scanout or image quality.')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(summary, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(dict(status=summary['status'], failures=failures,
                          fps=summary['presentation_notification_fps'],
                          worst_window_fps=min(r['fps'] for r in rows))))
    raise SystemExit(int(bool(failures)))


if __name__ == '__main__':
    main()
