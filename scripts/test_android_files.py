#!/usr/bin/env python3
"""Exercise existing filesystem/transfer MCP tools against the normal Android service.

ADB starts the ordinary home and verifies the installed artifact only. Every tested
file operation travels through authenticated PAB. Results retain hashes and states,
not text payloads or binary contents. The optional mount must be a user-granted tree.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
from pathlib import Path
import time
import uuid
import zipfile

from android_home import Home, normal_status
from android_public import PublicMcp
from test_android_remote import ROOT, installed_apk_sha256


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--operator-run-id', required=True, type=uuid.UUID)
    parser.add_argument('--mount', default='/workspace')
    parser.add_argument('--device-code', help='Use another normally paired device without changing the original session evidence')
    parser.add_argument('--apk-sha256', required=True)
    parser.add_argument('--mcp-executable', type=Path, help='Use an isolated test build without updating an installed application')
    parser.add_argument('--extended', action='store_true', help='Also verify filesystem v2/v3 and remove this run\'s files')
    args = parser.parse_args()
    assert args.mount.startswith('/') and '..' not in args.mount.split('/')
    assert installed_apk_sha256(args.serial) == args.apk_sha256
    public = ROOT / '.build/android/public' / str(args.operator_run_id)
    session = json.loads((public / 'session.json').read_text(encoding='utf-8'))
    code = args.device_code or session['device_code']
    assert re.fullmatch(r'[0-9]{9}', code)
    run_id = str(uuid.uuid4())
    directory = ROOT / '.build/android/files' / run_id
    directory.mkdir(parents=True)
    result = {'status': 'failed', 'run_id': run_id, 'started_at_unix': time.time(),
        'provenance': {'apk_sha256': args.apk_sha256, 'serial': args.serial,
            'normal_service': True, 'public_server': True, 'forced_relay': True},
        'mount': args.mount, 'checks': [], 'final_acceptance': False}
    executable = args.mcp_executable or public / 'pab-mcp.exe'
    result['provenance']['mcp_sha256'] = hashlib.sha256(executable.read_bytes()).hexdigest()
    home = Home(args.serial)
    previous_paused = normal_status(args.serial)['paused']
    home.start()
    home.set_paused(False)
    client = PublicMcp(public / 'mcp.toml', code, True,
        directory / 'mcp.log', executable)

    def call(name, values, *, allow_failure=False):
        arguments = {'device_code': code, **values}
        value, raw = client.tool(name, arguments, allow_error=True)
        with (directory / 'operation-trace.jsonl').open('a', encoding='utf-8') as output:
            data = value.get('result', value)
            output.write(json.dumps({'tool': name, 'operation_ref': value.get('operation_ref'),
                'state': data.get('state'), 'error': data.get('error'), 'is_error': bool(raw.get('isError')),
                'request_id': arguments.get('request_id'),
                'wall_time_unix': time.time()}) + '\n')
        if raw.get('isError') and not allow_failure:
            raise RuntimeError(f'{name}: {error_code(value)}')
        return value

    def error_code(value):
        error = value.get('result', {}).get('error') or value.get('error') or {}
        return error.get('code') if isinstance(error, dict) else error

    def file_call(name, values):
        value = call(name, values)
        if name != 'pab_file_read':
            deadline = time.monotonic() + 120
            reference = value['operation_ref']
            while value['result']['state'] == 'running':
                assert time.monotonic() < deadline, 'Query original file request; do not replay'
                value = call('pab_get_operation', {'operation_id': reference['operation_id'], 'wait_ms': 1000})
        assert value['result']['state'] == 'completed', (name, value['result'].get('error'))
        return value

    def transfer(name, source, destination):
        queued = call(name, {'source': str(source), 'destination': str(destination),
            'request_id': str(uuid.uuid4()), 'overwrite': False, 'wait_ms': 1000})
        operation = queued['operation_ref']
        deadline = time.monotonic() + 120
        while True:
            value = call('pab_get_operation', {'operation_id': operation['operation_id'], 'wait_ms': 1000})
            record = value.get('operation', {})
            state = value.get('state') or record.get('state') or record.get('operation', {}).get('state')
            if state in ('completed', 'failed', 'cancelled', 'unconfirmed', 'interrupted'):
                assert state == 'completed', value
                return value
            assert time.monotonic() < deadline, 'Transfer result remained unconfirmed'
            time.sleep(.2)

    def asynchronous_file(name, values, *, allow_failure=False):
        value = call(name, {**values, 'request_id': values.get('request_id', str(uuid.uuid4()))}, allow_failure=allow_failure)
        reference = value['operation_ref']
        deadline = time.monotonic() + 180
        while value['result']['state'] in ('running', 'cancel_requested'):
            assert time.monotonic() < deadline, 'File operation remained unresolved; do not replay'
            value = call('pab_get_operation', {'operation_id': reference['operation_id'], 'wait_ms': 1000}, allow_failure=allow_failure)
        if not allow_failure:
            assert value['result']['state'] == 'completed', value['result'].get('error')
        return value

    try:
        connected, raw = client.connect()
        assert connected.get('connected') and not raw.get('isError')
        assert connected['target']['execution']['os_family'] == 'android'
        prefix = 'pixels-acceptance-' + run_id[:8]
        text_path = args.mount.rstrip('/') + '/' + prefix + '-中文.txt'
        binary_path = args.mount.rstrip('/') + '/' + prefix + '.bin'
        utf16_path = args.mount.rstrip('/') + '/' + prefix + '-utf16.txt'
        empty_path = args.mount.rstrip('/') + '/' + prefix + '-empty.bin'
        result['test_paths'] = [text_path, binary_path, utf16_path, empty_path]
        result['test_files_cleanup_pending'] = True
        initial = '第一行：你好 Android\n第二行：文件工具\n'
        request_id = str(uuid.uuid4())
        written = file_call('pab_file_write', {'path': text_path, 'content': initial, 'request_id': request_id})
        initial_hash = hashlib.sha256(initial.encode()).hexdigest()
        assert written['result']['metadata']['sha256'] == initial_hash
        read = file_call('pab_file_read', {'path': text_path, 'mode': 'lines', 'start_line': 1, 'count': 10})
        assert read['text'] == initial
        duplicate = file_call('pab_file_write', {'path': text_path, 'content': initial, 'request_id': request_id})
        assert duplicate['result']['metadata']['sha256'] == initial_hash
        conflict = call('pab_file_write', {'path': text_path, 'content': 'different', 'request_id': request_id}, allow_failure=True)
        conflict_error = conflict.get('error', {})
        assert error_code(conflict) == 'request_conflict' or (
            error_code(conflict) == 'operation_error'
            and 'request ID already exists with a different device or command' in conflict_error.get('message', '')
        ), 'Changed parameters were not rejected as a request-ID conflict'
        unchanged = file_call('pab_file_read', {'path': text_path})
        assert unchanged['text'] == initial
        patched = file_call('pab_file_patch', {'path': text_path, 'expected_hash': initial_hash,
            'edits': [{'find': '文件工具', 'replace': '真实远程修改', 'expected_matches': 1}]})
        changed = initial.replace('文件工具', '真实远程修改')
        changed_hash = hashlib.sha256(changed.encode()).hexdigest()
        assert patched['result']['metadata']['sha256'] == changed_hash
        assert file_call('pab_file_read', {'path': text_path})['text'] == changed
        stale = call('pab_file_write', {'path': text_path, 'content': 'stale', 'overwrite': True,
            'expected_hash': initial_hash}, allow_failure=True)
        assert error_code(stale) == 'hash_conflict'
        assert file_call('pab_file_read', {'path': text_path})['text'] == changed
        result['checks'].append({'name': 'text_read_write_patch_conflict_and_dedup', 'passed': True,
            'sha256': changed_hash})
        listing = call('pab_list_directory', {'path': args.mount, 'limit': 64})
        page = listing.get('page', listing)
        assert any(item['name'] == Path(text_path).name for item in page['entries'])
        result['checks'].append({'name': 'directory_page', 'passed': True})
        utf16_text = '编码字符 😀\r\n第二行\r\n'
        utf16 = file_call('pab_file_write', {'path': utf16_path, 'content': utf16_text, 'encoding': 'utf16_le'})
        utf16_bytes = b'\xff\xfe' + utf16_text.encode('utf-16-le')
        assert utf16['result']['metadata']['sha256'] == hashlib.sha256(utf16_bytes).hexdigest()
        assert file_call('pab_file_read', {'path': utf16_path})['text'] == utf16_text
        result['checks'].append({'name': 'utf16_bom_and_surrogates', 'passed': True})
        original = directory / 'source.bin'
        original.write_bytes(os.urandom(1024 * 1024 + 37))
        expected = hashlib.sha256(original.read_bytes()).hexdigest()
        transfer('pab_upload_file', original, binary_path)
        downloaded = directory / 'downloaded.bin'
        transfer('pab_download_file', binary_path, downloaded)
        assert downloaded.stat().st_size == original.stat().st_size
        assert hashlib.sha256(downloaded.read_bytes()).hexdigest() == expected
        result['checks'].append({'name': 'binary_upload_download', 'passed': True,
            'size': original.stat().st_size, 'sha256': expected})
        empty = directory / 'empty.bin'
        empty.write_bytes(b'')
        transfer('pab_upload_file', empty, empty_path)
        empty_downloaded = directory / 'empty-downloaded.bin'
        transfer('pab_download_file', empty_path, empty_downloaded)
        assert empty_downloaded.read_bytes() == b''
        result['checks'].append({'name': 'empty_binary_upload_download', 'passed': True})
        if args.extended:
            tree = args.mount.rstrip('/') + '/' + prefix + '-tree'
            result['test_directories'] = [tree]
            source = tree + '/源'
            created = file_call('pab_mkdir', {'path': source + '/空目录', 'parents': True})
            assert created['result']['created_paths'] == [tree, source, source + '/空目录']
            body = '第一行 needle Ω\n第二行\n'
            file_call('pab_file_write', {'path': source + '/笔记.txt', 'content': body})
            file_call('pab_file_write', {'path': source + '/a-first.txt', 'content': 'ASCII pagination fixture'})
            page = call('pab_list_directory', {'path': source, 'limit': 1})
            page = page.get('page', page)
            assert [item['name'] for item in page['entries']] == ['a-first.txt']
            assert page['next_after'] == 'a-first.txt'
            following = call('pab_list_directory', {'path': source, 'after': page['next_after'], 'limit': 64})
            assert {item['name'] for item in following.get('page', following)['entries']} == {'空目录', '笔记.txt'}
            expected_text = hashlib.sha256(body.encode()).hexdigest()
            by_name = file_call('pab_file_search', {'path': tree, 'query': '笔记'})
            assert [item['path'] for item in by_name['result']['search']['matches']] == [source + '/笔记.txt']
            by_content = file_call('pab_file_search', {'path': tree, 'mode': 'content', 'query': 'needle'})
            matches = by_content['result']['search']['matches']
            assert len(matches) == 1 and matches[0]['line'] == 1 and matches[0]['sha256'] == expected_text
            hashed = asynchronous_file('pab_file_hash', {'path': binary_path})
            assert hashed['result']['metadata']['sha256'] == expected
            assert hashed['result']['progress']['completed_bytes'] == original.stat().st_size
            result['checks'].append({'name': 'mkdir_search_and_binary_hash', 'passed': True})
            copied = tree + '/副本'
            asynchronous_file('pab_file_copy', {'path': source, 'destination': copied, 'recursive': True})
            assert file_call('pab_file_read', {'path': copied + '/笔记.txt'})['text'] == body
            limited = asynchronous_file('pab_file_copy', {'path': source, 'destination': tree + '/预算拒绝',
                'recursive': True, 'max_entries': 1}, allow_failure=True)
            assert limited['result']['state'] == 'failed' and error_code(limited) == 'entry_limit'
            assert error_code(call('pab_file_stat', {'path': tree + '/预算拒绝'}, allow_failure=True)) == 'not_found'
            moved = tree + '/已移动'
            moving = asynchronous_file('pab_file_move', {'path': copied, 'destination': moved, 'recursive': True})
            assert moving['result']['mutation']['source_removed'] is True
            assert error_code(call('pab_file_stat', {'path': copied}, allow_failure=True)) == 'not_found'
            assert file_call('pab_file_read', {'path': moved + '/笔记.txt'})['text'] == body
            result['checks'].append({'name': 'recursive_copy_move_and_preflight_budget', 'passed': True})
            archive = tree + '/资料.zip'
            asynchronous_file('pab_archive_create', {'path': archive, 'sources': [source]})
            extracted = tree + '/解压'
            asynchronous_file('pab_archive_extract', {'path': archive, 'destination': extracted})
            assert file_call('pab_file_read', {'path': extracted + '/源/笔记.txt'})['text'] == body
            assert file_call('pab_file_stat', {'path': extracted + '/源/空目录'})['result']['metadata']['kind'] == 'directory'
            result['checks'].append({'name': 'zip_unicode_and_empty_directory_roundtrip', 'passed': True})
            malicious = directory / 'unsafe.zip'
            with zipfile.ZipFile(malicious, 'w') as zipped:
                zipped.writestr('../outside.txt', 'must never be published')
            bad_remote = tree + '/unsafe.zip'
            transfer('pab_upload_file', malicious, bad_remote)
            refused = asynchronous_file('pab_archive_extract', {'path': bad_remote, 'destination': tree + '/拒绝解压'}, allow_failure=True)
            assert refused['result']['state'] == 'failed' and error_code(refused) == 'unsafe_archive_path'
            assert error_code(call('pab_file_stat', {'path': tree + '/拒绝解压'}, allow_failure=True)) == 'not_found'
            result['checks'].append({'name': 'zip_traversal_rejected_before_destination_creation', 'passed': True})
            asynchronous_file('pab_file_delete', {'path': tree, 'recursive': True})
            assert error_code(call('pab_file_stat', {'path': tree}, allow_failure=True)) == 'not_found'
            for path in result['test_paths']:
                asynchronous_file('pab_file_delete', {'path': path})
                assert error_code(call('pab_file_stat', {'path': path}, allow_failure=True)) == 'not_found'
            result['test_files_cleanup_pending'] = False
            result['checks'].append({'name': 'remote_delete_and_test_file_cleanup', 'passed': True})
        for path in ('/workspace/../credentials', '/credentials', '/workspace/a\\b'):
            blocked = call('pab_file_stat', {'path': path}, allow_failure=True)
            assert error_code(blocked) in ('invalid_path', 'mount_not_found', 'access_denied')
        result['checks'].append({'name': 'private_roots_and_traversal_blocked', 'passed': True})
        result['status'] = 'passed'
    finally:
        client.close()
        home.set_paused(previous_paused)
        result['installed_apk_unchanged'] = installed_apk_sha256(args.serial) == args.apk_sha256
        result['ending_status'] = normal_status(args.serial)
        result['finished_at_unix'] = time.time()
        (directory / 'result.json').write_text(json.dumps(result, indent=2, ensure_ascii=False), encoding='utf-8')
        print(json.dumps({'run_id': run_id, 'status': result['status'], 'checks': len(result['checks']),
            'result': str(directory / 'result.json')}, ensure_ascii=False), flush=True)


if __name__ == '__main__':
    main()
