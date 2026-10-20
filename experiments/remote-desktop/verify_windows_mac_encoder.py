"""Replay verify_macos_encoder.py's generated fixtures on a Windows worker.

Usage: verify_windows_mac_encoder.py WORKER FIXTURE_DIRECTORY
Creates only owned generated-image windows. No screen capture or input.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path

from verify_macos_renderer import render


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('worker', type=Path)
    parser.add_argument('directory', type=Path)
    args = parser.parse_args()
    assert os.name == 'nt'
    worker = args.worker.resolve(strict=True)
    folder = args.directory.resolve(strict=True)
    source = json.loads((folder / 'report.json').read_text())
    assert source['passed'] and source['actual_screen_capture_tested'] is False
    result = folder / 'windows-render.json'
    assert not result.exists(), 'Preserve earlier evidence; use a separate directory for another run'
    report = {'worker_sha256': hashlib.sha256(worker.read_bytes()).hexdigest(), 'cases': []}
    for case in source['capture']:
        path = folder / (case['mode'] + '.pabvideo')
        assert path.parent == folder and hashlib.sha256(path.read_bytes()).hexdigest() == case['fixture_sha256']
        for decoder in ('hardware', 'software'):
            value = render(worker, folder, case['mode'], 0, True, decoder, hardware_decoder_code=1)
            report['cases'].append(value)
            result.write_text(json.dumps(report, indent=2) + '\n')
            print(json.dumps(value), flush=True)
    report['passed'] = all(case['passed'] for case in report['cases'])
    result.write_text(json.dumps(report, indent=2) + '\n')
    raise SystemExit(0 if report['passed'] else 1)


if __name__ == '__main__':
    main()
