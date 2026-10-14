"""Recheck the stored feasibility evidence, not a substitute for running probes."""
import json
from pathlib import Path
import re

root = Path(__file__).resolve().parents[3]
acceptance = root / 'acceptance'
for mode in ('user', 'service'):
    path = acceptance / f'desktop-terminal-{mode}-direct-2026-10-14.json'
    result = json.loads(path.read_text(encoding='utf-8'))
    assert result['success'] and result['mode'] == mode, path
    assert result['stable_terminal_while_paused'], path
    assert result['reader_eof_after_worker_shutdown'], path
    assert result['large_output_bytes'] > 16 * 1024 * 1024, path
    assert result['pause_stats']['produced'] - result['pause_stats']['acked'] <= 262144, path
    assert result['final_stats']['max_inflight'] <= 262144, path
    assert all(route == 'direct' for route in result['paths']), path
    assert re.fullmatch('[0-9a-f]{64}', result['output_sha256']), path
    print(f'PASS {mode}: native PTY, authenticated multi-stream connection, stable tools, owned cleanup')

for size in ('small', 'large'):
    path = acceptance / f'desktop-terminal-webview-{size}-2026-10-14.json'
    result = json.loads(path.read_text(encoding='utf-8'))
    assert result['success'] and result['pauseRecovered'], path
    assert result['received'] == result['consumed'] == result['native_received_bytes'], path
    assert result['received'] == result['finalStats']['produced'] == result['finalStats']['acked'], path
    assert result['native_received_sha256'] == result['finalStats']['sha256'], path
    assert result['maxPending'] <= 262144 and result['finalStats']['max_inflight'] <= 262144, path
    assert result['warmedLines'] == result['secondLines'] <= 10064, path
    if size == 'large':
        assert result['received'] > 64 * 1024 * 1024, path
    print(f'PASS {size}: native WebView, exact bytes/hash, consumption credit, hide/recovery, bounded scrollback')

for path in (root / 'REMOTE_TERMINAL_PLAN.md', acceptance / 'desktop-terminal-feasibility-2026-10-14.md'):
    for reference in re.findall(r'\]\(([^)]+)\)', path.read_text(encoding='utf-8')):
        if not reference.startswith(('http://', 'https://')):
            assert (path.parent / reference).exists(), (path, reference)
print('PASS document links')
