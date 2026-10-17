"""Bounded egress loss for one staged test peer, without changing root qdisc.

Run as root on the authorized VM with: SCRIPT STAGE PEER_IPV4 PORT... .
The caller must verify that every port belongs to its isolated Desktop process.
Only packets to that exact IPv4/UDP port tuple are dropped, for seven seconds.
Add --netem for a 20-second 150ms/128kbit bounded queue on an owned IFB instead.
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
        if saved.get('ifb'):
            name = saved['ifb']
            exists = subprocess.run(['ip', 'link', 'show', 'dev', name], capture_output=True, timeout=5).returncode == 0
            if exists:
                link = data('ip', '-j', '-d', 'link', 'show', 'dev', name)[0]
                assert link['ifindex'] == saved['ifb_ifindex'] and link['linkinfo']['info_kind'] == 'ifb'
                assert pathlib.Path('/sys/class/net', name, 'ifalias').read_text().strip() == saved.get('ifb_alias', '')
                run('ip', 'link', 'del', 'dev', name)
        after = data('tc', '-j', 'qdisc', 'show', 'dev', dev)
        assert after == saved['qdiscs_before'], (after, saved['qdiscs_before'])
        saved['restored'] = True
        saved['qdiscs_after'] = after
        state.write_text(json.dumps(saved, indent=2))


if sys.argv[2] == 'restore':
    restore()
    sys.exit(0)

netem = '--netem' in sys.argv[3:]
peer = str(ipaddress.IPv4Address(sys.argv[2]))
assert ipaddress.ip_address(peer).is_private and not ipaddress.ip_address(peer).is_loopback
ports = sorted(set(int(p) for p in sys.argv[3:] if p != '--netem'))
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
             qdiscs_before=qdiscs, restored=False, duration_seconds=20 if netem else 7,
             profile='150ms_128kbit' if netem else 'drop')
if netem:
    saved['ifb'] = 'pab' + stage.name[-10:]
    assert subprocess.run(['ip', 'link', 'show', 'dev', saved['ifb']], capture_output=True, timeout=5).returncode != 0
state.write_text(json.dumps(saved, indent=2))
run('systemd-run', '--unit=' + rollback, '--on-active=45s' if netem else '--on-active=20s',
    '--property=Type=oneshot', '/usr/bin/python3', str(pathlib.Path(__file__).resolve()),
    str(stage), 'restore')
try:
    if netem:
        name = saved['ifb']
        run('ip', 'link', 'add', 'name', name, 'type', 'ifb')
        saved['ifb_ifindex'] = data('ip', '-j', 'link', 'show', 'dev', name)[0]['ifindex']
        state.write_text(json.dumps(saved, indent=2))
        run('ip', 'link', 'set', 'dev', name, 'alias', stage.name)
        saved['ifb_alias'] = stage.name
        state.write_text(json.dumps(saved, indent=2))
        assert pathlib.Path('/sys/class/net', name, 'ifalias').read_text().strip() == stage.name
        run('ip', 'link', 'set', 'dev', name, 'up')
        run('tc', 'qdisc', 'add', 'dev', name, 'root', 'netem', 'delay', '150ms', 'rate', '128kbit', 'limit', '64')
    run('tc', 'qdisc', 'add', 'dev', dev, 'clsact')
    for pref, port in zip(preferences, ports):
        run('tc', 'filter', 'add', 'dev', dev, 'egress', 'protocol', 'ip', 'pref', str(pref),
            'flower', 'skip_hw', 'dst_ip', peer, 'ip_proto', 'udp', 'dst_port', str(port),
            'action', *(['mirred', 'egress', 'redirect', 'dev', saved['ifb']] if netem else ['drop']))
    started = time.monotonic()
    (stage / 'udp-fault-active').touch()
    samples = []
    for _ in range(saved['duration_seconds']):
        time.sleep(1)
        if netem:
            samples.append(dict(elapsed=time.monotonic() - started,
                                qdisc=data('tc', '-j', '-s', 'qdisc', 'show', 'dev', saved['ifb'])))
    if netem:
        saved['queue_samples'] = samples
    saved['observed_seconds'] = time.monotonic() - started
    saved['filter_statistics'] = filters(dev)
    state.write_text(json.dumps(saved, indent=2))
finally:
    restore()
    run('systemctl', 'stop', rollback + '.timer')
    (stage / 'udp-fault-done').touch()
print(state.read_text(), flush=True)
