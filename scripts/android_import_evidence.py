#!/usr/bin/env python3
"""Verify and import an allowlisted remote Android evidence ZIP into the workspace."""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import stat
import uuid
import zipfile

from test_android_remote import ROOT


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--archive', required=True, type=Path)
    parser.add_argument('--expected-sha256', required=True)
    parser.add_argument('--run-id', required=True, type=uuid.UUID)
    args = parser.parse_args()
    source = args.archive.resolve()
    assert source.is_relative_to((ROOT / '.build/android/xrserver').resolve())
    assert hashlib.sha256(source.read_bytes()).hexdigest() == args.expected_sha256
    destination = source.parent / 'data'
    assert not destination.exists(), 'Never overwrite previously imported evidence'
    top = {'network.json', 'operator-environment.json', 'session.json',
        'old-password-check.json', 'upgrade-proof-v14.json'}
    nested = {'result.json', 'progress.json', 'operation-trace.jsonl', 'standby-trace.jsonl'}
    with zipfile.ZipFile(source) as archive:
        infos = archive.infolist()
        assert len(infos) <= 256 and len({item.filename for item in infos}) == len(infos)
        assert sum(item.file_size for item in infos) <= 8 * 1024 * 1024
        for item in infos:
            path = PurePosixPath(item.filename)
            assert not path.is_absolute() and '..' not in path.parts and '\\' not in item.filename
            assert not stat.S_ISLNK(item.external_attr >> 16) and item.file_size <= 1024 * 1024
            allowed = item.filename in top or item.filename == 'export-manifest.json'
            allowed = allowed or bool(re.fullmatch(r'artifact-transition-[0-9a-f-]{36}\.json', item.filename))
            if len(path.parts) == 2:
                allowed = bool(re.fullmatch(r'(campaign|soak)-[0-9a-f-]{36}', path.parts[0])) and path.parts[1] in nested
            assert allowed, 'Unexpected content in remote evidence archive'
        manifest = json.loads(archive.read('export-manifest.json'))
        assert manifest['run_id'] == str(args.run_id)
        assert manifest['credential_files_included'] is False
        assert manifest['installed_application_updated'] is False
        expected = {row['path']:row for row in manifest['entries']}
        assert len(expected) == len(manifest['entries'])
        assert set(expected) == {item.filename for item in infos} - {'export-manifest.json'}
        for name, entry in expected.items():
            payload = archive.read(name)
            assert len(payload) == entry['size']
            assert hashlib.sha256(payload).hexdigest() == entry['sha256']
        # All paths and all payloads are validated before any extraction occurs.
        destination.mkdir()
        for item in infos:
            target = destination.joinpath(*PurePosixPath(item.filename).parts)
            assert target.resolve().is_relative_to(destination.resolve())
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(archive.read(item))
    print(json.dumps({'status':'passed', 'destination':str(destination), 'files':len(infos),
        'archive_sha256':args.expected_sha256, 'credentials_included':False}))


if __name__ == '__main__':
    main()
