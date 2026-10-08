"""Installer version allocation and build provenance; component versions stay unchanged."""
from contextlib import contextmanager
from hashlib import sha256
from pathlib import Path
import json
import os
import plistlib
import re
import tomllib


def parse_version(value):
    if not isinstance(value, str) or not re.fullmatch(r'(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)', value):
        raise ValueError(f'Invalid product version: {value!r}')
    parts = tuple(map(int, value.split('.')))
    if parts[1] >= 100 or parts[2] >= 100:
        raise ValueError('Minor and patch must be in 0..99')
    return parts


def next_version(value):
    major, minor, patch = parse_version(value)
    patch += 1
    if patch == 100:
        minor, patch = minor + 1, 0
    if minor == 100:
        major, minor = major + 1, 0
    return f'{major}.{minor}.{patch}'


def read_state(root):
    data = json.loads((root / 'build-version.json').read_text(encoding='utf-8'))
    parse_version(data['version'])
    if type(data['build_count']) is not int or data['build_count'] < 0:
        raise ValueError('Invalid build count')
    return data


def write_text(path, text):
    if path.exists() and path.read_text(encoding='utf-8') == text:
        return
    temporary = path.with_name(path.name + '.version-tmp')
    temporary.write_text(text, encoding='utf-8', newline='\n')
    os.replace(temporary, path)


def write_json(path, data):
    write_text(path, json.dumps(data, indent=2, ensure_ascii=False) + '\n')


@contextmanager
def build_lock(root):
    """OS releases the lock after a crash; never delete an in-use lock file."""
    folder = root / '.build'
    folder.mkdir(exist_ok=True)
    with (folder / 'version.lock').open('a+b') as handle:
        handle.seek(0, 2)
        if handle.tell() == 0:
            handle.write(b'0')
            handle.flush()
        handle.seek(0)
        try:
            if os.name == 'nt':
                import msvcrt
                msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
            else:
                import fcntl
                fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError as error:
            raise RuntimeError('Another product build is running in this checkout') from error
        try:
            yield
        finally:
            handle.seek(0)
            if os.name == 'nt':
                msvcrt.locking(handle.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                fcntl.flock(handle.fileno(), fcntl.LOCK_UN)


def reserve_version(root):
    state = read_state(root)
    version = next_version(state['version']) if state['build_count'] else state['version']
    # Reserve first: a failed or interrupted build never reuses a number.
    write_json(root / 'build-version.json', {'version': version, 'build_count': state['build_count'] + 1})
    return version


def digest(path):
    result = sha256()
    with path.open('rb') as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b''):
            result.update(chunk)
    return result.hexdigest()


def record_artifacts(root, target, profile, version, files):
    folder = root / '.build/builds'
    folder.mkdir(parents=True, exist_ok=True)
    components = {}
    for name, relative in {'pab-executor': 'crates/executor', 'pab-mcp': 'crates/bridge',
                           'pab-desktop': 'apps/desktop/src-tauri', 'pab-server': 'crates/server',
                           'pab-relay-server': 'crates/relay'}.items():
        manifest = root / relative / 'Cargo.toml'
        if manifest.is_file() and any(Path(key).stem == name for key in files):
            components[name] = tomllib.loads(manifest.read_text(encoding='utf-8'))['package']['version']
    write_json(folder / f'{target}-{profile}.json', {
        'version': version,
        'components': components,
        'files': {name: digest(path) for name, path in files.items()},
    })


def macos_artifacts(binaries, app):
    """Bind both service binaries and the complete built app to one build record."""
    if not (app / 'Contents/MacOS/pab-desktop').is_file():
        raise ValueError(f'Missing built macOS app: {app}')
    files = {name: binaries / name for name in ('pab-executor', 'pab-mcp')}
    files.update({'Pixels Agent Bridge.app/' + path.relative_to(app).as_posix(): path
                  for path in sorted(app.rglob('*')) if path.is_file()})
    return files


def macos_app_version(app):
    with (app / 'Contents/Info.plist').open('rb') as source:
        version = plistlib.load(source)['CFBundleShortVersionString']
    parse_version(version)
    return version


def verify_artifacts(root, target, profile, files):
    path = root / '.build/builds' / f'{target}-{profile}.json'
    if not path.is_file():
        raise ValueError(f'Missing build record: use python scripts/build.py {target} first')
    data = json.loads(path.read_text(encoding='utf-8'))
    parse_version(data['version'])
    if set(data['files']) != set(files):
        raise ValueError('Build artifact list changed; rebuild before packaging')
    for name, file in files.items():
        if digest(file) != data['files'][name]:
            raise ValueError(f'Build artifact changed since recorded build: {name}')
    return data['version']
