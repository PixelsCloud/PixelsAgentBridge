"""Product version allocation, manifest synchronization and build provenance."""
from contextlib import contextmanager
from hashlib import sha256
from pathlib import Path
import json
import os
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


def synchronize(root, version):
    """Only own package versions change; registry/vendor dependencies stay pinned."""
    parse_version(version)
    manifests = sorted((root / 'crates').glob('*/Cargo.toml'))
    manifests.append(root / 'apps/desktop/src-tauri/Cargo.toml')
    names = set()
    changes = {}
    for path in manifests:
        text = path.read_text(encoding='utf-8')
        package = tomllib.loads(text)['package']
        names.add(package['name'])
        text, count = re.subn(r'(?m)^(version\s*=\s*)"[^"]+"', lambda m: m[1] + f'"{version}"', text, count=1)
        if count != 1:
            raise ValueError(f'Package version missing in {path}')
        changes[path] = text
    for path in (root / 'Cargo.lock', root / 'apps/desktop/src-tauri/Cargo.lock'):
        text = path.read_text(encoding='utf-8')
        blocks = text.split('[[package]]')
        for index, block in enumerate(blocks[1:], 1):
            data = tomllib.loads(block)
            if data['name'] in names and 'source' not in data:
                blocks[index] = re.sub(r'(?m)^version = "[^"]+"', f'version = "{version}"', block, count=1)
        changes[path] = '[[package]]'.join(blocks)
    for app in ('desktop', 'web'):
        folder = root / 'apps' / app
        for name in ('package.json', 'package-lock.json'):
            path = folder / name
            data = json.loads(path.read_text(encoding='utf-8'))
            data['version'] = version
            if name == 'package-lock.json':
                data['packages']['']['version'] = version
            changes[path] = json.dumps(data, indent=2, ensure_ascii=False) + '\n'
    path = root / 'apps/desktop/src-tauri/tauri.conf.json'
    data = json.loads(path.read_text(encoding='utf-8'))
    data['version'] = version
    changes[path] = json.dumps(data, indent=2, ensure_ascii=False) + '\n'
    # Validate all inputs before writing any manifests. A retry repairs partial writes.
    for path, text in changes.items():
        write_text(path, text)


def reserve_version(root):
    state = read_state(root)
    version = next_version(state['version']) if state['build_count'] else state['version']
    # Reserve first: a failed or interrupted build never reuses a number.
    write_json(root / 'build-version.json', {'version': version, 'build_count': state['build_count'] + 1})
    synchronize(root, version)
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
    write_json(folder / f'{target}-{profile}.json', {
        'version': version,
        'files': {name: digest(path) for name, path in files.items()},
    })


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
