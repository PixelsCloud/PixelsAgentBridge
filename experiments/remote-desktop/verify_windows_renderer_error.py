"""Verify native failure framing using invalid input in a private viewer process."""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import subprocess

p = argparse.ArgumentParser()
p.add_argument('binary', type=Path)
p.add_argument('output', type=Path)
p.add_argument('--desktop', action='store_true')
p.add_argument('--expect-legacy', action='store_true')
a = p.parse_args()
assert not a.output.exists(), 'Preserve previous evidence'
command = [str(a.binary.resolve())]
if a.desktop:
    command.append('--media-worker')
command += ['render', '320', '240', '15', 'stream-interactive']
# This process owns only a generated, initially empty viewer. No remote
# connection, input enabled message, credentials or system-wide input is used.
child = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                         stderr=subprocess.PIPE, creationflags=subprocess.CREATE_NO_WINDOW)
try:
    # Greater than the native 16 MiB packet bound; no image allocation follows.
    stdout, stderr = child.communicate(struct.pack('<I', 16*1024*1024+1), timeout=20)
finally:
    if child.poll() is None:
        child.kill()
        child.wait(timeout=5)
expected = b'Render packet too large'
report = dict(binary_sha256=hashlib.sha256(a.binary.read_bytes()).hexdigest(),
              exit_code=child.returncode, stdout_hex=stdout.hex(), stderr_hex=stderr.hex(),
              expected_legacy=a.expect_legacy, passed=False)
try:
    assert child.returncode == 1
    assert stdout[:4] == b'PAB1'
    # Decoder-path metadata is emitted before reading the deliberately invalid packet.
    assert stdout[4:8] == struct.pack('<I', 2)
    assert stdout[8] == 4 and stdout[9] in (1, 2)
    if a.expect_legacy:
        assert stdout[10:] == b'' and stderr in (expected+b'\n', expected+b'\r\n')
    else:
        assert stdout[10:13] == bytes([5])+struct.pack('<H', len(expected))
        assert stdout[13:] == expected
        assert stderr[:8] == bytes([7])+struct.pack('<H', len(expected))+bytes(5)
        assert stderr[8:] == expected
    report['passed'] = True
finally:
    a.output.write_text(json.dumps(report, indent=2), encoding='utf-8')
print(json.dumps({'passed': report['passed'], 'exit_code': child.returncode,
                  'expected_legacy': a.expect_legacy}))
