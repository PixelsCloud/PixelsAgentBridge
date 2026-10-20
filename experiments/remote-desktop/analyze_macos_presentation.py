"""Audit saved compositor observations without changing their original verdict.

Only generated frame codes and timing metadata are read. Frame-code skips are
not filled in: every counted transition must have been independently observed.
The observer's capture clock and the Python receipt clock are never subtracted
from one another. This is not a latency measurement or a loss-rate estimate.
"""
import argparse
import hashlib
import json
import math
from pathlib import Path


def analyze(report):
    duration = report['seconds']
    warmup = report['warmup_seconds']
    assert math.isfinite(duration) and 0 <= warmup < duration
    observations = report['observations']
    assert len(observations) == report['samples'] and len(observations) >= 2
    events = []
    invalid = repeated = 0
    previous_receipt = previous_capture = previous_code = None
    for receipt, captured, code in observations:
        assert math.isfinite(receipt) and 0 <= receipt <= duration
        assert type(captured) is int and type(code) is int and -1 <= code < 4096
        if previous_receipt is not None:
            assert receipt > previous_receipt and captured > previous_capture
        previous_receipt, previous_capture = receipt, captured
        if code == -1:
            invalid += 1
        elif code == previous_code:
            repeated += 1
        else:
            if previous_code is not None:
                assert 0 < (code - previous_code) % 4096 < 2048
            events.append([receipt, code])
            previous_code = code
    assert events == report['events']
    assert len(events) == report['unique_presentations']
    assert invalid == report['invalid_samples'] and repeated == report['repeated_samples']
    steady = [receipt for receipt, _ in events if receipt >= warmup]
    rate = len(steady) / (duration - warmup)
    times = [warmup] + steady + [duration]
    gap = max(b - a for a, b in zip(times, times[1:]))
    assert math.isclose(rate, report['steady_fps'], rel_tol=1e-12)
    assert math.isclose(gap, report['longest_presentation_gap_seconds'], rel_tol=1e-12)
    checks = {
        'steady_receipt_rate_above_12': rate > 12,
        'invalid_samples_at_most_5_percent': invalid <= len(observations) * .05,
        'observer_receipt_rate_at_least_30': len(observations) / duration >= 30,
    }
    assert type(report['passed']) is bool and report['passed'] == all(checks.values())
    capture_span = (observations[-1][1] - observations[0][1]) / 1_000_000
    # Count observed transitions, not all frames implied by the source counter.
    # Include startup/tail time and use the larger of the two measured intervals.
    interval = max(duration, capture_span)
    return {
        'original_passed': report['passed'], 'original_checks': checks,
        'failed_original_checks': [name for name, value in checks.items() if not value],
        'seconds': duration, 'capture_span_seconds': capture_span,
        'samples': len(observations), 'observer_receipt_hz': len(observations) / duration,
        'invalid_samples': invalid, 'repeated_samples': repeated,
        'unique_observed_codes': len(events), 'steady_receipt_fps': rate,
        'observed_transitions_per_second': max(0, len(events) - 1) / interval,
        'longest_observed_gap_seconds': gap,
        'limitations': 'Sampled composed images under observer load; no exact refresh, end-to-end latency, or unobserved-frame loss claim.',
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('reports', type=Path, nargs='+')
    args = parser.parse_args()
    for path in args.reports:
        raw = path.read_bytes()
        print(json.dumps({'file': path.name, 'sha256': hashlib.sha256(raw).hexdigest(),
                          **analyze(json.loads(raw))}, ensure_ascii=False))


if __name__ == '__main__':
    main()
