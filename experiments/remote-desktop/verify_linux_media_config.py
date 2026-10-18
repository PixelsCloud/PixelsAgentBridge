"""Supervisor startup regression, for a disposable Linux container only.

Runs the real Desktop binary without a graphical session. Checks configuration
selection before any helper can register. Never use on an installed host.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

p = argparse.ArgumentParser()
p.add_argument('binary', type=Path)
p.add_argument('output', type=Path)
p.add_argument('--expect-legacy', action='store_true')
a = p.parse_args()
assert os.geteuid() == 0 and Path('/.dockerenv').exists()
assert os.environ.get('PAB_CONFIG_ACCEPTANCE_ISOLATED') == '1'
assert not Path('/run/pixels-agent-bridge-media').exists()
assert not Path('/etc/pixels-agent-bridge/config.toml').exists()
assert not Path('/var/lib/pixels-agent-bridge').exists()
assert not a.output.exists()
report = dict(binary_sha256=hashlib.sha256(a.binary.read_bytes()).hexdigest(),
              expected_legacy=a.expect_legacy, cases=[], passed=False)
env = {k: v for k, v in os.environ.items() if not k.startswith('PAB_')}
try:
    with tempfile.TemporaryDirectory(prefix='pab-media-config-') as directory:
        root = Path(directory)
        binary = root/'pab-desktop'
        shutil.copyfile(a.binary, binary)
        binary.chmod(0o755)
        config = root/'config.toml'
        config.write_text("control_url='wss://control.invalid/control'\nrelay_urls=['https://relay.invalid']\n"
                          "[executor]\ndata_dir='executor-data'\n"
                          "[operator]\ndata_dir='operator-data'\nlog_dir='operator-logs'\n")
        before = config.read_bytes()
        # Installed-path startup and explicit --config must use the same role.
        cases = ['explicit', 'installed'] if not a.expect_legacy else ['explicit']
        for case in cases:
            installed = Path('/etc/pixels-agent-bridge/config.toml')
            if case == 'installed':
                installed.parent.mkdir(parents=True)
                installed.write_text(config.read_text().replace("'executor-data'", repr(str(root/'executor-data')))
                                     .replace("'operator-data'", repr(str(root/'operator-data')))
                                     .replace("'operator-logs'", repr(str(root/'operator-logs'))))
            args = [str(binary), '--media-supervisor']
            if case == 'explicit':
                args += ['--config', str(config)]
            child = subprocess.Popen(args, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            try:
                expected = (Path('/var/lib/pixels-agent-bridge') if a.expect_legacy else root/'executor-data')/'logs/media-supervisor.log'
                deadline = time.monotonic()+8
                while True:
                    assert child.poll() is None, child.communicate()
                    status = Path(f'/proc/{child.pid}/status').read_text()
                    caught = int(next(row.split()[1] for row in status.splitlines()
                                      if row.startswith('SigCgt:')), 16)
                    if expected.exists() and caught & (1 << 14):
                        break
                    assert time.monotonic() < deadline, 'Configured log directory was not used'
                    time.sleep(.05)
                assert not (root/'operator-data').exists() and not (root/'operator-logs').exists()
                child.terminate()
                stdout, stderr = child.communicate(timeout=10)
                assert child.returncode == 0, stderr.decode(errors='replace')
                assert config.read_bytes() == before
                report['cases'].append(dict(case=case, exit_code=child.returncode,
                                            used_executor_directory=not a.expect_legacy,
                                            operator_directory_absent=True, config_preserved=True))
            finally:
                if child.poll() is None:
                    child.kill(); child.wait(timeout=5)
                if installed.exists():
                    installed.unlink()
                if Path('/run/pixels-agent-bridge-media').exists():
                    shutil.rmtree('/run/pixels-agent-bridge-media')
                if (root/'executor-data').exists():
                    shutil.rmtree(root/'executor-data')
        if not a.expect_legacy:
            for content in [None, 'not valid toml [']:
                bad = root/'invalid.toml'
                if content is not None:
                    bad.write_text(content)
                result = subprocess.run([str(binary), '--media-supervisor', '--config', str(bad)],
                                        env=env, capture_output=True, timeout=8)
                assert result.returncode == 1
                assert not Path('/run/pixels-agent-bridge-media').exists()
                report['cases'].append(dict(case='missing' if content is None else 'malformed',
                                            exit_code=result.returncode, no_runtime_created=True))
        report['passed'] = True
finally:
    a.output.write_text(json.dumps(report, indent=2), encoding='utf-8')
print(json.dumps(report))
