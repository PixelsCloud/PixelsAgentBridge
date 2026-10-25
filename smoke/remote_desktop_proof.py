"""Join synthetic capture, packet equality and decoded/rendered cell checks.

This diagnostic reads metadata only. Grid samples are not a pixel-perfect
full-frame visual comparison, and correctness readbacks are not FPS evidence.
"""
import argparse
import json
from pathlib import Path


def load(path):
    return [json.loads(line) for line in Path(path).read_text(encoding='utf-8-sig').splitlines() if line.strip()]


def analyze(events, captures, require_encoder=None):
    errors = []
    opened = next((e for e in events if e.get('event') == 'opened'), {})
    closed = next((e for e in events if e.get('event') == 'closed'), {})
    summary = closed.get('native_summary') or {}
    telemetry = events[-1] if events and events[-1].get('event') == 'telemetry' else {}
    received = [e for e in events if e.get('event') == 'received']
    presented = [e for e in events if e.get('event') == 'presented']
    source = {e['frame']: e for e in captures}
    output = {e['frame']: (e.get('native_timings') or {}).get('pattern_proof') for e in presented}
    encoders = sorted({e['encoder'] for e in captures if e.get('encoder')})
    if require_encoder and (not captures or any(e.get('encoder') != require_encoder for e in captures)):
        errors.append('required encoder was not observed on every captured frame')
    if not opened.get('pattern_proof') or not closed or not received:
        errors.append('missing proof-mode open, received frames or normal close')
    if len(source) != len(captures) or len(output) != len(presented):
        errors.append('duplicate capture or presentation identifiers')
    if telemetry.get('dropped_events') != 0 or telemetry.get('written_events') != len(events)-1:
        errors.append('incomplete telemetry')
    errors.extend(e.get('message', 'probe error') for e in events if e.get('event') == 'error')
    if [e['frame'] for e in received] != list(range(1, len(received)+1)):
        errors.append('received frame sequence is incomplete')
    patterns = []
    max_decoded = max_rendered = 0
    for event in received:
        frame = event['frame']
        original, proof = source.get(frame), output.get(frame)
        if not original or not proof:
            errors.append(f'frame {frame}: missing capture or pixel proof')
            continue
        if original.get('checks') != 576 or original.get('max_error') != 0:
            errors.append(f'frame {frame}: capture source was not verified exactly')
        if not event.get('fingerprint') or event['fingerprint'] != original.get('fingerprint'):
            errors.append(f'frame {frame}: encoded packet fingerprint mismatch')
        if proof.get('checks') != 1152 or proof.get('pattern') != original.get('pattern'):
            errors.append(f'frame {frame}: decoded/rendered picture belongs to another source frame')
        if not (0 <= proof.get('decoded_error', 999) <= 16 and 0 <= proof.get('rendered_error', 999) <= 16):
            errors.append(f'frame {frame}: pixel color check failed')
        patterns.append(proof['pattern'])
        max_decoded = max(max_decoded, proof['decoded_error'])
        max_rendered = max(max_rendered, proof['rendered_error'])
    if len(set(patterns)) < 10:
        errors.append('insufficient independently changing synthetic pictures')
    if summary.get('test_pixel_checks') != len(received)*1152:
        errors.append('native pixel-check total does not cover all received frames')
    if closed and not summary.get('displayed'):
        errors.append('no native swapchain presentations were observed')
    paths = sorted({e['path'] for e in events if e.get('event') == 'diagnostics'})
    if opened.get('relay_requested') and (not paths or any(p != 'Some(Relay)' for p in paths)):
        errors.append('forced Relay was not observed throughout measurement')
    return dict(status='failed' if errors else 'passed', errors=errors, received_frames=len(received),
                capture_frames=len(captures), presented_frames=len(presented), distinct_patterns=len(set(patterns)),
                max_decoded_rgb_error=max_decoded, max_rendered_rgb_error=max_rendered,
                native_pixel_checks=summary.get('test_pixel_checks'), decoder=summary.get('decoder'), paths=paths, encoders=encoders,
                note='576 cell centers per stage/frame; tolerance 0 capture, 16 codec/render. Fingerprint is noncryptographic. Diagnostic readbacks invalidate performance comparisons.')


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('receiver', type=Path)
    p.add_argument('capture', type=Path)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--require-encoder', choices=['intel_mf_gpu', 'nvenc_gpu', 'openh264_software'])
    args = p.parse_args()
    result = analyze(load(args.receiver), load(args.capture), args.require_encoder)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2), encoding='utf-8')
    print(json.dumps(result))
    return int(result['status'] != 'passed')


if __name__ == '__main__':
    raise SystemExit(main())
