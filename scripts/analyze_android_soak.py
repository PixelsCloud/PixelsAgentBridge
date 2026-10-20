#!/usr/bin/env python3
"""Fetch and analyze debug resource telemetry without treating short or gapped runs as 8h passes."""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import statistics
import uuid

from test_android_remote import ROOT, PACKAGE, adb, installed_apk_sha256


def analyze(events):
    headers = [value for value in events if value.get('type') == 'start']
    samples = [value for value in events if value.get('type') == 'sample']
    errors = []
    if len(headers) != 1 or len(samples) < 2:
        return {'telemetry_valid':False, 'errors':['missing_unique_start_or_samples']}
    header = headers[0]
    if any(value.get('type') in ('sampling_failed', 'capacity_reached') for value in events):
        errors.append('sampling_failed_or_capacity_reached')
    if [value.get('index') for value in samples] != list(range(len(samples))):
        errors.append('missing_or_repeated_sample_index')
    if any(value.get('pid') != header['pid'] for value in samples):
        errors.append('process_changed')
    gaps = [(right['elapsed_ms'] - left['elapsed_ms']) / 1000 for left, right in zip(samples, samples[1:])]
    if any(gap <= 0 or gap > 150 for gap in gaps):
        errors.append('non_monotonic_or_incomplete_sampling')
    duration = (samples[-1]['elapsed_ms'] - samples[0]['elapsed_ms']) / 1000
    if duration < 28800:
        errors.append('less_than_eight_hours')
    if samples[0]['elapsed_ms'] - header['elapsed_ms'] > 150000:
        errors.append('initial_sample_delayed')
    resource_fields = ('pss_kib', 'native_heap_bytes', 'fd_count', 'thread_count', 'uid_rx_bytes', 'uid_tx_bytes')
    if any(type(value.get(field)) is not int or value[field] < 0 for value in samples for field in resource_fields):
        errors.append('resource_measurement_unavailable')
    if any(value.get('battery_scale', -1) <= 0 or value.get('battery_level', -1) < 0
           or value.get('plugged', -1) < 0 or value.get('temperature_tenths_c', -1) == -1 for value in samples):
        errors.append('battery_or_temperature_measurement_unavailable')
    segments = []
    for number in range(4):
        bucket = [value for value in samples if min(3, int((value['elapsed_ms'] - samples[0]['elapsed_ms']) / max(1, duration * 1000) * 4)) == number]
        metrics = {}
        for field in resource_fields[:4]:
            values = [value[field] for value in bucket if type(value.get(field)) is int and value[field] >= 0]
            if values:
                metrics[field] = {'min':min(values), 'median':statistics.median(values), 'max':max(values)}
        segments.append({'quarter':number + 1, 'samples':len(bucket), 'metrics':metrics})
    return {'telemetry_valid':not errors, 'errors':errors, 'apk_sha256':header['apk_sha256'],
        'request_id':header['request_id'], 'continuous_seconds':duration, 'samples':len(samples),
        'maximum_sample_gap_seconds':max(gaps), 'segments':segments,
        'charging_samples':sum(value.get('plugged', 0) > 0 for value in samples),
        'remote_not_ready_samples':sum(value.get('remote_ready') is not True for value in samples),
        'temperature_tenths_c':{'min':min(value.get('temperature_tenths_c', -1) for value in samples),
                              'max':max(value.get('temperature_tenths_c', -1) for value in samples)},
        'uid_rx_delta_bytes':samples[-1]['uid_rx_bytes'] - samples[0]['uid_rx_bytes'],
        'uid_tx_delta_bytes':samples[-1]['uid_tx_bytes'] - samples[0]['uid_tx_bytes'],
        'resource_trend_review_required':True, 'final_acceptance':False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--run-id', required=True, type=uuid.UUID)
    options = parser.parse_args()
    root = ROOT / '.build/android/acceptance' / str(options.run_id)
    root.mkdir(parents=True, exist_ok=True)
    source = root / 'phone-soak.jsonl'
    source.write_text(adb(options.serial, 'shell', 'run-as', PACKAGE, 'cat',
        f'files/soak-{options.run_id}.jsonl'), encoding='utf-8')
    try:
        events = [json.loads(line) for line in source.read_text(encoding='utf-8').splitlines()]
        report = analyze(events)
    except (KeyError, ValueError, TypeError):
        report = {'telemetry_valid':False, 'errors':['malformed_or_incomplete_telemetry']}
    apk = ROOT / 'apps/android/app/build/outputs/apk/debug/app-debug.apk'
    digest = hashlib.sha256(apk.read_bytes()).hexdigest()
    if report.get('apk_sha256') != digest or installed_apk_sha256(options.serial) != digest:
        report['telemetry_valid'] = False
        report.setdefault('errors', []).append('apk_changed')
    (root / 'soak-analysis.json').write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(report))
    return 0 if report['telemetry_valid'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
