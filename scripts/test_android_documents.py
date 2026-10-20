#!/usr/bin/env python3
"""Exercise document tools over authenticated public PAB, on owned fixtures only."""
from __future__ import annotations

import argparse
import base64
import hashlib
import io
import json
from pathlib import Path
import time
import threading
import subprocess
import re
import uuid


from PIL import Image
from android_home import Home, normal_status
from android_public import PublicMcp
from android_scenarios import AndroidTasks
from test_android_remote import ROOT, PACKAGE, adb, installed_apk_sha256


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--operator-run-id', required=True, type=uuid.UUID)
    parser.add_argument('--device-code', required=True)
    parser.add_argument('--apk-sha256', required=True)
    parser.add_argument('--mcp-executable', required=True, type=Path)
    parser.add_argument('--mount', default='/workspace')
    parser.add_argument('--size-boundary', action='store_true')
    parser.add_argument('--fault-only', action='store_true')
    parser.add_argument('--fault-worker', action='store_true', help='Terminate only this APK isolated parser PID through ActivityManager')
    parser.add_argument('--enhanced-documents', action='store_true', help='Offline OCR, image previews and extended formats')
    parser.add_argument('--sheet-search-only', action='store_true', help='Focused ODS/legacy XLS cell-search acceptance')
    args = parser.parse_args()
    if args.fault_only and not args.fault_worker: parser.error('--fault-only requires --fault-worker')
    if args.sheet_search_only and (args.fault_worker or args.size_boundary or args.enhanced_documents):
        parser.error('--sheet-search-only is a separate focused cohort')
    assert installed_apk_sha256(args.serial) == args.apk_sha256
    assert args.mount.startswith('/') and '..' not in args.mount.split('/')
    run = ROOT / '.build/android/documents' / str(uuid.uuid4())
    run.mkdir(parents=True)
    operator = ROOT / '.build/android/public' / str(args.operator_run_id)
    result = {'status': 'failed', 'serial': args.serial, 'apk_sha256': args.apk_sha256,
              'mcp_sha256': hashlib.sha256(args.mcp_executable.read_bytes()).hexdigest(),
              'public_server': True, 'forced_relay': True, 'mount': args.mount,
              'checks': [], 'final_acceptance': False, 'started_at_unix': time.time()}
    home = Home(args.serial)
    client = None
    handles = set()
    remote = args.mount + '/pixels-documents-' + run.name
    result['test_root'] = remote
    result['cleanup_pending'] = False
    preview_paths = []

    def call(name, fields=None):
        fields = fields or {}
        value, raw = client.tool(name, {'device_code': args.device_code, **fields}, allow_error=True)
        data = value.get('result', value)
        with (run / 'operation-trace.jsonl').open('a', encoding='utf-8') as stream:
            stream.write(json.dumps({'tool': name, 'request_id': fields.get('request_id'),
                'operation_ref': value.get('operation_ref'), 'state': data.get('state'),
                'error': data.get('error'), 'is_error': bool(raw.get('isError'))}) + '\n')
        return value, raw

    def wait(value):
        ref = value['operation_ref']
        deadline = time.monotonic() + 120
        while value.get('result', {}).get('state') in ('running', 'cancel_requested'):
            assert time.monotonic() < deadline, 'Query original request, never replay'
            value, _ = call('pab_get_operation', {'operation_id': ref['operation_id'],
                'kind': ref['kind']})
            time.sleep(.1)
        return value['result']

    def document(name, fields=None, error=None):
        value, raw = call(name, fields)
        assert 'result' in value, (name, value.get('error'))
        reply = wait(value)
        if error is None:
            assert reply['state'] == 'completed' and not raw.get('isError'), (name, reply)
        else:
            assert reply['state'] == 'failed' and reply['error'] == error, (name, reply)
        return reply, raw

    def open_doc(path, error=None, request_id=None, ocr_mode='auto'):
        reply, _ = document('pab_document_open', {'path': path,
            'request_id': request_id or str(uuid.uuid4()), 'ocr_mode': ocr_mode}, error)
        if reply['state'] == 'completed': handles.add(reply['document_ref'])
        return reply

    def close(reference):
        document('pab_document_close', {'document_ref': reference})
        handles.discard(reference)

    def read(reference, selector):
        combined = ''; cursor = None; pages = 0
        while True:
            fields = {'document_ref': reference, 'selector': selector}
            if cursor is not None: fields['cursor'] = cursor
            reply, _ = document('pab_document_read', fields)
            combined += reply['data']['text']; pages += 1
            cursor = reply['data']['next_cursor']
            if cursor is None: return combined, pages

    def upload(source, destination):
        value, raw = call('pab_upload_file', {'source': str(source), 'destination': destination,
            'request_id': str(uuid.uuid4()), 'overwrite': False})
        assert not raw.get('isError'), value
        ref = value['operation_ref']; deadline = time.monotonic() + 300
        while True:
            value, _ = call('pab_get_operation', {'operation_id': ref['operation_id']})
            record = value.get('operation', {})
            state = value.get('state') or record.get('state') or record.get('operation', {}).get('state')
            if state in ('completed','failed','cancelled','unconfirmed','interrupted'):
                assert state == 'completed', value
                return
            assert time.monotonic() < deadline, 'Transfer still pending; query original operation ' + ref['operation_id']
            time.sleep(.1)

    def file_action(name, fields):
        value, raw = call(name, fields)
        assert not raw.get('isError'), value
        # File operation references are handled by their own queue, without a document kind.
        ref = value.get('operation_ref'); deadline = time.monotonic() + 120
        while value.get('result', {}).get('state') == 'running':
            assert time.monotonic() < deadline
            value, _ = call('pab_get_operation', {'operation_id': ref['operation_id']})
        assert value['result']['state'] == 'completed', value
        return value

    def passed(name, **metadata):
        result['checks'].append({'name': name, 'passed': True, **metadata})
        print(json.dumps({'check': name, 'passed': True}), flush=True)

    try:
        home.start(); home.set_paused(True)
        client = PublicMcp(operator / 'mcp.toml', args.device_code, True, run / 'mcp.log', args.mcp_executable)
        connected, raw = client.connect()
        assert connected.get('connected') and not raw.get('isError')
        catalog = client.rpc('tools/list', {})
        catalog_bytes = json.dumps(catalog, ensure_ascii=False, indent=2).encode('utf-8')
        (run / 'catalog.json').write_bytes(catalog_bytes)
        result['catalog_sha256'] = hashlib.sha256(catalog_bytes).hexdigest()
        result['catalog_tools'] = len(catalog['tools'])
        open_doc('/workspace/paused.txt', 'control_paused')
        passed('normal_pause_gate')
        home.set_paused(False)
        file_action('pab_mkdir', {'path': remote, 'parents': False, 'exist_ok': False})
        result['cleanup_pending'] = True
        if args.sheet_search_only:
            result['scope'] = 'ODS and legacy XLS cell search'
            fixtures = ROOT / 'acceptance/fixtures/document'
            for name, query, address in [('extended.ods','42','B1'),('sheets-formula.xls','逾期','B3')]:
                upload(fixtures / name, remote + '/' + name)
                opened = open_doc(remote + '/' + name); reference = opened['document_ref']
                assert opened['data']['sha256'] == hashlib.sha256((fixtures / name).read_bytes()).hexdigest()
                assert query in read(reference, {'kind':'sheet','name':'风险'})[0]
                found, _ = document('pab_document_search', {'document_ref':reference,'query':query})
                assert found['data']['matches'][0]['location'] == {'kind':'cell','sheet':'风险','address':address}, found
                close(reference)
                passed('cell_search_' + name, query=query, address=address)
            result['status'] = 'passed'
            return
        if args.fault_only:
            local = run / '笔记.md'; local.write_bytes('# 中文\n\n风险记录'.encode('utf-8'))
            upload(local, remote + '/笔记.md')
        else:
            fixtures = ROOT / 'acceptance/fixtures/document'
            for name in ('heading-table.docx','sheets-formula.xlsx','native-table.pdf',
                         'slides-notes.pptx','native-zh-multipage.pdf','cid-without-unicode.pdf','encrypted.pdf','malformed.pdf',
                         'scan-zh-en.pdf','scan-zh-en.png','oversized-expanded.docx'):
                upload(fixtures / name, remote + '/' + name)
            if args.enhanced_documents:
                for name in ('mixed.pdf','ocr-cancel.pdf','scan-zh-en.jpg','scan-exif.jpg','blank.png',
                             'oversized-raster.png','malformed.png','extended.odt','extended.ods',
                             'extended.odp','extended.rtf','extended.epub',
                             'heading-table.doc','sheets-formula.xls','slides-notes.ppt'):
                    upload(fixtures / name, remote + '/' + name)
            text = ('中文😀 风险 risk RISK\n' * 2000) + 'END'
            for name, body in [('分页.txt', text), ('笔记.md', '# 中文\n\n风险记录'),
                               ('页面.html', '<html><body><h1>中文标题</h1><p>风险 42</p></body></html>'),
                               ('数据.csv', 'Name,Amount\n风险,42\n')]:
                local = run / name; local.write_bytes(body.encode('utf-8'))
                upload(local, remote + '/' + name)
    
            original_id = str(uuid.uuid4())
            opened = open_doc(remote + '/分页.txt', request_id=original_id)
            reference = opened['document_ref']
            assert opened['data']['sha256'] == hashlib.sha256(text.encode()).hexdigest()
            body, count = read(reference, {'kind':'chunk','number':1})
            assert body == text and count > 1
            first, _ = document('pab_document_search', {'document_ref':reference,'query':'risk','limit':1})
            second, _ = document('pab_document_search', {'document_ref':reference,'query':'risk','limit':1,
                'cursor':first['data']['next_cursor']})
            second_offset = second['data']['matches'][0]['offset']
            assert second_offset > first['data']['matches'][0]['offset']
            assert text.encode('utf-8')[second_offset:second_offset + 4] == b'RISK'
            same = open_doc(remote + '/分页.txt', request_id=original_id)
            assert same['document_ref'] == reference
            open_doc(remote + '/笔记.md', 'request_conflict', original_id)
            file_action('pab_file_write', {'path':remote + '/分页.txt','content':'source changed',
                'overwrite':True,'encoding':'utf8','request_id':str(uuid.uuid4())})
            assert read(reference, {'kind':'chunk','number':1})[0] == text
            document('pab_document_read', {'document_ref':reference,'selector':{'kind':'chunk','number':1},
                'cursor':'2'}, 'invalid_cursor')
            close(reference)
            document('pab_document_read', {'document_ref':reference,'selector':{'kind':'chunk','number':1}},
                'document_expired_or_not_found')
            passed('utf8_paging_search_dedup_conflict_immutable_snapshot_and_close', pages=count)
    
            opened = open_doc(remote + '/heading-table.docx'); reference = opened['document_ref']
            assert opened['data']['tables'] >= 1
            table, _ = read(reference, {'kind':'table','number':1})
            assert '设备' in table and '42' in table
            assert '风险' in read(reference, {'kind':'chunk','number':1})[0]
            close(reference); passed('docx_chinese_headings_and_table')
    
            opened = open_doc(remote + '/sheets-formula.xlsx'); reference = opened['document_ref']
            assert '风险' in opened['data']['sheets']
            revenue, _ = read(reference, {'kind':'sheet','name':'Revenue'})
            assert 'B2*2' in revenue and '42' in revenue
            assert '逾期' in read(reference, {'kind':'sheet','name':'风险'})[0]
            found, _ = document('pab_document_search', {'document_ref':reference,'query':'逾期'})
            assert found['data']['matches'][0]['location'] == {'kind':'cell','sheet':'风险','address':'B3'}
            close(reference); passed('xlsx_sheet_absolute_cell_and_formula')
    
            opened = open_doc(remote + '/native-table.pdf'); reference = opened['document_ref']
            assert opened['data']['pages'] == 1
            assert 'Risk Clause' in read(reference, {'kind':'page','number':1})[0]
            found, _ = document('pab_document_search', {'document_ref':reference,'query':'Risk'})
            assert found['data']['matches'][0]['location'] == {'kind':'page','number':1}
            rendered, raw = document('pab_document_render_page', {'document_ref':reference,
                'page_number':1,'max_dimension':512})
            images = [item for item in raw.get('content', []) if item['type'] == 'image']
            assert len(images) == 1, 'Synchronous render must include verified MCP image'
            png = base64.b64decode(images[0]['data'], validate=True)
            assert hashlib.sha256(png).hexdigest() == rendered['data']['sha256']
            assert len(png) == rendered['data']['size']
            image = Image.open(io.BytesIO(png)); image.load()
            assert max(image.size) <= 512 and min(image.size) > 0
            minimum, maximum = image.convert('L').getextrema()
            assert minimum < maximum, 'PDF must contain rendered ink'
            (run / 'pdf-preview.png').write_bytes(png)
            preview_paths.append(rendered['data']['path'])
            close(reference); passed('pdf_page_search_and_verified_mcp_png', dimensions=image.size)
    
            opened = open_doc(remote + '/native-zh-multipage.pdf'); reference = opened['document_ref']
            assert opened['data']['pages'] == 2
            assert '中文风险条款' in read(reference, {'kind':'page','number':2})[0]
            found, _ = document('pab_document_search', {'document_ref':reference,'query':'风险'})
            assert found['data']['matches'][0]['location'] == {'kind':'page','number':2}
            close(reference); passed('multipage_pdf_chinese_text_and_location')
    
            opened = open_doc(remote + '/slides-notes.pptx'); reference = opened['document_ref']
            slide, _ = read(reference, {'kind':'page','number':1})
            assert '风险汇报' in slide and '讲者备注' in slide
            found, _ = document('pab_document_search', {'document_ref':reference,'query':'风险汇报'})
            assert found['data']['matches'][0]['location'] == {'kind':'slide','number':1}
            close(reference); passed('pptx_slide_and_speaker_notes')
    
            for name, expected in [('笔记.md','风险记录'),('页面.html','中文标题'),('数据.csv','风险')]:
                opened = open_doc(remote + '/' + name); reference = opened['document_ref']
                assert expected in read(reference, {'kind':'chunk','number':1})[0]
                close(reference)
            passed('markdown_html_csv')
    
            main_pid = normal_status(args.serial)["pid"]
            failures = [('encrypted.pdf','password_required'),('malformed.pdf','document_parse_failed'),
                        ('oversized-expanded.docx','document_resource_limit')]
            if not args.enhanced_documents:
                failures += [('cid-without-unicode.pdf','ocr_unavailable'),('scan-zh-en.pdf','ocr_unavailable'),('scan-zh-en.png','ocr_unavailable')]
            for name, error in failures:
                open_doc(remote + '/' + name, error)
            open_doc('/data/user/0/com.pixels.agent.debug/no_backup/credentials/private.txt', 'mount_not_found')
            open_doc(remote + '/bad.exe', 'unsupported_format')
            assert normal_status(args.serial)['pid'] == main_pid
            passed('protected_corrupt_zip_budget_and_private_path_errors', main_process_preserved=True)
            if args.enhanced_documents:
                for name in ('scan-zh-en.png','scan-zh-en.jpg','scan-exif.jpg','scan-zh-en.pdf'):
                    opened = open_doc(remote + '/' + name); reference = opened['document_ref']
                    assert [page['number'] for page in opened['data']['ocr_pages']] == [1], (name,opened)
                    body, _ = read(reference, {'kind':'page','number':1})
                    assert '中文' in body and '风险' in body and '12345' in body, (name,body)
                    found, _ = document('pab_document_search', {'document_ref':reference,'query':'12345'})
                    assert found['data']['matches'][0]['location'] == {'kind':'page','number':1}
                    rendered, raw = document('pab_document_render_page', {'document_ref':reference,'page_number':1,'max_dimension':512})
                    images = [item for item in raw.get('content', []) if item['type'] == 'image']
                    assert len(images) == 1, (name,raw)
                    png = base64.b64decode(images[0]['data'], validate=True)
                    assert hashlib.sha256(png).hexdigest() == rendered['data']['sha256']
                    image = Image.open(io.BytesIO(png)); image.load()
                    assert max(image.size) <= 512 and image.convert('L').getextrema()[0] < 200
                    preview_paths.append(rendered['data']['path'])
                    (run / (name + '-ocr.txt')).write_text(body,encoding='utf-8')
                    (run / (name + '-preview.png')).write_bytes(png)
                    close(reference)
                passed('offline_chinese_latin_png_jpeg_exif_scanned_pdf_and_verified_previews')
                for name in ('mixed.pdf','cid-without-unicode.pdf'):
                    opened = open_doc(remote + '/' + name); reference = opened['document_ref']
                    assert opened['data']['pages'] == 2
                    assert [page['number'] for page in opened['data']['ocr_pages']] == [2], (name,opened)
                    first = read(reference, {'kind':'page','number':1})[0]
                    second = read(reference, {'kind':'page','number':2})[0]
                    assert ('Risk Clause' in first if name == 'mixed.pdf' else 'First page' in first)
                    assert '中文' in second and '风险' in second, second
                    found, _ = document('pab_document_search', {'document_ref':reference,'query':'风险'})
                    assert found['data']['matches'][0]['location'] == {'kind':'page','number':2}
                    close(reference)
                forced = open_doc(remote + '/native-table.pdf',ocr_mode='on'); reference=forced['document_ref']
                assert 'Risk' in read(reference, {'kind':'page','number':1})[0]
                close(reference)
                for name in ('scan-zh-en.pdf','scan-zh-en.png'):
                    open_doc(remote + '/' + name,'no_extractable_text',ocr_mode='off')
                open_doc(remote + '/blank.png','no_extractable_text')
                for name in ('malformed.png','oversized-raster.png'):
                    open_doc(remote + '/' + name,'ocr_recognition_failed')
                passed('mixed_cid_pdf_page_locations_force_on_off_blank_and_decode_bounds')
                for name,expected in [('extended.odt','中文风险'),('extended.odp','中文风险'),
                                      ('extended.rtf','中文风险'),('extended.epub','中文风险'),
                                      ('heading-table.doc','风险'),('slides-notes.ppt','风险')]:
                    opened = open_doc(remote + '/' + name); reference = opened['document_ref']
                    assert expected in read(reference, {'kind':'chunk','number':1})[0], name
                    found, _ = document('pab_document_search', {'document_ref':reference,'query':expected})
                    assert found['data']['matches'], name
                    close(reference)
                for name in ('extended.ods','sheets-formula.xls'):
                    opened = open_doc(remote + '/' + name); reference=opened['document_ref']
                    assert '风险' in opened['data']['sheets'], (name,opened)
                    sheet = read(reference, {'kind':'sheet','name':'风险'})[0]
                    assert ('42' in sheet and '84' in sheet) if name.endswith('.ods') else '逾期' in sheet
                    close(reference)
                passed('odf_rtf_epub_legacy_doc_xls_ppt_read_search_and_ods_cells')
                assert normal_status(args.serial)['pid'] == main_pid
                original = str(uuid.uuid4())
                fields = {'path':remote + '/ocr-cancel.pdf','request_id':original,'ocr_mode':'auto'}
                queued, _ = call('pab_document_open',fields)
                assert queued['result']['state'] == 'running', queued
                mobile = AndroidTasks(client,PACKAGE,run / 'ocr-control-trace.jsonl')
                latencies = []
                for _ in range(2):
                    started = time.monotonic(); observed = mobile.query()
                    latency = time.monotonic() - started
                    assert latency < 5 and observed['package_name'] == PACKAGE, (latency,observed)
                    latencies.append(latency)
                cancelled, _ = call('pab_cancel_operation', {'operation_id':original,'kind':'document_open'})
                outcome = wait(cancelled)
                assert outcome['state'] == 'cancelled' and outcome['data'] is None and outcome['document_ref'] is None, outcome
                duplicate, _ = call('pab_document_open',fields)
                assert wait(duplicate)['state'] == 'cancelled'
                opened = open_doc(remote + '/scan-zh-en.png'); reference=opened['document_ref']
                assert '12345' in read(reference,{'kind':'page','number':1})[0]
                close(reference)
                assert normal_status(args.serial)['pid'] == main_pid
                passed('active_ocr_observation_cancel_discards_result_dedup_and_next_open',
                    operation_id=original,maximum_observation_s=max(latencies),main_process_preserved=True)
        main_pid = normal_status(args.serial)['pid']
        if args.fault_worker:
            load = run / '取消.txt'
            load.write_bytes((b'bounded document cancellation\n' * 300000)[:8 * 1024 * 1024])
            upload(load, remote + '/取消.txt')
            queued, _ = call('pab_document_open', {'path':remote + '/取消.txt',
                'request_id':str(uuid.uuid4()), 'ocr_mode':'off'})
            ref = queued['operation_ref']
            cancelled, _ = call('pab_cancel_operation', {'operation_id':ref['operation_id'],'kind':ref['kind']})
            outcome = wait(cancelled)
            assert outcome['state'] == 'cancelled', outcome
            passed('cancel_original_open_discards_result', operation_id=ref['operation_id'])

            from openpyxl import Workbook
            workbook = Workbook(write_only=True); sheet = workbook.create_sheet('Fault')
            for _ in range(10000): sheet.append(['isolated parser fault case'] * 10)
            heavy = run / 'fault-workbook.xlsx'; workbook.save(heavy)
            upload(heavy, remote + '/fault-workbook.xlsx')
            fault = {}
            stop = threading.Event()
            stamp = adb(args.serial,'shell',"date '+%m-%d %H:%M:%S'").strip() + '.000'
            logcat = subprocess.Popen(['adb','-s',args.serial,'logcat','-v','brief','-T',stamp,
                '-s','PixelsDocumentWorker:I','*:S'],stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,
                text=True,encoding='utf-8',errors='replace')
            def terminate_worker():
                try:
                    for line in logcat.stdout:
                        if stop.is_set(): return
                        found = re.search(r'job_started pid=(\d+) uid=(\d+) action=extract',line)
                        if not found: continue
                        pid, uid = map(int,found.groups())
                        assert pid != main_pid and uid % 100000 >= 90000
                        services = adb(args.serial,'shell','dumpsys','activity','services',PACKAGE)
                        (run / 'fault-service-metadata.txt').write_text(services,encoding='utf-8')
                        assert 'DocumentWorkerService' in services and f'{pid}:' in services, 'Verify owned service before fault injection'
                        fault.update(worker_pid=pid,worker_uid=uid,owned_service_confirmed=True)
                        fault['output'] = adb(args.serial,'shell','am','crash','--user','0',str(pid))[-400:]
                        fault['injected'] = True
                        return
                except Exception as failure: fault['error'] = str(failure)[:300]
            watcher = threading.Thread(target=terminate_worker, daemon=True)
            watcher.start()
            crash_id = str(uuid.uuid4())
            try:
                queued, _ = call('pab_document_open', {'path':remote + '/fault-workbook.xlsx',
                    'request_id':crash_id,'ocr_mode':'off'})
                crashed = wait(queued)
                if crashed.get('state') == 'completed': handles.add(crashed['document_ref'])
            finally:
                stop.set(); logcat.terminate(); watcher.join(timeout=20)
                logcat.wait(timeout=5)
            assert fault.get('injected') and crashed['state'] == 'failed', (fault,crashed)
            assert crashed['error'] == 'document_worker_died_or_limit', crashed
            duplicate, _ = document('pab_document_open', {'path':remote + '/fault-workbook.xlsx',
                'request_id':crash_id,'ocr_mode':'off'}, 'document_worker_died_or_limit')
            assert duplicate['request_id'] == crash_id
            assert normal_status(args.serial)['pid'] == main_pid
            info, _ = call('pab_system_info')
            assert info['result']['data']['info']['executor']['pid'] == main_pid
            opened = open_doc(remote + '/笔记.md'); reference = opened['document_ref']
            assert '风险记录' in read(reference, {'kind':'chunk','number':1})[0]
            close(reference)
            passed('isolated_worker_death_preserves_connection_and_next_open', **fault,
                   operation_id=crash_id, main_pid=main_pid)
        if args.size_boundary:
            large = run / 'oversized.txt'
            with large.open('wb') as stream:
                block = b'x' * (1024 * 1024)
                for _ in range(33): stream.write(block)
            upload(large, remote + '/oversized.txt')
            open_doc(remote + '/oversized.txt', 'file_size_limit')
            passed('input_33mib_rejected_before_parser')
        for preview in preview_paths:
            value, _ = call('pab_file_stat', {'path':preview})
            assert value.get('result', {}).get('error', {}).get('code') == 'not_found', value
        passed('close_removes_own_preview_files')
        result['status'] = 'passed'
    except Exception as failure:
        result['error'] = str(failure)[:1200]
        raise
    finally:
        if client is not None:
            try:
                for reference in list(handles): close(reference)
                if result['cleanup_pending']:
                    file_action('pab_file_delete', {'path':remote,'recursive':True,
                        'request_id':str(uuid.uuid4())})
                    result['cleanup_pending'] = False
            except Exception as failure:
                result['cleanup_error'] = str(failure)[:600]
                result['status'] = 'failed'
            client.close()
        try: home.start(); home.set_paused(True)
        except Exception as failure:
            result['restore_error'] = str(failure)[:400]; result['status'] = 'failed'
        result['final_state'] = normal_status(args.serial)
        result['completed_at_unix'] = time.time()
        (run / 'result.json').write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding='utf-8')
        print(json.dumps({'run_id':run.name,'status':result['status'],'checks':len(result['checks'])}), flush=True)
        if result['status'] != 'passed': raise RuntimeError('Document acceptance incomplete; inspect ' + str(run))


if __name__ == '__main__': main()
