"""Verify a second operator can view after the first viewer has closed.

Uses the existing input acceptance executable but sends only Close, after its
first actual presentation. Never enables input. Run after the product viewer
has closed, with an isolated profile and an independent remote rollback.
"""
import argparse
import hashlib
import json
import pathlib
import queue
import subprocess
import threading
import time


def main():
    p = argparse.ArgumentParser()
    p.add_argument('--profile', type=pathlib.Path, required=True)
    p.add_argument('--probe', type=pathlib.Path, required=True)
    p.add_argument('--code', required=True)
    a = p.parse_args()
    assert len(a.code) == 9 and a.code.isdecimal()
    root, probe = a.profile.resolve(strict=True), a.probe.resolve(strict=True)
    destination = root / 'viewer-handoff.json'
    assert not destination.exists(), 'Preserve previous results; do not replay an uncertain open'
    report = {'code': a.code, 'probe_sha256': hashlib.sha256(probe.read_bytes()).hexdigest(),
              'input_sent': False, 'passed': False}

    def save():
        destination.write_text(json.dumps(report, indent=2), encoding='utf-8')

    check = subprocess.run(['node', str(pathlib.Path(__file__).with_name('viewer_reopen_cdp.mjs')),
                            'closed', a.code], capture_output=True, text=True, timeout=40)
    assert check.returncode == 0 and json.loads(check.stdout) is True
    save()
    started = time.monotonic()
    process = None
    try:
        with (root / 'viewer-handoff.stderr.log').open('xb') as errors:
            process = subprocess.Popen([str(probe), '--config', str(root / 'config.toml'), a.code],
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=errors,
                text=True, encoding='utf-8', creationflags=subprocess.CREATE_NO_WINDOW)
            messages = queue.Queue()

            def read():
                try:
                    for line in process.stdout:
                        messages.put(json.loads(line))
                except BaseException as error:
                    messages.put(error)
                finally:
                    messages.put(EOFError('Probe ended before expected presentation/close'))

            reader = threading.Thread(target=read, daemon=True)
            reader.start()

            def next_message():
                value = messages.get(timeout=max(.01, 35 - (time.monotonic() - started)))
                if isinstance(value, BaseException):
                    raise value
                return value

            ready = next_message()
            assert ready.get('ready') is True, ready
            report.update(first_presented=True, screen=ready['screen'],
                          first_presented_seconds=time.monotonic() - started)
            save()
            # Exactly one cleanup action; no Enable or Input commands.
            process.stdin.write('{"command":"close"}\n')
            process.stdin.flush()
            closed = next_message()
            assert closed.get('closed') is True and closed.get('legacy_environment') is True, closed
            assert process.wait(timeout=10) == 0
            report.update(passed=True, closed=True, legacy_environment=True,
                          elapsed_seconds=time.monotonic() - started)
    except BaseException as error:
        report['failure'] = type(error).__name__ + ': ' + str(error)
        raise
    finally:
        if process is not None:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=10)
            if process.stdin:
                process.stdin.close()
            if process.stdout:
                process.stdout.close()
            report['exit_code'] = process.returncode
        save()
    print(json.dumps(report), flush=True)


if __name__ == '__main__':
    main()
