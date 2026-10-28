"""Sequential compile/run probes for old Linux DRM/libc headers; no display access."""
import argparse
import hashlib
import json
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if not sys.platform.startswith('linux'):
        parser.error('This native probe requires Linux')
    output = args.output.resolve()
    output.relative_to(ROOT / 'smoke')
    output.mkdir(parents=True, exist_ok=True)
    compiler = shutil.which('g++')
    if not compiler:
        parser.error('g++ is unavailable')
    counts = {'passed': 0, 'failed': 0, 'skipped': 0}
    records = []
    try:
        flags = shlex.split(subprocess.check_output(['pkg-config', '--cflags', 'libdrm'], text=True))
        with tempfile.TemporaryDirectory(prefix='linux-media-', dir=output) as temporary:
            for variant in ('current', 'legacy'):
                binary = Path(temporary) / variant
                command = [compiler, '-std=c++17', '-Wall', '-Wextra', '-Werror']
                if variant == 'legacy':
                    command.append('-DPAB_TEST_OLD_SYSCALL_HEADERS=1')
                command += ['-I' + str(ROOT / 'crates/desktop-media/native/linux'), *flags,
                            str(ROOT / 'smoke/linux_media_compat.cpp'), '-o', str(binary)]
                with (output / (variant + '.log')).open('w') as log:
                    built = subprocess.run(command, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT)
                    if built.returncode:
                        counts['failed'] += 1
                        records.append({'variant': variant, 'phase': 'compile', 'exit_code': built.returncode})
                        continue
                    artifact_sha256 = hashlib.sha256(binary.read_bytes()).hexdigest()
                    tested = subprocess.run([str(binary)], cwd=ROOT, stdout=log, stderr=subprocess.STDOUT)
                    counts['passed' if tested.returncode == 0 else 'failed'] += 1
                    records.append({'variant': variant, 'phase': 'run', 'exit_code': tested.returncode,
                                    'artifact_sha256': artifact_sha256})
    except Exception as error:
        counts['failed'] += 1
        records.append({'phase': 'setup_or_cleanup', 'error_type': type(error).__name__, 'message': str(error)})
    (output / 'counts.json').write_text(json.dumps(counts) + '\n')
    (output / 'native-probe.json').write_text(json.dumps({'scope': 'Linux native compile/ABI forwarding only',
                                                        'probes': records}, indent=2) + '\n')
    print(json.dumps(counts))
    return 0 if counts == {'passed': 2, 'failed': 0, 'skipped': 0} else 1


if __name__ == '__main__':
    raise SystemExit(main())
