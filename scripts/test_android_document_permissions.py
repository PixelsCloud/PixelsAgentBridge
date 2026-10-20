#!/usr/bin/env python3
"""Verify cached document access against real read-only and revoked SAF grants.

Only the normal picker and home controls change grants. ADB creates/removes one
owned fixture folder; document operations themselves use authenticated PAB relay.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import subprocess
import sys
import time
import uuid

from android_home import Home, normal_status
from android_public import PublicMcp
from android_scenarios import AndroidTasks
from test_android_remote import ROOT, PACKAGE, adb, installed_apk_sha256


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--public-run-id', required=True, type=uuid.UUID)
    parser.add_argument('--device-code', required=True)
    parser.add_argument('--apk-sha256', required=True)
    parser.add_argument('--full-formats', action='store_true')
    parser.add_argument('--file-enhancements', action='store_true', help='Also run the v4 file cohort in this normally granted directory')
    parser.add_argument('--enhanced-documents', action='store_true', help='Include OCR and extended formats in the granted-directory cohort')
    parser.add_argument('--mcp-executable', required=True, type=Path)
    args = parser.parse_args()
    assert installed_apk_sha256(args.serial) == args.apk_sha256
    root = ROOT / '.build/android/documents' / ('permissions-' + str(uuid.uuid4()))
    root.mkdir(parents=True)
    folder = 'PixelsAcceptance_' + uuid.uuid4().hex[:8]
    physical = '/sdcard/Documents/' + folder
    sentinel = 'pixels-acceptance-' + uuid.uuid4().hex[:8] + '.txt'
    result = {'status':'failed', 'apk_sha256':args.apk_sha256, 'serial':args.serial,
        'folder':folder, 'physical_path':physical, 'sentinel':sentinel, 'forced_relay':True,
        'normal_picker_and_home':True, 'checks':[], 'physical_cleanup_pending':False}
    home = Home(args.serial)
    operator = ROOT / '.build/android/public' / str(args.public_run_id)
    client = None
    document_ref = None
    ocr_ref = None
    mount = None

    def connect(label):
        nonlocal client
        home.start(); home.set_paused(False)
        client = PublicMcp(operator / 'mcp.toml', args.device_code, True, root / (label + '.log'), args.mcp_executable)
        value, raw = client.connect()
        assert value.get('connected') and not raw.get('isError')

    def disconnect():
        nonlocal client
        if client is not None: client.close(); client = None

    def call(name, fields):
        value, raw = client.tool(name, {'device_code':args.device_code, **fields}, allow_error=True)
        with (root / 'operation-trace.jsonl').open('a', encoding='utf-8') as stream:
            reply = value.get('result', value)
            stream.write(json.dumps({'tool':name, 'operation_ref':value.get('operation_ref'),
                'state':reply.get('state'), 'error':reply.get('error'), 'is_error':bool(raw.get('isError'))}) + '\n')
        return value, raw

    def completed(name, fields):
        value, raw = call(name,fields)
        ref = value.get('operation_ref'); deadline = time.monotonic() + 90
        while value.get('result', {}).get('state') == 'running':
            assert time.monotonic() < deadline
            query = {'operation_id':ref['operation_id']}
            if ref['kind'].startswith('document_'): query['kind'] = ref['kind']
            value, raw = call('pab_get_operation',query)
            time.sleep(.1)
        assert not raw.get('isError') and value['result']['state'] == 'completed', value
        return value

    def helper(script, fields):
        process = subprocess.run([sys.executable,'-X','utf8',str(ROOT / 'scripts' / script),*fields],
            capture_output=True, text=True, encoding='utf-8')
        (root / (script + '-' + str(uuid.uuid4()) + '.log')).write_text(process.stdout + process.stderr, encoding='utf-8')
        assert process.returncode == 0, 'Owned picker/permission helper failed; inspect retained log'
        summary = json.loads(process.stdout.strip().splitlines()[-1])
        return summary

    try:
        connect('prepare')
        roots, _ = call('pab_list_directory',{'path':'/','limit':64})
        assert [entry['name'] for entry in roots.get('page',roots)['entries']] == ['workspace'], 'Preserve existing user grants'
        adb(args.serial,'shell','mkdir',physical)
        result['physical_cleanup_pending'] = True
        tasks = AndroidTasks(client,PACKAGE,root / 'picker-trace.jsonl')
        direction = 'forward'
        for attempt in range(24):
            screen = tasks.query(); assert screen['package_name'] == PACKAGE
            labels = [item['name'] for item in screen['elements'] if item.get('name') in ('授权文件目录','Authorize a folder')]
            if labels: tasks.act(labels[0]); break
            scrolls = tasks.query(role='android.widget.ScrollView')['elements']; assert len(scrolls) == 1
            outcome, _ = tasks.call({'action':'scroll','element_ref':scrolls[0]['element_ref'],
                'direction':direction},allow_pre_dispatch_errors=('target_stale','unsupported_action'))
            if outcome.get('error') == 'unsupported_action' or attempt == 11:
                direction = 'backward' if direction == 'forward' else 'forward'
        else: raise RuntimeError('Normal directory authorization control unavailable')
        disconnect()
        grant = helper('android_file_grant.py',['--public-run-id',str(args.public_run_id),
            '--folder',folder,'--mcp-executable',str(args.mcp_executable)])
        grant_result = json.loads(Path(grant['result']).read_text(encoding='utf-8'))
        mount = grant_result['mount']; result['mount'] = mount; result['grant_evidence'] = grant['result']
        if args.full_formats:
            formats = helper('test_android_documents.py',['--serial',args.serial,
                '--operator-run-id',str(args.public_run_id),'--device-code',args.device_code,
                '--apk-sha256',args.apk_sha256,'--mcp-executable',str(args.mcp_executable),'--mount',mount,
                *(['--enhanced-documents'] if args.enhanced_documents else [])])
            assert formats['status'] == 'passed'
            result['formats_evidence'] = str(ROOT / '.build/android/documents' / formats['run_id'] / 'result.json')
        if args.file_enhancements:
            enhanced = helper('test_android_file_enhancements.py', ['--serial', args.serial,
                '--operator-run-id', str(args.public_run_id), '--device-code', args.device_code,
                '--apk-sha256', args.apk_sha256, '--mcp-executable', str(args.mcp_executable), '--mount', mount])
            assert enhanced['status'] == 'passed'
            result['file_enhancements_evidence'] = enhanced['result']
        path = mount + '/' + sentinel
        text = 'Cached document consent 中文 ' + uuid.uuid4().hex
        connect('open')
        completed('pab_file_write',{'path':path,'content':text,'request_id':str(uuid.uuid4())})
        original = str(uuid.uuid4())
        opened = completed('pab_document_open',{'path':path,'request_id':original,'ocr_mode':'off'})
        document_ref = opened['result']['document_ref']
        read = completed('pab_document_read',{'document_ref':document_ref,'selector':{'kind':'chunk','number':1}})
        assert read['result']['data']['text'] == text
        cached_read = read['operation_ref']
        result.update(document_ref=document_ref, original_open=original, cached_read=cached_read)
        if args.enhanced_documents:
            ocr_sentinel = sentinel.removesuffix('.txt') + '-ocr.png'
            ocr_path = mount + '/' + ocr_sentinel
            transfer, raw = call('pab_upload_file', {'source':str(ROOT / 'acceptance/fixtures/document/scan-zh-en.png'),
                'destination':ocr_path,'overwrite':False,'request_id':str(uuid.uuid4())})
            assert not raw.get('isError'), transfer
            deadline = time.monotonic() + 120
            while True:
                value, _ = call('pab_get_operation', {'operation_id':transfer['operation_ref']['operation_id']})
                record = value.get('operation', {})
                state = value.get('state') or record.get('state') or record.get('operation', {}).get('state')
                if state in ('completed','failed','cancelled'):
                    assert state == 'completed', value
                    break
                assert time.monotonic() < deadline
                time.sleep(.1)
            ocr_original = str(uuid.uuid4())
            opened = completed('pab_document_open', {'path':ocr_path,'request_id':ocr_original,'ocr_mode':'auto'})
            ocr_ref = opened['result']['document_ref']
            read = completed('pab_document_read', {'document_ref':ocr_ref,'selector':{'kind':'page','number':1}})
            ocr_text = read['result']['data']['text']; ocr_read = read['operation_ref']
            assert '中文' in ocr_text and '12345' in ocr_text
            result.update(ocr_original_open=ocr_original,ocr_cached_read=ocr_read)
        disconnect()
        common = ['--serial',args.serial,'--public-run-id',str(args.public_run_id),'--mount',mount,
            '--folder',folder,'--read-path',path,'--apk-sha256',args.apk_sha256,
            '--mcp-executable',str(args.mcp_executable)]
        result['readonly_evidence'] = helper('android_file_permissions.py',['readonly',*common])['result']
        connect('readonly')
        read = completed('pab_document_read',{'document_ref':document_ref,'selector':{'kind':'chunk','number':1}})
        assert read['result']['data']['text'] == text
        fresh = completed('pab_document_open',{'path':path,'request_id':str(uuid.uuid4()),'ocr_mode':'off'})
        fresh_ref = fresh['result']['document_ref']
        fresh_read = completed('pab_document_read',{'document_ref':fresh_ref,'selector':{'kind':'chunk','number':1}})
        assert fresh_read['result']['data']['text'] == text
        completed('pab_document_close',{'document_ref':fresh_ref})
        if ocr_ref is not None:
            read = completed('pab_document_read', {'document_ref':ocr_ref,'selector':{'kind':'page','number':1}})
            assert read['result']['data']['text'] == ocr_text
            fresh = completed('pab_document_open', {'path':ocr_path,'request_id':str(uuid.uuid4()),'ocr_mode':'auto'})
            fresh_ref = fresh['result']['document_ref']
            read = completed('pab_document_read', {'document_ref':fresh_ref,'selector':{'kind':'page','number':1}})
            assert read['result']['data']['text'] == ocr_text
            completed('pab_document_close', {'document_ref':fresh_ref})
        result['checks'].append({'name':'cached_document_readable_with_readonly_grant','passed':True})
        disconnect()
        result['revoke_evidence'] = helper('android_file_permissions.py',['revoke',*common])['result']
        connect('revoked')
        for tool,fields in [
            ('pab_document_read',{'document_ref':document_ref,'selector':{'kind':'chunk','number':1}}),
            ('pab_document_open',{'path':path,'request_id':original,'ocr_mode':'off'}),
            ('pab_get_operation',{'operation_id':original,'kind':'document_open'}),
            ('pab_get_operation',{'operation_id':cached_read['operation_id'],'kind':'document_read'}),
        ]:
            value, _ = call(tool,fields)
            assert value['result']['state'] == 'failed' and value['result']['error'] == 'mount_not_found', value
            assert value['result']['data'] is None
        result['checks'].append({'name':'revoked_grant_denies_new_read_repeated_open_and_old_operation_payloads','passed':True})
        if ocr_ref is not None:
            for tool,fields in [
                ('pab_document_read',{'document_ref':ocr_ref,'selector':{'kind':'page','number':1}}),
                ('pab_document_open',{'path':ocr_path,'request_id':ocr_original,'ocr_mode':'auto'}),
                ('pab_get_operation',{'operation_id':ocr_original,'kind':'document_open'}),
                ('pab_get_operation',{'operation_id':ocr_read['operation_id'],'kind':'document_read'}),
            ]:
                value, _ = call(tool,fields)
                assert value['result']['state'] == 'failed' and value['result']['error'] == 'mount_not_found', value
                assert value['result']['data'] is None
            completed('pab_document_close', {'document_ref':ocr_ref}); ocr_ref = None
            result['checks'].append({'name':'readonly_ocr_reparse_and_revoked_ocr_cached_results_denied','passed':True})
        completed('pab_document_close',{'document_ref':document_ref}); document_ref = None
        # No grant remains: this is exact fixture cleanup through ADB, not evidence
        # of remote deletion. Regular remote deletion is covered by the main cohort.
        adb(args.serial,'shell','rm',physical + '/' + sentinel)
        if args.enhanced_documents:
            adb(args.serial,'shell','rm',physical + '/' + ocr_sentinel)
        adb(args.serial,'shell','rmdir',physical)
        adb(args.serial,'shell','test','!','-e',physical)
        result['physical_cleanup_pending'] = False
        result['physical_cleanup_is_not_remote_delete_evidence'] = True
        result['status'] = 'passed'
    except Exception as failure:
        result['error'] = str(failure)[:1200]
        raise
    finally:
        if client is not None and ocr_ref is not None:
            try: completed('pab_document_close',{'document_ref':ocr_ref})
            except Exception: pass
        if client is not None and document_ref is not None:
            try: completed('pab_document_close',{'document_ref':document_ref})
            except Exception: pass
        disconnect()
        home.start(); home.set_paused(True)
        result['final_state'] = normal_status(args.serial)
        (root / 'result.json').write_text(json.dumps(result,ensure_ascii=False,indent=2),encoding='utf-8')
        print(json.dumps({'status':result['status'],'result':str(root / 'result.json')}),flush=True)


if __name__ == '__main__': main()
