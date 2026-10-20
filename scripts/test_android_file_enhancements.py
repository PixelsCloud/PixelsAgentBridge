#!/usr/bin/env python3
"""Exercise filesystem v4 over authenticated public PAB, never adb file operations."""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import shlex
import time
import uuid

from android_home import Home, normal_status
from android_public import PublicMcp
from test_android_remote import ROOT, installed_apk_sha256


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--operator-run-id', required=True, type=uuid.UUID)
    parser.add_argument('--device-code', required=True)
    parser.add_argument('--apk-sha256', required=True)
    parser.add_argument('--mcp-executable', required=True, type=Path)
    parser.add_argument('--mount', default='/workspace')
    args = parser.parse_args()
    assert installed_apk_sha256(args.serial) == args.apk_sha256
    assert args.mount.startswith('/') and '..' not in args.mount.split('/')
    directory = ROOT / '.build/android/file-enhancement' / str(uuid.uuid4())
    directory.mkdir(parents=True)
    operator = ROOT / '.build/android/public' / str(args.operator_run_id)
    remote = args.mount.rstrip('/') + '/pixels-enhanced-' + directory.name[:8]
    result = {'status': 'failed', 'serial': args.serial, 'mount': args.mount,
        'apk_sha256': args.apk_sha256, 'forced_relay': True, 'checks': [],
        'mcp_sha256': hashlib.sha256(args.mcp_executable.read_bytes()).hexdigest(),
        'qa_script_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        'remote_root': remote, 'cleanup_pending': False, 'full_android_acceptance': False}
    home = Home(args.serial)
    client = None
    tasks = {}

    def call(name, fields=None, *, failure=False):
        fields = {'device_code': args.device_code, **(fields or {})}
        value, raw = client.tool(name, fields, allow_error=True)
        with (directory / 'trace.jsonl').open('a', encoding='utf-8') as output:
            output.write(json.dumps({'tool': name, 'fields': fields, 'value': value,
                'is_error': bool(raw.get('isError')), 'time': time.time()}, ensure_ascii=False) + '\n')
        if not failure: assert not raw.get('isError'), (name, value)
        return value

    def file(name, fields, *, failure=False):
        value = call(name, fields, failure=failure)
        reference = value.get('operation_ref')
        deadline = time.monotonic() + 90
        while value.get('result', {}).get('state') in ('running', 'cancel_requested'):
            assert time.monotonic() < deadline, 'Query original request; never replay'
            time.sleep(.05)
            value = call('pab_get_operation', {'operation_id': reference['operation_id']}, failure=failure)
        if not failure: assert value['result']['state'] == 'completed', value
        return value

    def rejected(name, fields, code):
        value = file(name, fields, failure=True)
        error = value.get('result', {}).get('error') or value.get('error') or {}
        assert error.get('code') == code, value
        return value

    def passed(name, **details):
        result['checks'].append({'name': name, 'passed': True, **details})
        print(json.dumps({'check': name, 'passed': True}), flush=True)

    def write(path, content, **fields):
        return file('pab_file_write', {'path': path, 'content': content, **fields})

    def read(path, **fields):
        return file('pab_file_read', {'path': path, **fields})

    def command(script):
        value = call('pab_run_command', {'program': '/system/bin/sh', 'args': ['-c', script],
            'cwd': remote, 'request_id': str(uuid.uuid4()), 'wait_ms': 5000, 'timeout_ms': 30000})
        snapshot = value.get('task') or value.get('snapshot')
        task = snapshot['task_ref']['task_id']; tasks[task] = snapshot['request_id']
        deadline = time.monotonic() + 40
        while snapshot['state'] not in ('succeeded', 'failed', 'cancelled', 'interrupted'):
            assert time.monotonic() < deadline
            value = call('pab_get_task', {'task_id': task, 'wait_ms': 1000})
            snapshot = value.get('snapshot') or value.get('task')
        assert snapshot['state'] == 'succeeded', snapshot
        tasks.pop(task, None)

    def append(path, body):
        # Only the test-owned workspace is mapped into this app's native cwd.
        assert args.mount == '/workspace'
        octal = ''.join('\\%03o' % byte for byte in body)
        command("printf '%b' " + shlex.quote(octal) + ' >> ' + shlex.quote(Path(path).name))

    try:
        home.start(); home.set_paused(False)
        client = PublicMcp(operator / 'mcp.toml', args.device_code, True,
            directory / 'mcp.log', args.mcp_executable)
        connected, raw = client.connect()
        assert connected.get('connected') and not raw.get('isError'), connected
        file('pab_mkdir', {'path': remote})
        result['cleanup_pending'] = True
        for encoding in ('utf8', 'utf8_bom', 'utf16_le', 'utf16_be'):
            path = remote + '/' + encoding + '.log'
            text = '甲😀乙\r\n末尾\n'
            write(path, text, encoding=encoding)
            current = read(path, mode='follow', max_bytes=4)
            actual = current['text']; pages = 1
            while current['result']['range']['truncated']:
                assert pages < 40
                current = read(path, mode='follow', max_bytes=4,
                    cursor=current['result']['log']['cursor'], encoding=encoding)
                actual += current['text']; pages += 1
            assert actual == text, (encoding, actual)
            cursor = current['result']['log']['cursor']
            idle = read(path, mode='follow', cursor=cursor, encoding=encoding, wait_ms=250)
            assert idle['text'] == '' and idle['result']['log']['wait_expired'], idle
            tail = read(path, mode='tail', tail_bytes=5, encoding=encoding)
            assert text.endswith(tail['text']) and '\ufffd' not in tail['text'], tail
            rejected('pab_file_read', {'path': path, 'mode': 'follow', 'cursor': cursor[:-1] + 'x'}, 'invalid_cursor')
            rejected('pab_file_read', {'path': remote + '/other.log', 'mode': 'follow', 'cursor': cursor}, 'invalid_cursor')
        passed('streaming_four_encodings_unicode_boundaries_wait_and_cursor_scope')
        empty = remote + '/empty.log'
        write(empty, '')
        assert read(empty, mode='tail')['text'] == ''
        passed('empty_tail')

        if args.mount == '/workspace':
            path = remote + '/partial.log'
            write(path, 'prefix\n')
            cursor = read(path, mode='tail')['result']['log']['cursor']
            emoji = '😀'.encode()
            append(path, emoji[:2])
            partial = read(path, mode='follow', cursor=cursor)
            assert partial['text'] == '' and partial['result']['log']['incomplete_character'], partial
            append(path, emoji[2:])
            whole = read(path, mode='follow', cursor=partial['result']['log']['cursor'])
            assert whole['text'] == '😀', whole
            passed('partial_append_resumes_original_character')
            command("head -c 5242880 /dev/zero | tr '\\000' x > large.log; printf '%b' '\\347\\224\\262\\360\\237\\230\\200\\012' >> large.log")
            large = remote + '/large.log'
            value = read(large, mode='tail', tail_bytes=8)
            assert value['text'] == '甲😀\n' and value['result']['metadata']['size'] > 4194304, value
            assert value['result']['metadata']['sha256'] is None, value
            cursor = value['result']['log']['cursor']
            append(large, '新增\n'.encode())
            followed = read(large, mode='follow', cursor=cursor)
            assert followed['text'] == '新增\n', followed
            cursor = followed['result']['log']['cursor']
            command("printf x > large.log")
            rejected('pab_file_read', {'path': large, 'mode': 'follow', 'cursor': cursor}, 'log_changed')
            cursor = read(large, mode='tail')['result']['log']['cursor']
            command('mv large.log rotated.log; printf replacement > large.log')
            rejected('pab_file_read', {'path': large, 'mode': 'follow', 'cursor': cursor}, 'log_changed')
            passed('large_log_bounded_tail_append_truncation_and_rotation', size=5242888)

        search_root = remote + '/search'
        for sub in ('', '/sub', '/skip'):
            file('pab_mkdir', {'path': search_root + sub})
        write(search_root + '/a.txt', '开头\n错误1\n中间\n错误2\n结尾\n')
        write(search_root + '/sub/b.txt', '前\n错误3\n后\n')
        write(search_root + '/skip/c.txt', '错误4\n')
        fields = {'path': search_root, 'mode': 'content', 'query': '错误[1-4]', 'regex': True,
            'exclude': ['skip', 'skip/**'], 'context_lines': 1, 'max_results': 1}
        first = file('pab_file_search', fields)['result']['search']
        assert first['matches'][0]['line'] == 2 and len(first['matches'][0]['context']) == 3, first
        seen = []
        page = first
        while True:
            seen.extend((m['path'], m['line']) for m in page['matches'])
            cursor = page.get('next_cursor')
            if not cursor: break
            assert len(seen) < 10
            page = file('pab_file_search', {**fields, 'cursor': cursor})['result']['search']
        assert len(seen) == len(set(seen)) == 3 and all('/skip/' not in p for p, _ in seen), seen
        write(search_root + '/new.txt', '新文件\n')
        rejected('pab_file_search', {**fields, 'cursor': first['next_cursor']}, 'search_changed')
        rejected('pab_file_search', {**fields, 'query': '['}, 'invalid_regex')
        literal = file('pab_file_search', {'path': search_root, 'mode': 'content', 'query': '错误1'})
        assert len(literal['result']['search']['matches']) == 1
        passed('search_regex_excludes_context_stable_pages_changes_and_literal', matches=len(seen))
        rejected('pab_file_search', {**fields, 'glob': '['}, 'invalid_glob')
        shallow = file('pab_file_search', {**fields, 'max_depth': 1})['result']['search']
        assert shallow['truncated'] and not shallow.get('next_cursor'), shallow
        write(search_root + '/CaseNeedle.TXT', 'case\n')
        insensitive = file('pab_file_search', {'path': search_root, 'query': 'caseneedle', 'case_sensitive': False})['result']['search']
        assert len(insensitive['matches']) == 1
        write(search_root + '/budget.txt', ('错误 ' + '中' * 200 + '\n') * 24)
        budget = file('pab_file_search', {'path': search_root, 'mode': 'content', 'query': '错误',
            'context_lines': 5, 'max_results': 100})['result']['search']
        assert budget['truncated'] and budget.get('next_cursor'), budget
        assert len(json.dumps(budget, ensure_ascii=False).encode()) < 32768
        assert all(len(m['preview'] or '') <= 160 and all(len(c['text']) <= 160 for c in m.get('context', [])) for m in budget['matches'])
        passed('search_depth_limit_has_no_cursor_case_and_serialized_result_budget')

        for encoding in ('utf8', 'utf8_bom', 'utf16_le', 'utf16_be'):
            path = remote + '/preview-' + encoding + '.txt'
            original = '甲\r\nold😀 old\r\n'
            written = write(path, original, encoding=encoding)
            original_hash = written['result']['metadata']['sha256']
            fields = {'path': path, 'expected_hash': original_hash,
                'edits': [{'find': 'old', 'replace': '新', 'expected_matches': 2}]}
            preview_id = str(uuid.uuid4())
            preview = file('pab_file_patch', {**fields, 'dry_run': True, 'request_id': preview_id})['result']['patch_preview']
            assert preview['original_sha256'] == original_hash and preview['matched_edits'] == [2] and preview['changed'], preview
            unchanged = read(path)
            assert unchanged['text'] == original and unchanged['result']['metadata']['sha256'] == original_hash
            applied = file('pab_file_patch', {**fields, 'request_id': str(uuid.uuid4())})
            assert applied['result']['metadata']['sha256'] == preview['result_sha256'], applied
            assert read(path)['text'] == original.replace('old', '新')
            rejected('pab_file_patch', fields, 'hash_conflict')
            # A replay of the preview remains the original preview and cannot publish.
            replay = file('pab_file_patch', {**fields, 'dry_run': True, 'request_id': preview_id})
            assert replay['result']['patch_preview'] == preview
        passed('preview_and_apply_four_encodings_no_publication_conflicts_and_replay')
        path = remote + '/no-op.txt'
        original_hash = write(path, 'same')['result']['metadata']['sha256']
        preview = file('pab_file_patch', {'path': path, 'expected_hash': original_hash, 'dry_run': True,
            'edits': [{'find': 'same', 'replace': 'same'}]})['result']['patch_preview']
        assert not preview['changed'] and preview['result_sha256'] == original_hash
        passed('unchanged_preview')
        result['status'] = 'passed'
    except Exception as error:
        result['error'] = repr(error)
        raise
    finally:
        if client:
            try:
                for request in tasks.values(): call('pab_cancel_operation', {'operation_id': request}, failure=True)
                if result['cleanup_pending']:
                    home.set_paused(False)
                    file('pab_file_delete', {'path': remote, 'recursive': True})
                    result['cleanup_pending'] = False
                home.set_paused(True)
                result['final_state'] = normal_status(args.serial)
                call('pab_disconnect', {}, failure=True)
            except Exception as error:
                result['cleanup_error'] = repr(error); result['status'] = 'failed'
            client.close()
        result['finished_at_unix'] = time.time()
        (directory / 'result.json').write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding='utf-8')
        print(json.dumps({'result': str(directory / 'result.json'), 'status': result['status']}), flush=True)
    assert result['status'] == 'passed', result.get('cleanup_error')


if __name__ == '__main__':
    main()
