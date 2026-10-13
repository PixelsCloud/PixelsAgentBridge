"""Validate the six live TCP forwarding acceptance reports."""

import json
from pathlib import Path


ROOT = Path(__file__).resolve().parent
STAGES = {
    'open', 'session_isolation', 'echo', 'backpressure', 'remote_dns', 'http',
    'refused', 'connection_limit', 'before_listener_limit', 'listener_limit',
    'closed', 'disconnect',
}


def load(platform, path):
    name = f'port-forward-{platform}-{path}'
    if platform == 'linux':
        document = json.loads((ROOT / f'{name}.json').read_text(encoding='utf-8-sig'))
        assert document['path'] == path
        assert set(document['package_mcp_tools']) == {
            'pab_port_forward_open', 'pab_port_forward_status', 'pab_port_forward_close'}
        rows = document['results']
    else:
        rows = [json.loads(line) for line in (ROOT / f'{name}.jsonl').read_text(
            encoding='utf-8-sig').splitlines()]
    result = {row['stage']: row for row in rows}
    assert set(result) == STAGES, (name, set(result) ^ STAGES)
    assert result['open']['forward']['connection_path'] == path, name
    echo = result['echo']['status']
    assert echo['successful_connections'] == 65 and echo['failed_connections'] == 0, name
    assert echo['bytes_to_remote'] == echo['bytes_to_local'] == 267555, name
    assert result['backpressure']['bytes'] == 2_000_003, name
    assert len(result['backpressure']['sha256']) == 64, name
    assert result['http']['path'] == path and result['http']['curl'], name
    refused = result['refused']['status']
    assert refused['state'] == 'listening' and refused['failed_connections'] == 1, name
    assert refused['last_error_at_unix_ms'] > 0, name
    assert result['connection_limit'] == {'stage': 'connection_limit', 'active': 64,
                                          'failed': 1}, name
    assert result['before_listener_limit']['states'] == ['listening'] * 3, name
    assert result['listener_limit']['active_listeners'] == 8, name
    assert result['disconnect']['status']['state'] == 'failed', name
    print(f'{platform:7} {path:6} PASS 65 exact TCP exchanges, 2,000,003-byte slow read, '
          'HTTP/curl, limits, isolation, disconnect')


for platform in ('windows', 'macos', 'linux'):
    for path in ('direct', 'relay'):
        load(platform, path)
