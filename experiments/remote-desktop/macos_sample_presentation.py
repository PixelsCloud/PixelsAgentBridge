"""Observe composed frame codes through an already-authorized Mac test bundle.

The existing capture helper performs a separate local 60 FPS observation. Its
H264 packets are decoded only to read generated frame-code cells. No frames are
saved. This adds observer encode/decode load and therefore measures with that
load present; it does not count the product's decoder/presentation callbacks.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import select
import struct
import subprocess
import tempfile
import time


def read_exact(pipe, size):
    data = bytearray()
    deadline = time.monotonic() + 12
    while len(data) < size:
        remaining = deadline - time.monotonic()
        if remaining <= 0 or not select.select([pipe], [], [], remaining)[0]:
            raise TimeoutError('Observer stalled; inspect original capture process')
        part = os.read(pipe.fileno(), size-len(data))
        if not part:
            raise EOFError('Observer pipe closed')
        data.extend(part)
    return bytes(data)


def write_all(pipe, data):
    data = memoryview(data)
    while data:
        written = pipe.write(data)
        if not written: raise EOFError('Observer input pipe closed')
        data = data[written:]


def main():
    p = argparse.ArgumentParser()
    p.add_argument('profile', type=Path)
    p.add_argument('--helper', type=Path, required=True)
    p.add_argument('--helper-sha256', required=True)
    p.add_argument('--decoder', type=Path, required=True)
    p.add_argument('--view', type=int, nargs=4, required=True, metavar=('X', 'Y', 'WIDTH', 'HEIGHT'))
    p.add_argument('--display', type=int, default=1)
    p.add_argument('--seconds', type=int, default=30)
    args = p.parse_args()
    assert 10 <= args.seconds <= 60 and os.getuid() != 0
    assert hashlib.sha256(args.helper.read_bytes()).hexdigest() == args.helper_sha256
    pid = json.loads((args.profile/'process.json').read_text())['pid']
    assert subprocess.check_output(['/bin/ps', '-p', str(pid), '-o', 'comm='], text=True).strip() == str(args.helper)
    events, observations = [], []
    capture = decoder = None
    worker_pid = None
    pipes = tempfile.TemporaryDirectory(prefix='observer-', dir=args.profile)
    capture_command = str(args.helper) + ' --media-worker capture ' + str(args.display) + ' 60 desktop'
    def capture_pids():
        result = []
        for line in subprocess.check_output(['/bin/ps', '-axo', 'pid=,command='], text=True).splitlines():
            fields = line.strip().split(None, 1)
            if len(fields) == 2 and fields[1] == capture_command: result.append(int(fields[0]))
        return result
    assert not capture_pids(), 'Candidate capture already running'
    try:
        with (args.profile/'observer-capture.err').open('wb') as caperr, (args.profile/'observer-decoder.err').open('wb') as decerr:
            incoming, outgoing = Path(pipes.name)/'input', Path(pipes.name)/'output'
            for path in (incoming, outgoing): os.mkfifo(path, 0o600)
            reader = os.fdopen(os.open(outgoing, os.O_RDWR), 'rb', buffering=0)
            writer = os.fdopen(os.open(incoming, os.O_RDWR), 'wb', buffering=0)
            # LaunchServices preserves the approved application's responsible
            # identity; spawning the binary from Python inherits Python's TCC context.
            capture = subprocess.Popen(['/usr/bin/open', '-n', '-g', '-W', '--stdin', str(incoming),
                                        '--stdout', str(outgoing), '--stderr', str(args.profile/'observer-capture.err'),
                                        str(args.helper.parents[2]), '--args', '--media-worker', 'capture',
                                        str(args.display), '60', 'desktop'], stdout=subprocess.DEVNULL, stderr=caperr)
            capture.stdin, capture.stdout = writer, reader
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                children = capture_pids()
                if children:
                    assert len(children) == 1
                    worker_pid = children[0]; break
                if capture.poll() is not None: raise RuntimeError('Capture app exited during launch')
                time.sleep(.05)
            magic, width, height = struct.unpack('<III', read_exact(capture.stdout, 12))
            assert magic == 0x31424150 and (width, height) == (1920, 1080)
            decoder = subprocess.Popen([str(args.decoder), str(width), str(height), *map(str, args.view), str(pid)],
                                       stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=decerr, bufsize=0)
            start = time.monotonic(); last = None; first_sample = None; previous_sample = None
            invalid = repeated = 0
            while time.monotonic() - start < args.seconds:
                capture.stdin.write(b'\x01')
                size, index, sampled, key = struct.unpack('<IQQI', read_exact(capture.stdout, 24))
                assert 0 < size <= 16 * 1024 * 1024 and index == len(observations) and key in (0, 1)
                packet = read_exact(capture.stdout, size)
                write_all(decoder.stdin, struct.pack('<IQQ', size, index, sampled) + packet)
                line = bytearray()
                while not line.endswith(b'\n'):
                    line.extend(read_exact(decoder.stdout, 1))
                    assert len(line) < 512
                row = json.loads(line)
                assert row['index'] == index and row['sample_us'] == sampled
                if previous_sample is not None: assert sampled > previous_sample
                previous_sample = sampled
                if first_sample is None: first_sample = sampled
                elapsed = time.monotonic() - start
                observations.append([elapsed, sampled, row['code']])
                code = row['code']
                if code < 0: invalid += 1
                elif code == last: repeated += 1
                else:
                    if last is not None: assert 0 < (code-last) % 4096 < 2048, 'Frame code went backwards'
                    events.append([elapsed, code]); last = code
            duration = time.monotonic() - start
            capture.stdin.write(b'\x00'); capture.stdin.close()
            decoder.stdin.write(struct.pack('<I', 0)); decoder.stdin.close()
            assert capture.wait(timeout=8) == decoder.wait(timeout=8) == 0
        steady = [e for e in events if e[0] >= 3]
        times = [3] + [e[0] for e in steady] + [duration]
        rate = len(steady)/(duration-3)
        report = {'method': 'composed display observed through separate local capture and hardware decode',
                  'observer_load': '60 FPS capture + VideoToolbox encode/decode; CPU reads only code cells',
                  'seconds': duration, 'samples': len(observations), 'invalid_samples': invalid,
                  'repeated_samples': repeated, 'unique_presentations': len(events), 'warmup_seconds': 3,
                  'steady_fps': rate, 'longest_presentation_gap_seconds': max(b-a for a,b in zip(times,times[1:])),
                  'helper_sha256': args.helper_sha256, 'view': args.view, 'events': events,
                  'observations': observations, 'passed': rate > 12 and invalid <= len(observations)*.05 and len(observations)/duration >= 30}
        (args.profile/'presentation.json').write_text(json.dumps(report, indent=2))
        print(json.dumps({k:v for k,v in report.items() if k not in ('events','observations')}), flush=True)
        assert report['passed'], 'Actual composed presentation threshold not met'
    finally:
        if capture is not None:
            for pipe in (capture.stdin, capture.stdout):
                if pipe and not pipe.closed: pipe.close()
        for process in (capture, decoder):
            if process is not None and process.poll() is None:
                process.terminate()
                try: process.wait(timeout=5)
                except subprocess.TimeoutExpired: process.kill(); process.wait(timeout=5)
        if worker_pid is not None and worker_pid in capture_pids():
            os.kill(worker_pid, 15)
            deadline = time.monotonic() + 5
            while worker_pid in capture_pids() and time.monotonic() < deadline: time.sleep(.1)
            assert worker_pid not in capture_pids(), 'Owned observer capture remains'
        pipes.cleanup()


if __name__ == '__main__': main()
