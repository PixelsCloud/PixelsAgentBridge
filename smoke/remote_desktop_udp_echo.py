"""Bounded, non-amplifying LAN echo for the optional desktop network probe."""
import argparse
import collections
import errno
import ipaddress
import json
from pathlib import Path
import socket
import time

MAGIC = b'PABRDNET'


def transient(error):
    return (getattr(error, 'winerror', None) in (10035, 10054, 10055)
            or error.errno in (errno.EAGAIN, errno.EWOULDBLOCK, errno.ENOBUFS, errno.ECONNRESET))


def send_reply(sock, data, address, record):
    try:
        sock.sendto(data, address)
        return True
    except OSError as error:
        record('send', error, int.from_bytes(data[8:16], 'little'))
        if not transient(error):
            raise
        # Drop this datagram: retries would hide the observed local send failure.
        return False


def private_ipv4(value):
    ip = ipaddress.IPv4Address(value)
    allowed = ip.is_loopback or any(ip in net for net in (
        ipaddress.IPv4Network('10.0.0.0/8'), ipaddress.IPv4Network('172.16.0.0/12'),
        ipaddress.IPv4Network('192.168.0.0/16')))
    if not allowed:
        raise ValueError('an explicit private or loopback IPv4 address is required')
    return str(ip)


def reply(data, peer, allowed_peer, received_ns):
    if peer != allowed_peer or len(data) != 64 or data[:8] != MAGIC or any(data[32:]):
        return None
    result = bytearray(data)
    work_us = max(0, (time.perf_counter_ns() - received_ns) // 1000)
    result[32:40] = work_us.to_bytes(8, 'little')
    return bytes(result)


def serve(bind, peer, ready, seconds, port=0):
    bind, peer = private_ipv4(bind), private_ipv4(peer)
    if not ready.is_absolute() or 'smoke' not in ready.parts:
        raise ValueError('ready file must be an absolute path under smoke')
    if not 30 <= seconds <= 3600 or not 0 <= port <= 65535:
        raise ValueError('invalid duration or port')
    summary, stop = ready.with_suffix('.summary.json'), ready.with_suffix('.stop')
    errors_path = ready.with_suffix('.errors.jsonl')
    if any(p.exists() for p in (ready, summary, stop, errors_path)):
        raise FileExistsError('use fresh diagnostic output paths')
    accepted = rejected = errors = 0
    error_types = collections.Counter()
    began = time.monotonic()
    status = 'failed'
    with errors_path.open('x', encoding='utf-8') as error_file:
        def record(operation, error, sequence=None):
            nonlocal errors
            errors += 1
            code = getattr(error, 'winerror', None) or error.errno
            error_types[f'{operation}:{code}'] += 1
            if errors <= 256:
                error_file.write(json.dumps(dict(operation=operation, sequence=sequence,
                    elapsed_us=int((time.monotonic()-began)*1e6), errno=error.errno,
                    winerror=getattr(error, 'winerror', None)))+'\n')
                error_file.flush()
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
                sock.bind((bind, port))
                sock.settimeout(.05)
                with ready.open('x', encoding='utf-8') as file:
                    json.dump(dict(bind=bind, peer=peer, port=sock.getsockname()[1], seconds=seconds), file)
                while time.monotonic()-began < seconds and not stop.exists() and accepted+rejected+errors < 72000:
                    try:
                        data, address = sock.recvfrom(65)
                    except socket.timeout:
                        continue
                    except OSError as error:
                        record('receive', error)
                        if not transient(error):
                            raise
                        time.sleep(.005)
                        continue
                    received_ns = time.perf_counter_ns()
                    response = reply(data, address[0], peer, received_ns)
                    if response is None:
                        rejected += 1
                        continue
                    accepted += send_reply(sock, response, address, record)
                status = 'completed'
        finally:
            with summary.open('x', encoding='utf-8') as file:
                json.dump(dict(status=status, accepted=accepted, rejected=rejected,
                    errors=errors, error_types=dict(error_types), error_records=min(errors, 256),
                    elapsed_s=time.monotonic()-began), file)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--bind', required=True)
    p.add_argument('--peer', required=True)
    p.add_argument('--ready', type=Path, required=True)
    p.add_argument('--seconds', type=int, default=420)
    p.add_argument('--port', type=int, default=0)
    args = p.parse_args()
    serve(args.bind, args.peer, args.ready, args.seconds, args.port)


if __name__ == '__main__':
    main()
