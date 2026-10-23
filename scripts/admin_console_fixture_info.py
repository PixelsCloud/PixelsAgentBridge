"""Describe a selected task-owned HTTPS fixture after checking its DB boundary."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tomllib
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[1]


def fixture_info(config):
    config = config.resolve()
    if config.is_relative_to(ROOT / 'smoke/local'):
        sys.path.insert(0, str(ROOT))
        from smoke.web_fixture import fixture_info as smoke_fixture_info
        return smoke_fixture_info(config)
    relative = config.relative_to(ROOT / '.build/admin-console')
    allowed = relative.parts[0] in {'20261020-b3', '20261020-b4', '20261020-b5', '20261020-b6'} and relative.parts[1:] == ('https', 'pab-server.toml')
    allowed |= relative.parts == ('20261020-b7-global', 'candidate', 'https', 'pab-server.toml')
    assert allowed, 'Only this task\'s explicitly selected browser fixtures are supported'
    settings = tomllib.loads(config.read_text(encoding='utf-8'))
    database = urlsplit(settings['database']['url'])
    assert database.hostname == '127.0.0.1' and database.path.startswith('/admin_browser_')
    assert settings['listen'] == '127.0.0.1:38623' and settings['web']['origin'] == 'https://127.0.0.1:38623'
    container = 'pab-admin-test-20261020'
    info = json.loads(subprocess.check_output(['docker', 'inspect', container], text=True))[0]
    assert info['Config']['Labels'].get('pixels.task') == 'admin-console-20261020'
    assert info['State']['Running'] and any(p['HostIp'] == '127.0.0.1' and int(p['HostPort']) == database.port for p in info['NetworkSettings']['Ports']['5432/tcp'])
    return {'config': str(config), 'container': container, 'database': database.path[1:],
            'origin': settings['web']['origin'], 'cert': settings['tls']['cert'],
            'executable': str(config.parent / 'pab-server.exe')}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', type=Path, required=True)
    print(json.dumps(fixture_info(parser.parse_args().config)))
