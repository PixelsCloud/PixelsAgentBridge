"""Exercise all five document tools against an installed Executor.

Copy acceptance/fixtures/document to the target, then run with a locally built
or installed MCP executable. Control/account environment variables follow the
normal MCP launcher; this script does not create a guest identity or secrets.
"""
import argparse
import json
import os
from pathlib import Path
import queue
import subprocess
import threading
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--mcp', required=True, type=Path)
parser.add_argument('--device-code', required=True)
parser.add_argument('--fixture-root', required=True, help='Absolute path on the target device')
parser.add_argument('--require-user', action='store_true', help='Also verify interactive user execution')
parser.add_argument('--stress', action='store_true', help='Also verify cancellation and system responsiveness using ocr-25-pages.pdf')
args = parser.parse_args()
EXE = args.mcp.resolve()
DEVICE = args.device_code
ROOT = args.fixture_root.rstrip('\\/')
SEPARATOR = '\\' if '\\' in ROOT else '/'
responses = queue.Queue()
env = os.environ.copy()
process = subprocess.Popen([str(EXE)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                           stderr=subprocess.PIPE, env=env,
                           creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0))


def reader():
    for line in process.stdout:
        try:
            responses.put(json.loads(line))
        except json.JSONDecodeError:
            responses.put({'invalid_line': line[:200].decode(errors='replace')})


threading.Thread(target=reader, daemon=True).start()
serial = 0


def request(method, params=None, timeout=30):
    global serial
    serial += 1
    key = serial
    message = {'jsonrpc': '2.0', 'id': key, 'method': method}
    if params is not None:
        message['params'] = params
    process.stdin.write((json.dumps(message) + '\n').encode())
    process.stdin.flush()
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            response = responses.get(timeout=min(1, max(.01, deadline - time.monotonic())))
        except queue.Empty:
            if process.poll() is not None:
                raise RuntimeError(f'MCP exited {process.returncode}: {process.stderr.read()[-1000:]}')
            continue
        if response.get('id') == key:
            if 'error' in response:
                raise RuntimeError(response['error'])
            return response['result']
    raise TimeoutError(f'MCP {method} timed out')


def tool(name, args, timeout=60):
    result = request('tools/call', {'name': name, 'arguments': args}, timeout)
    if result.get('isError'):
        raise RuntimeError(f'{name}: {result.get("content")}')
    value = result.get('structuredContent')
    if value is None:
        value = json.loads(result['content'][0]['text'])
    return value, result


def complete_open(filename, execution=None):
    arguments = {'device_code': DEVICE, 'path': ROOT + SEPARATOR + filename}
    if execution:
        arguments['execution'] = execution
    opened, _ = tool('pab_document_open', arguments)
    print('open', filename, opened.get('result', {}).get('state'), flush=True)
    ref = opened['operation_ref']['operation_id']
    for _ in range(40):
        if opened.get('result', {}).get('state') in ('completed', 'failed'):
            break
        opened, _ = tool('pab_get_operation', {'device_code': DEVICE, 'operation_id': ref,
                                               'wait_until': 'complete', 'wait_ms': 1000})
    else:
        raise TimeoutError(f'document {filename} did not finish')
    result = opened.get('result', {})
    if result.get('state') != 'completed':
        raise RuntimeError(f'{filename}: {opened}')
    document_ref = result['document_ref']
    print('complete', filename, result.get('data'), flush=True)
    return document_ref


