"""Read, search and render maintained synthetic document fixtures on test hosts.

No packages are installed. Existing parser fixtures are shared with native unit
tests; all live-test code and private evidence stay under smoke/.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import time
import tomllib
import uuid

from mcp_client import McpClient, McpToolError


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ('lab', 'config', 'mcp', 'output'):
        parser.add_argument('--' + key, type=Path, required=True)
    parser.add_argument('--fixtures', type=Path, default=Path(__file__).resolve().parent.parent / 'acceptance/fixtures/document')
    args = parser.parse_args()
    files = ['native-table.pdf', 'native-zh-multipage.pdf', 'heading-table.docx', 'sheets-formula.xlsx',
             'slides-notes.pptx', 'scan-zh-en.png', 'encrypted.pdf', 'malformed.pdf']
    assert all((args.fixtures / name).is_file() for name in files)
    devices = tomllib.loads(args.lab.read_text(encoding='utf-8-sig'))['device']
    args.output.mkdir(parents=True, exist_ok=False)
    report = {'status': 'running', 'calls': [], 'checks': [], 'failures': [], 'cleanup': [],
              'fixtures': {name: hashlib.sha256((args.fixtures / name).read_bytes()).hexdigest() for name in files}}

    def save():
        (args.output / 'result.json').write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')

    env = {k: v for k, v in os.environ.items() if not k.startswith('PAB_')}
    with McpClient(args.mcp.resolve(), args.output / 'mcp.log', config=args.config.resolve(), env=env) as c:
        def call(tool, values, allow_error=False):
            entry = {'tool': tool, 'arguments': values, 'value': None}
            report['calls'].append(entry)
            save()
            try:
                value = c.call(tool, values, allow_error=allow_error)
                entry['value'] = value
                save()
                value = c.observe(value, timeout=260)
            except McpToolError as error:
                entry['value'] = error.value
                save()
                raise
            entry['value'] = value
            save()
            if not allow_error:
                assert not value.get('error'), tool
                assert value.get('result', {}).get('state') not in ('failed', 'cancelled'), tool + ' failed'
            return value

        def check(code, name):
            report['checks'].append({'device': code, 'name': name, 'passed': True})
            save()
            print(code + ' ' + name + ': passed', flush=True)

        try:
            for device in devices:
                code, family = device['code'], device['os']
                assert family in ('windows', 'macos')
                root = (r'C:\Windows\Temp\pab-doc-' if family == 'windows' else '/tmp/pab-doc-') + str(uuid.uuid4())
                created = False
                native_pdf_passed = False
                try:
                    connected = call('pab_connect', {'device_code': code, 'wait_ms': 1000})
                    deadline = time.monotonic() + 50
                    while connected.get('state') == 'connecting' and time.monotonic() < deadline:
                        connected = call('pab_connect', {'device_code': code, 'wait_ms': 1000})
                    assert connected.get('connected')
                    call('pab_mkdir', {'device_code': code, 'path': root})
                    created = True
                    for name in files:
                        reference = None
                        try:
                            path = root + '/' + name
                            call('pab_upload_file', {'device_code': code, 'source': str((args.fixtures / name).resolve()),
                                'destination': path, 'overwrite': False, 'wait_ms': 1000})
                            opened = call('pab_document_open', {'device_code': code, 'path': path,
                                'ocr_mode': 'auto' if name.endswith('.png') else 'off'}, allow_error=True)
                            result = opened.get('result', {})
                            reference = result.get('document_ref') or opened.get('operation_ref', {}).get('operation_id')
                            if name in ('encrypted.pdf', 'malformed.pdf'):
                                assert result.get('state') == 'failed', 'invalid/protected input was accepted'
                                if name == 'encrypted.pdf':
                                    assert 'password_required' in json.dumps(result), 'protected PDF error'
                                else:
                                    assert native_pdf_passed and result.get('error'), 'a working PDF parser is required before rejecting malformed input'
                                check(code, name + '_explicit_error')
                                continue
                            assert result.get('state') == 'completed' and reference, name + ': ' + str(result.get('error'))
                            document = {'device_code': code, 'document_ref': reference}
                            overview = call('pab_document_read', {**document, 'selector': {'kind': 'overview'}})
                            assert overview['result']['data'], 'empty overview'
                            if name == 'sheets-formula.xlsx':
                                body = call('pab_document_read', {**document, 'selector': {'kind': 'sheet', 'name': 'Revenue'}})
                                assert '42' in body['result']['data']['text'] and 'B2*2' in body['result']['data']['text']
                                term = 'Widget'
                            elif name == 'native-zh-multipage.pdf':
                                body = call('pab_document_read', {**document, 'selector': {'kind': 'page', 'number': 2}})
                                assert '中文风险条款' in body['result']['data']['text']
                                term = '中文'
                            elif name == 'heading-table.docx':
                                body = call('pab_document_read', {**document, 'selector': {'kind': 'table', 'number': 1}})
                                assert '设备' in body['result']['data']['text'] and '42' in body['result']['data']['text']
                                term = '付款'
                            elif name == 'slides-notes.pptx':
                                body = call('pab_document_read', {**document, 'selector': {'kind': 'page', 'number': 1}})
                                assert '讲者备注' in body['result']['data']['text']
                                term = '风险'
                            elif name == 'scan-zh-en.png':
                                body = call('pab_document_read', {**document, 'selector': {'kind': 'page', 'number': 1}})
                                assert '12345' in body['result']['data']['text'], 'OCR text'
                                term = 'Invoice'
                            else:
                                body = call('pab_document_read', {**document, 'selector': {'kind': 'page', 'number': 1}})
                                assert 'Risk Clause 7' in body['result']['data']['text']
                                term = 'Risk'
                            search = call('pab_document_search', {**document, 'query': term})
                            assert search['result']['data']['matches'], 'search missed known fixture text'
                            if name in ('native-table.pdf', 'scan-zh-en.png'):
                                values = {**document, 'page_number': 1, 'max_dimension': 512}
                                raw = c.rpc('tools/call', {'name': 'pab_document_render_page', 'arguments': values})
                                result = raw.get('structuredContent')
                                if result is None:
                                    result = json.loads(next(b['text'] for b in raw['content'] if b['type'] == 'text'))
                                report['calls'].append({'tool': 'pab_document_render_page', 'arguments': values, 'value': result})
                                assert not raw.get('isError'), 'render rejected'
                                result = c.observe(result, timeout=260)
                                assert result['result']['state'] == 'completed', 'render did not complete'
                                images = [base64.b64decode(b['data'], validate=True) for b in raw['content'] if b['type'] == 'image']
                                preview = result['result']['data']
                                if not images:
                                    # Async completion provides a remote artifact, not a fabricated image block.
                                    assert 0 < preview['size'] <= 8 * 1024 * 1024
                                    destination = args.output.resolve() / ('preview-' + str(uuid.uuid4()) + '.png')
                                    call('pab_download_file', {'device_code': code, 'source': preview['path'],
                                        'destination': str(destination), 'overwrite': False, 'wait_ms': 1000})
                                    images = [destination.read_bytes()]
                                assert all(b.startswith(b'\x89PNG\r\n\x1a\n') and len(b) == preview['size']
                                    and hashlib.sha256(b).hexdigest() == preview['sha256'] for b in images), 'preview transfer hash'
                                save()
                            check(code, name + '_read_search_content_and_preview')
                            if name == 'native-table.pdf':
                                native_pdf_passed = True
                        except Exception as error:
                            report['failures'].append({'device': code, 'fixture': name, 'error': repr(error)})
                            save()
                            print(code + ' ' + name + ': FAILED (see private result.json)', flush=True)
                        finally:
                            if reference:
                                call('pab_document_close', {'device_code': code, 'document_ref': reference})
                finally:
                    if created:
                        removed = call('pab_file_delete', {'device_code': code, 'path': root, 'recursive': True,
                            'max_entries': 12, 'max_depth': 1, 'max_bytes': 2 * 1024 * 1024})
                        report['cleanup'].append({'device': code, 'confirmed': removed['result']['state'] == 'completed'})
                    call('pab_disconnect', {'device_code': code})
                    save()
            report['status'] = 'failed' if report['failures'] else 'passed'
        except BaseException as error:
            report['status'] = 'failed'
            report['error'] = repr(error)
            raise
        finally:
            save()
    return 1 if report['status'] != 'passed' else 0


if __name__ == '__main__':
    raise SystemExit(main())
