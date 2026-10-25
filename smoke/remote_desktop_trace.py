"""Join per-frame metadata; compare endpoint intervals, never peer clock offsets."""
import argparse
import json
from pathlib import Path


def analyze(path, threshold_ms=100, network_errors_path=None):
    raw = path.read_bytes()
    events = [json.loads(line) for line in raw.decode(
        'utf-16' if raw.startswith((b'\xff\xfe', b'\xfe\xff')) else 'utf-8-sig').splitlines() if line.strip()]
    opened = next((e for e in events if e.get('event') == 'opened'), {})
    received = {e['frame']: e for e in events if e.get('event') == 'received'}
    submitted = {e['frame']: e for e in events if e.get('event') == 'submitted'}
    presented = [e for e in events if e.get('event') == 'presented' and e['elapsed_us'] >= 5_000_000]
    traces, polls, controls = {}, [], {}
    endpoints = set()
    udp = [e for e in events if e.get('event') == 'network_probe']
    # Join responder failures by echoed sequence, never by its independent clock.
    server_errors = [json.loads(line) for line in network_errors_path.read_text(encoding='utf-8-sig').splitlines()
                     if line.strip()] if network_errors_path else []
    errors_by_sequence = {e['sequence']: e for e in server_errors
                          if e.get('operation') == 'send' and e.get('sequence') is not None}
    scheduling = [e for e in events if e.get('event') in ('schedule_probe', 'runtime_probe')]

    def probes_between(left, right):
        # All these times originate on this receiver's same Instant clock.
        samples = [e for e in udp if e['send_us'] <= right and e['elapsed_us'] >= left]
        delays = {kind: [e.get('late_us', 0) / 1000 for e in scheduling
                        if e['event'] == kind and e['elapsed_us'] >= left
                        and e['elapsed_us'] - e.get('late_us', 0) <= right]
                  for kind in ('schedule_probe', 'runtime_probe')}
        good = [e for e in samples if e.get('status') == 'ok']
        return dict(samples=len(samples), completed=len(good),
            failures=sum(e.get('status') != 'ok' for e in samples),
            responder_send_error_codes=[errors_by_sequence[e['sequence']].get('winerror')
                or errors_by_sequence[e['sequence']].get('errno') for e in samples
                if e.get('sequence') in errors_by_sequence],
            max_rtt_ms=max((e['rtt_us']/1000 for e in good), default=None),
            max_server_work_ms=max((e['server_work_us']/1000 for e in good), default=None),
            max_thread_late_ms=max(delays['schedule_probe'], default=None),
            max_runtime_late_ms=max(delays['runtime_probe'], default=None))
    generation = opened.get('config', {}).get('generation', 1)
    for event in events:
        for control in event.get('receiver_control', []):
            if control['generation'] == generation:
                old = controls.setdefault(control['frame_id'], {})
                old.update({k: v for k, v in control.items() if v is not None})
        net = event.get('receiver_network') or {}
        if net.get('remote_addr'):
            endpoints.add((net['remote_addr'], net.get('local_addr')))
        if event.get('event') != 'frame_trace' or not event.get('trace'):
            continue
        polls.append(event)
        for frame in event['trace']['frames'] + event['trace']['slow_frames']:
            old = traces.setdefault(frame['frame_id'], {})
            old.update({k: v for k, v in frame.items() if v is not None})

    def loss_near(at):
        window = [e for e in polls if abs(e['elapsed_us'] - at) <= 1_000_000]
        networks = [e['trace']['network'] for e in window if e['trace'].get('network')]
        if len(networks) < 2 or len({n['path_id'] for n in networks}) != 1:
            return None
        delta = networks[-1]['lost_packets'] - networks[0]['lost_packets']
        return delta if delta >= 0 else None

    gaps = []
    for left, right in zip(presented, presented[1:]):
        gap = (right['elapsed_us'] - left['elapsed_us']) / 1000
        if gap < threshold_ms:
            continue
        first, last = left['frame'], right['frame']
        row = {'from_frame': first, 'to_frame': last, 'notification_gap_ms': gap,
               'elapsed_s': right['elapsed_us'] / 1e6, 'coalesced_frames': max(0, last-first-1),
               'sender_loss_detected_near_gap': loss_near(right['elapsed_us'])}
        row['next_frame_keyframe'] = received.get(last, {}).get('keyframe')
        if udp:
            row['concurrent_probes'] = probes_between(left['elapsed_us'], right['elapsed_us'])
        if first in received and last in received:
            row['receive_interval_ms'] = (received[last]['elapsed_us'] - received[first]['elapsed_us']) / 1000
            row['read_wait_ms'] = received[last].get('read_us', 0) / 1000
            row['receive_to_notify_ms'] = (right['elapsed_us'] - received[last]['elapsed_us']) / 1000
        row['previous_submit_ms'] = submitted.get(first, {}).get('submit_us', 0) / 1000
        row['native_presentation'] = right.get('native_timings')
        if first in traces and last in traces:
            row['producer_interval_ms'] = (traces[last]['elapsed_us'] - traces[first]['elapsed_us']) / 1000
        candidates = [traces[i] for i in range(first+1, last+1) if i in traces]
        stages = []
        for f in candidates:
            c = f.get('capture') or {}
            d = c.get('detail') or {}
            values = dict(credit=f['credit_us'], budget=f['budget_us'], source=f['source_us'],
                          native_wait=c.get('wait_us', 0), encode=c.get('encode_us', 0),
                          readback=d.get('readback_us', 0), conversion=d.get('conversion_us', 0),
                          codec=d.get('codec_us', 0), send=f.get('send_us') or 0, ack=f.get('ack_us') or 0,
                          worker_ipc=max(0, d.get('worker_roundtrip_us', 0)-d.get('native_total_us', 0)),
                          helper_ipc=max(0, d.get('helper_roundtrip_us', 0)-d.get('worker_roundtrip_us', 0)))
            stages.append({'frame': f['frame_id'], **{k: v/1000 for k, v in values.items()}})
        row['frame_stages_ms'] = stages
        if row['previous_submit_ms'] >= gap * .6:
            row['segment'] = 'receiver_submit_or_decoder_backpressure'
        elif row.get('receive_to_notify_ms', 0) >= gap * .6:
            row['segment'] = 'receiver_decode_or_presentation'
        elif row.get('producer_interval_ms', gap) < gap * .5 and row.get('receive_interval_ms', 0) >= gap * .7:
            row['segment'] = 'transport_or_receiver_read_wait'
        elif stages:
            stage = max(stages, key=lambda s: max(s['credit'], s['budget'], s['source'], s['send']))
            if stage['budget'] >= gap * .5:
                row['segment'] = 'encoder_reconfiguration'
            elif stage['credit'] >= gap * .5:
                row['segment'] = 'producer_display_credit_wait'
            elif stage['source'] >= gap * .5:
                row['segment'] = max(('native_wait', 'readback', 'conversion', 'codec', 'worker_ipc', 'helper_ipc'), key=lambda k: stage[k])
            else:
                row['segment'] = 'mixed_or_unresolved'
        else:
            row['segment'] = 'missing_frame_trace'
        prior_controls = [controls[i] for i in range(max(1, first-3), first+1) if i in controls]
        row['receiver_control_before'] = prior_controls
        if prior_controls:
            row['ack_queue_before_ms'] = max(c.get('queue_us', 0) for c in prior_controls) / 1000
            row['ack_write_before_ms'] = max(c.get('write_us', 0) for c in prior_controls) / 1000
            row['control_reply_before_ms'] = max(c.get('reply_us', 0) for c in prior_controls) / 1000
            if row['segment'] == 'producer_display_credit_wait' and row['ack_queue_before_ms'] >= gap * .5:
                row['segment'] = 'receiver_control_queue'
        gaps.append(row)
    gaps.sort(key=lambda e: e['notification_gap_ms'], reverse=True)
    missing = sorted(set(received) - set(traces))
    telemetry = events[-1] if events and events[-1].get('event') == 'telemetry' else {}
    good_probes = [e for e in udp if e.get('status') == 'ok']
    return {'opened': opened, 'trace_complete': bool(received and polls and not missing
            and any(e.get('event') == 'closed' for e in events)
            and telemetry.get('dropped_events') == 0 and telemetry.get('written_events') == len(events)-1
            and not any(e.get('event') == 'error' for e in events)),
            'release_receiver': opened.get('debug_assertions') is False,
            'received_frames': len(received), 'traced_frames': len(traces), 'missing_frames': missing,
            'receiver_control_frames': len(controls),
            'selected_endpoints': [{'remote_addr': r, 'local_addr': l} for r, l in sorted(endpoints)],
            'threshold_ms': threshold_ms, 'gaps': gaps,
            'network_probe': dict(opened=next((e for e in events if e.get('event') == 'network_probe_opened'), None),
                samples=len(udp), completed=len(good_probes),
                span_s=(max(e['elapsed_us'] for e in udp)-min(e['send_us'] for e in udp))/1e6 if udp else None,
                max_rtt_ms=max((e['rtt_us']/1000 for e in good_probes), default=None),
                note='Overlapping receiver-clock intervals show correlation, not one-way latency or physical-network causation.'),
            'note': 'Segments are evidence, not proof of physical network loss. Native decode includes queue/admission; present_call is not panel scanout. Peer clocks are never directly subtracted.'}


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('input', type=Path)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--network-errors', type=Path, help='matching responder error JSONL, joined by probe sequence')
    args = p.parse_args()
    result = analyze(args.input, network_errors_path=args.network_errors)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2, ensure_ascii=False), encoding='utf-8')
    print(json.dumps({'trace_complete': result['trace_complete'], 'gaps': len(result['gaps']), 'missing_frames': len(result['missing_frames'])}))


if __name__ == '__main__':
    main()