try:
    initialized = request('initialize', {'protocolVersion': '2025-11-25',
        'capabilities': {}, 'clientInfo': {'name': 'document-acceptance', 'version': '1.0'}})
    process.stdin.write(b'{"jsonrpc":"2.0","method":"notifications/initialized"}\n')
    process.stdin.flush()
    listed = request('tools/list')
    names = {item['name'] for item in listed['tools']}
    assert {'pab_document_open', 'pab_document_read', 'pab_document_search',
            'pab_document_render_page', 'pab_document_close'} <= names
    connected, _ = tool('pab_connect', {'device_code': DEVICE, 'wait_ms': 5000})
    assert connected.get('connected'), connected
    contexts, _ = tool('pab_list_execution_contexts', {'device_code': DEVICE})
    users = [entry for entry in contexts.get('result', {}).get('data', {}).get('entries', [])
             if entry.get('mode') == 'user' and entry.get('selection') and not entry.get('unavailable_reason')]
    print('interactive user contexts', [entry['account_name'] for entry in users], flush=True)
    if args.require_user:
        assert users, 'No available logged-in user context'
    pdf_ref = complete_open('mixed.pdf')
    page, _ = tool('pab_document_read', {'device_code': DEVICE, 'document_ref': pdf_ref,
                                        'selector': {'kind': 'page', 'number': 2}})
    assert 'Invoice total 12345 USD' in page['result']['data']['text'], page
    assert '中文' in page['result']['data']['text'] and '合同' in page['result']['data']['text'], page
    matches, _ = tool('pab_document_search', {'device_code': DEVICE, 'document_ref': pdf_ref,
                                              'query': 'Invoice', 'limit': 5})
    assert matches['result']['data']['matches'][0]['location']['number'] == 2, matches
    rendered, raw = tool('pab_document_render_page', {'device_code': DEVICE,
        'document_ref': pdf_ref, 'page_number': 2, 'max_dimension': 800})
    assert rendered['result']['state'] == 'completed', rendered
    assert any(item['type'] == 'image' for item in raw['content']), 'MCP image missing'
    closed, _ = tool('pab_document_close', {'device_code': DEVICE, 'document_ref': pdf_ref})
    assert closed['result']['state'] == 'completed', closed
    sheet_ref = complete_open('sheets-formula.xlsx')
    sheet, _ = tool('pab_document_read', {'device_code': DEVICE, 'document_ref': sheet_ref,
                                         'selector': {'kind': 'sheet', 'name': 'Revenue'}})
    assert 'B2\tnumber\t42' in sheet['result']['data']['text'], sheet
    found, _ = tool('pab_document_search', {'device_code': DEVICE, 'document_ref': sheet_ref,
                                            'query': 'Widget'})
    assert found['result']['data']['matches'][0]['location']['address'] == 'A2', found
    tool('pab_document_close', {'device_code': DEVICE, 'document_ref': sheet_ref})
    docx_ref = complete_open('heading-table.docx')
    docx, _ = tool('pab_document_read', {'device_code': DEVICE, 'document_ref': docx_ref,
                                        'selector': {'kind': 'chunk', 'number': 1}})
    assert '风险评估' in docx['result']['data']['text'], docx
    table, _ = tool('pab_document_read', {'device_code': DEVICE, 'document_ref': docx_ref,
                                         'selector': {'kind': 'table', 'number': 1}})
    assert '设备\t42' in table['result']['data']['text'], table
    tool('pab_document_close', {'device_code': DEVICE, 'document_ref': docx_ref})
    slide_ref = complete_open('slides-notes.pptx')
    slide, _ = tool('pab_document_read', {'device_code': DEVICE, 'document_ref': slide_ref,
                                         'selector': {'kind': 'page', 'number': 1}})
    assert '讲者备注' in slide['result']['data']['text'], slide
    tool('pab_document_close', {'device_code': DEVICE, 'document_ref': slide_ref})
    image_ref = complete_open('scan-zh-en.png')
    image, _ = tool('pab_document_read', {'device_code': DEVICE, 'document_ref': image_ref,
                                         'selector': {'kind': 'page', 'number': 1}})
    assert 'Invoice total 12345 USD' in image['result']['data']['text'], image
    tool('pab_document_close', {'device_code': DEVICE, 'document_ref': image_ref})
    if args.require_user:
        user_ref = complete_open('native-table.pdf', users[0]['selection'])
        user_page, _ = tool('pab_document_read', {'device_code': DEVICE,
            'document_ref': user_ref, 'selector': {'kind': 'page', 'number': 1}})
        assert 'Risk Clause 7' in user_page['result']['data']['text'], user_page
        tool('pab_document_close', {'device_code': DEVICE, 'document_ref': user_ref})
    if args.stress:
        busy, _ = tool('pab_document_open', {'device_code': DEVICE,
                                            'path': ROOT + SEPARATOR + 'ocr-25-pages.pdf'})
        busy_id = busy['operation_ref']['operation_id']
        assert busy['result']['state'] == 'running', busy
        began = time.monotonic()
        info, _ = tool('pab_system_info', {'device_code': DEVICE, 'sample_cpu': False}, 15)
        elapsed = time.monotonic() - began
        assert info['result']['state'] == 'completed' and elapsed < 10, (elapsed, info)
        cancelled, _ = tool('pab_cancel_operation', {'device_code': DEVICE,
                                                    'operation_id': busy_id, 'kind': 'document_open'})
        assert cancelled['result']['state'] in ('cancel_requested', 'cancelled', 'completed'), cancelled
        for _ in range(20):
            result, _ = tool('pab_get_operation', {'device_code': DEVICE, 'operation_id': busy_id,
                                                  'kind': 'document_open', 'wait_until': 'complete', 'wait_ms': 1000})
            if result['result']['state'] in ('cancelled', 'completed', 'failed'):
                break
        else:
            raise TimeoutError('cancelled document worker did not stop')
        print('busy OCR worker left system_info responsive:', round(elapsed, 2), 's, terminal',
              result['result']['state'], flush=True)
    print('MCP document acceptance passed', flush=True)
finally:
    process.stdin.close()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.terminate()
        process.wait(timeout=5)
