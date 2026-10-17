"""Bounded egress loss for one staged test peer, without changing root qdisc.

Run as root on the authorized VM with: SCRIPT STAGE PEER_IPV4 PORT... .
The caller must verify that every port belongs to its isolated Desktop process.
Only packets to that exact IPv4/UDP port tuple are dropped, for seven seconds.
The script refuses existing clsact/ingress state and keeps a rollback timer.
"""
import fcntl
import ipaddress
import json
import os
import pathlib
import subprocess
import sys
import time

stage = pathlib.Path(sys.argv[1]).resolve(strict=True)
assert os.geteuid() == 0 and stage.parent == pathlib.Path('/tmp')
assert stage.name.startswith('pab-formal-linux-') and stage.stat().st_uid == 0
assert stage.stat().st_mode & 0o022 == 0
state = stage / 'udp-fault.json'
rollback = stage.name + '-udp-rollback'


def run(*args):
    return subprocess.check_output(args, text=True, timeout=10).strip()


def data(*args):
    return json.loads(run(*args))


def filters(dev):
    return data('tc', '-j', '-s', 'filter', 'show', 'dev', dev, 'egress')


def restore():
    with (stage / 'udp-fault.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        saved = json.loads(state.read_text())
        dev = saved['dev']
        active = filters(dev)
        owned = set(saved['preferences'])
        # Do not remove a qdisc somebody else has begun using.
        assert all(f.get('pref') in owned and f.get('kind') == 'flower' for f in active), active
        for pref in sorted({f['pref'] for f in active}):
            run('tc', 'filter', 'del', 'dev', dev, 'egress', 'protocol', 'ip', 'pref', str(pref))
        qdiscs = data('tc', '-j', 'qdisc', 'show', 'dev', dev)
        if any(q['kind'] == 'clsact' for q in qdiscs):
            assert not filters(dev)
            assert not data('tc', '-j', 'filter', 'show', 'dev', dev, 'ingress')
            run('tc', 'qdisc', 'del', 'dev', dev, 'clsact')
        after = data('tc', '-j', 'qdisc', 'show', 'dev', dev)
        assert after == saved['qdiscs_before'], (after, saved['qdiscs_before'])
        saved['restored'] = True
        saved['qdiscs_after'] = after
        state.write_text(json.dumps(saved, indent=2))


if sys.argv[2] == 'restore':
    restore()
    sys.exit(0)

peer = str(ipaddress.IPv4Address(sys.argv[2]))
assert ipaddress.ip_address(peer).is_private and not ipaddress.ip_address(peer).is_loopback
ports = sorted(set(int(p) for p in sys.argv[3:]))
assert 0 < len(ports) <= 4 and all(1024 <= p <= 65535 for p in ports)
assert not state.exists()
route = data('ip', '-j', 'route', 'get', peer)
assert len(route) == 1 and route[0]['dev'] == 'ens33'
dev = route[0]['dev']
pid = int(run('systemctl', 'show', 'pixels-agent-bridge-executor.service', '-p', 'MainPID', '--value'))
assert pathlib.Path(f'/proc/{pid}/exe').resolve() == stage / 'pab-executor'
qdiscs = data('tc', '-j', 'qdisc', 'show', 'dev', dev)
assert all(q['kind'] not in ('clsact', 'ingress') for q in qdiscs), qdiscs
assert not filters(dev)
preferences = [49152 + i for i in range(len(ports))]
saved = dict(peer=peer, ports=ports, dev=dev, preferences=preferences,
             qdiscs_before=qdiscs, restored=False, duration_seconds=7)
state.write_text(json.dumps(saved, indent=2))
run('systemd-run', '--unit=' + rollback, '--on-active=20s',
    '--property=Type=oneshot', '/usr/bin/python3', str(pathlib.Path(__file__).resolve()),
    str(stage), 'restore')
try:
    run('tc', 'qdisc', 'add', 'dev', dev, 'clsact')
    for pref, port in zip(preferences, ports):
        run('tc', 'filter', 'add', 'dev', dev, 'egress', 'protocol', 'ip', 'pref', str(pref),
            'flower', 'skip_hw', 'dst_ip', peer, 'ip_proto', 'udp', 'dst_port', str(port),
            'action', 'drop')
    started = time.monotonic()
    (stage / 'udp-fault-active').touch()
    time.sleep(7)
    saved['observed_seconds'] = time.monotonic() - started
    saved['filter_statistics'] = filters(dev)
    state.write_text(json.dumps(saved, indent=2))
finally:
    restore()
    run('systemctl', 'stop', rollback + '.timer')
    (stage / 'udp-fault-done').touch()
print(state.read_text(), flush=True)
