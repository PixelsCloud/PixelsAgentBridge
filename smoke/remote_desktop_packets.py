"""Compare endpoint-local packet intervals; never subtract peer timestamps."""
import argparse
import collections
import datetime
import ipaddress
import json
from pathlib import Path
import re
import statistics
import struct

STAMP = re.compile(r'::(\d{4}-\d\d-\d\d) (\d\d):(\d\d):(\d\d)\.(\d+) ')
PACKET = re.compile(r'PktGroupId \d+.*Direction (Tx|Rx)\s*,.*Component (\d+), Edge (\d+)')
HEX = re.compile(r'^\s*0x[0-9a-fA-F]+:\s+([0-9a-fA-F ]+)\s*$')
COMPONENT = re.compile(r'Component (\d+), Type ([^,]+), Name (.*)')
DROP = re.compile(r'Drop Counters: Component (\d+).*Packets In (\d+).*Packets Out (\d+)')


def timestamp(match):
    day, hour, minute, second, fraction = match.groups()
    seconds = datetime.date.fromisoformat(day).toordinal()*86400+int(hour)*3600+int(minute)*60+int(second)
    return seconds*1_000_000_000+int(fraction.ljust(9, '0')[:9])


def probe_identity(data, source, receiver):
    if len(data) < 42 or data[12:14] != b'\x08\x00' or data[14] >> 4 != 4:
        return None
    ihl = (data[14] & 15)*4
    if ihl < 20 or data[23] != 17 or len(data) < 14+ihl+8+32:
        return None
    if struct.unpack_from('!H', data, 20)[0] & 0x3fff:
        return None
    src, dst = data[26:30], data[30:34]
    direction = 'request' if (src, dst) == (receiver, source) else 'response' if (src, dst) == (source, receiver) else None
    offset = 14+ihl+8
    if not direction or data[offset:offset+8] != b'PABRDNET':
        return None
    return (data[offset+16:offset+32].hex(), struct.unpack_from('<Q', data, offset+8)[0]), direction


def read_capture(path, source_ip, receiver_ip):
    source, receiver = ipaddress.IPv4Address(source_ip).packed, ipaddress.IPv4Address(receiver_ip).packed
    probes, components, drops = {}, {}, {}
    header, events_lost, buffers_lost = False, None, None
    current, data = None, bytearray()

    def finish():
        if current is None:
            return
        identity = probe_identity(data, source, receiver)
        if identity:
            key, direction = identity
            probes.setdefault(key, {}).setdefault(direction, []).append(current)

    with path.open('rb') as file:
        prefix = file.read(4)
    with path.open(encoding='utf-16' if prefix.startswith((b'\xff\xfe', b'\xfe\xff')) else 'utf-8-sig') as file:
        for line in file:
            if 'Event: Header,' in line:
                header = True
                lost = re.search(r'EventsLost: (\d+)', line)
                buffers = re.search(r'BuffersLost: (\d+)', line)
                events_lost = int(lost[1]) if lost else None
                buffers_lost = int(buffers[1]) if buffers else None
            component = COMPONENT.search(line)
            if component:
                components[int(component[1])] = dict(type=component[2].strip(), name=component[3].strip())
            drop = DROP.search(line)
            if drop:
                drops[int(drop[1])] = dict(rx=int(drop[2]), tx=int(drop[3]))
            packet, stamp = PACKET.search(line), STAMP.search(line)
            if packet and stamp:
                finish()
                current = dict(at=timestamp(stamp), direction=packet[1], component=int(packet[2]), edge=int(packet[3]))
                data = bytearray()
            else:
                hex_line = HEX.match(line)
                if hex_line and current is not None and len(data) < 256:
                    data.extend(bytes.fromhex(hex_line[1]))
    finish()
    return dict(probes=probes, components=components, drops=drops, header=header,
                events_lost=events_lost, buffers_lost=buffers_lost)


def stats(values):
    values = sorted(values)
    return dict(count=len(values), median=statistics.median(values),
                p95=values[min(len(values)-1, int(len(values)*.95))], maximum=max(values)) if values else None


def interval(rows, kind, component, direction):
    return [r['at'] for r in rows.get(kind, []) if r['component'] == component and r['direction'] == direction]


def analyze(source_path, receiver_path, source_ip, receiver_ip, source_nic, receiver_nic):
    source = read_capture(source_path, source_ip, receiver_ip)
    receiver = read_capture(receiver_path, source_ip, receiver_ip)
    rows = []
    missing = collections.Counter()
    for key in sorted(source['probes'].keys() | receiver['probes'].keys()):
        s, r = source['probes'].get(key, {}), receiver['probes'].get(key, {})
        sr = interval(s, 'request', source_nic, 'Rx')
        st = interval(s, 'response', source_nic, 'Tx')
        rt = interval(r, 'request', receiver_nic, 'Tx')
        rr = interval(r, 'response', receiver_nic, 'Rx')
        for name, values in [('source_request', sr), ('source_response', st), ('receiver_request', rt), ('receiver_response', rr)]:
            if not values:
                missing[name] += 1
        if not all((sr, st, rt, rr)):
            continue
        source_turn = (min(st)-min(sr))/1e6
        receiver_roundtrip = (min(rr)-min(rt))/1e6
        row = dict(sequence=key[1], session=key[0], source_nic_turnaround_ms=source_turn,
                   receiver_nic_roundtrip_ms=receiver_roundtrip,
                   unattributed_duration_ms=receiver_roundtrip-source_turn)
        for name, endpoint in [('source', s), ('receiver', r)]:
            for kind in ('request', 'response'):
                times = [v['at'] for v in endpoint.get(kind, [])]
                row[name+'_'+kind+'_stack_span_ms'] = (max(times)-min(times))/1e6 if times else None
        rows.append(row)
    complete = all(c['header'] and c['events_lost'] == 0 and c['buffers_lost'] == 0 for c in (source, receiver))
    return dict(etl_events_complete=complete, paired_observations_complete=bool(complete and rows and not missing),
        paired_probes=len(rows), missing_observations=dict(missing),
        source_components=source['components'], receiver_components=receiver['components'],
        source_drops=source['drops'], receiver_drops=receiver['drops'],
        source_events_lost=source['events_lost'], receiver_events_lost=receiver['events_lost'],
        source_buffers_lost=source['buffers_lost'], receiver_buffers_lost=receiver['buffers_lost'],
        intervals_ms={field: stats([row[field] for row in rows]) for field in (
            'source_nic_turnaround_ms', 'receiver_nic_roundtrip_ms', 'unattributed_duration_ms',
            'source_request_stack_span_ms', 'source_response_stack_span_ms',
            'receiver_request_stack_span_ms', 'receiver_response_stack_span_ms')},
        probes=rows,
        note='Only local intervals are compared. The remainder is not one-way or exact physical-wire latency. Missing observations are not automatically packet loss. Capture overhead can alter timing; compare with a capture-off run.')


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('source', type=Path)
    p.add_argument('receiver', type=Path)
    p.add_argument('--source-ip', required=True)
    p.add_argument('--receiver-ip', required=True)
    p.add_argument('--source-nic', type=int, required=True)
    p.add_argument('--receiver-nic', type=int, required=True)
    p.add_argument('--output', type=Path, required=True)
    args = p.parse_args()
    result = analyze(args.source, args.receiver, args.source_ip, args.receiver_ip, args.source_nic, args.receiver_nic)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2), encoding='utf-8')
    print(json.dumps({k: result[k] for k in ('etl_events_complete', 'paired_observations_complete', 'paired_probes', 'missing_observations', 'intervals_ms')}))


if __name__ == '__main__':
    main()
