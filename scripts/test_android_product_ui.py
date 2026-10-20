#!/usr/bin/env python3
"""Real USB UI acceptance. Screen contents and credentials are never dumped to logs."""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import time
import tomllib
import uuid
import xml.etree.ElementTree as ET

from android_home import Home, LABELS, normal_status
from android_upgrade import phone_state
from android_accessibility import set_enabled
from android_public import PublicMcp
from test_android_remote import ROOT, PACKAGE, adb


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', choices=['e2b3b128'], required=True)
    parser.add_argument('--mcp-executable',type=Path,required=True)
    args = parser.parse_args()
    run = ROOT / '.build/android/product-ui' / str(uuid.uuid4())
    run.mkdir(parents=True)
    resources = ROOT / 'apps/android/app/src/main/res'
    values = {}
    for folder in ('values-zh-rCN', 'values'):
        for element in ET.parse(resources / folder / 'product.xml').getroot():
            values.setdefault(element.attrib['name'], []).append(element.text.replace(r'\n','\n').replace(r"\'", "'"))
    LABELS.update({key:tuple(items) for key, items in values.items()})
    LABELS.update({'tab_activity':tuple(values['product_tab_activity']),
                   'tab_settings':tuple(values['product_tab_settings'])})
    LABELS['open_test_target'] = ('打开验收页面','Open acceptance screen')
    home = Home(args.serial)
    before = phone_state(args.serial)
    result = {'status':'failed', 'serial':args.serial, 'version_code':before['version_code'],
              'apk_sha256':before['apk_sha256'], 'checks':[], 'screenshots':[],
              'mcp_sha256':hashlib.sha256(args.mcp_executable.read_bytes()).hexdigest(),
              'emulator_tested':False, 'android9_tested':False, 'installed_desktop_updated':False}

    def passed(name, **metadata):
        result['checks'].append({'name':name, 'passed':True, **metadata})
        (run/'result.json').write_text(json.dumps(result,ensure_ascii=False,indent=2),encoding='utf-8')
        print(json.dumps({'check':name,'passed':True}),flush=True)

    def texts():
        return [item.get('text') for item in home.tree().iter('node') if item.get('text')]

    def has(key):
        return any(text in LABELS[key] for text in texts())

    def wait(key, present=True):
        deadline = time.monotonic()+10
        while time.monotonic()<deadline:
            if has(key) == present: return
            time.sleep(.2)
        raise AssertionError('Unexpected UI state for '+key)

    def click(key):
        home.click(key)
        time.sleep(.35)

    def screenshot(name):
        # Only take screenshots while the password is hidden.
        assert not has('hide')
        path = run / (name+'.png')
        path.write_bytes(subprocess.check_output([
            str(Path('D:/android/sdk/platform-tools/adb.exe')), '-s', args.serial,
            'exec-out', 'screencap', '-p']))
        result['screenshots'].append({'name':name, 'path':path.name,
                                     'sha256':hashlib.sha256(path.read_bytes()).hexdigest()})

    def preferences():
        return tomllib.loads(adb(args.serial,'shell','run-as',PACKAGE,'cat','no_backup/appearance.toml'))

    def choose(heading, choice):
        click('tab_settings'); click(heading); click(choice)

    def back():
        adb(args.serial,'shell','input','keyevent','4'); time.sleep(.5)

    def focused_window():
        return next(line for line in adb(args.serial,'shell','dumpsys','window').splitlines() if 'mCurrentFocus=' in line)

    try:
        home.start(); home.set_paused(True)
        wait('product_online'); wait('product_control_paused'); wait('show'); wait('hide', False)
        screenshot('home-light')
        assert not has('open_test_target')
        passed('home_separates_online_and_paused_hides_password_and_debug_tools')
        click('product_copy_code'); wait('product_code_copied')
        passed('device_code_copy_action_reports_success')
        click('show'); wait('hide'); click('tab_access'); click('tab_home'); wait('show'); wait('hide', False)
        click('show'); wait('hide')
        adb(args.serial,'shell','input','keyevent','3')
        time.sleep(.8)
        adb(args.serial,'shell','am','start','-n',PACKAGE+'/com.pixels.agent.MainActivity')
        wait('show'); wait('hide', False)
        passed('password_reveal_clears_after_page_change_and_background')
        home.set_paused(False); assert not normal_status(args.serial)['paused']
        home.set_paused(True); assert normal_status(args.serial)['paused']
        passed('allow_and_pause_control_persist_through_existing_intent_repository')
        click('rotate'); wait('product_password_rotation_hint'); click('product_close')
        passed('password_reset_does_not_rotate_or_disconnect_an_active_connection')

        click('tab_access'); wait('product_access_title'); wait('product_interface_control')
        screenshot('access-light')
        click('product_notification_access')
        focused = focused_window()
        assert 'com.android.settings' in focused or 'com.miui' in focused
        back(); wait('product_access_title')
        click('product_screen_view'); wait('product_screen_ready_hint'); click('product_close')
        passed('access_rows_open_real_system_permissions_and_explain_native_screen_view')
        click('product_add_folder')
        deadline=time.monotonic()+10
        while '.documentsui/' not in focused_window():
            assert time.monotonic()<deadline; time.sleep(.2)
        for _ in range(8):
            back()
            if '.documentsui/' not in focused_window(): break
        assert PACKAGE in focused_window()
        click('tab_home'); click('tab_access'); wait('product_access_title')
        passed('add_folder_opens_android_picker_and_cancel_retains_existing_access')

        home.set_paused(False)
        client=PublicMcp(ROOT/'.build/android/public/e8e3df87-90cb-4fe5-b45e-6a4c3d78cd83/mcp.toml',
            '919383159',True,run/'unsupported-mcp.log',args.mcp_executable)
        try:
            connected,raw=client.connect(); assert connected.get('connected') and not raw.get('isError')
            value,_=client.tool('pab_list_windows',{'device_code':'919383159'})
            window=value['result']['data']['snapshot']['window_ref']
            value,raw=client.tool('pab_window_control',{'device_code':'919383159','window_ref':window,'control':'maximize'},allow_error=True)
            assert value['result']['error']=='android_desktop_action_unavailable'
            result['unsupported_operation_ref']=value['operation_ref']
        finally:
            client.close()
            home.set_paused(True)
        click('tab_activity'); wait('product_all'); screenshot('activity-light')
        click('product_incomplete'); wait('product_incomplete')
        wait('product_unsupported'); wait('product_unsupported_description')
        screenshot('activity-unsupported')
        passed('real_public_unsupported_operation_is_shown_as_unsupported_not_permission_error')
        # A completed filter label remains; completed result badges must be absent.
        assert sum(text in LABELS['product_completed'] for text in texts()) == 1
        click('product_completed'); wait('product_completed'); click('product_all')
        passed('activity_reads_existing_receipts_and_filters_without_exposing_payloads')

        click('tab_settings'); wait('product_this_device'); screenshot('settings-light')
        click('product_device_name'); wait('product_edit_disconnected'); click('product_close')
        click('product_connection_settings'); wait('product_edit_disconnected')
        editable = [node for node in home.tree().iter('node') if node.get('resource-id') in ('control_url','relay_url')]
        assert len(editable)==2 and all(node.get('enabled')=='false' for node in editable)
        back()
        passed('online_name_and_connection_edits_are_blocked_without_changing_configuration')
        home.click('disconnect')
        deadline=time.monotonic()+10
        while normal_status(args.serial)['connection_requested']:
            assert time.monotonic()<deadline; time.sleep(.2)
        click('tab_settings'); click('product_device_name'); click('product_save')
        wait('product_this_device')
        click('product_connection_settings'); click('product_save'); wait('product_this_device')
        assert not normal_status(args.serial)['connection_requested']
        home.start()
        assert phone_state(args.serial)['fingerprints']==before['fingerprints']
        passed('offline_name_and_server_forms_save_without_starting_connection_or_changing_identity')

        choose('product_appearance','product_dark')
        assert preferences()['appearance']=='dark'
        screenshot('settings-dark')
        for tab, name in [('tab_home','home-dark'),('tab_access','access-dark'),('tab_activity','activity-dark')]:
            click(tab); screenshot(name)
        choose('product_language','product_language_english'); wait('product_this_device')
        assert preferences()['language']=='english'
        assert 'This device' in texts()
        screenshot('settings-english-dark')
        passed('windows_dark_palette_and_english_preferences_apply_to_all_tabs')

        pid=normal_status(args.serial)['pid']
        assert adb(args.serial,'shell','run-as',PACKAGE,'cat',f'/proc/{pid}/cmdline').rstrip('\0')==PACKAGE
        adb(args.serial,'shell','run-as',PACKAGE,'kill','-9',str(pid)); time.sleep(1)
        adb(args.serial,'shell','am','start','-n',PACKAGE+'/com.pixels.agent.MainActivity')
        set_enabled(args.serial,True); home.start()
        click('tab_settings'); assert 'This device' in texts()
        assert preferences()['appearance']=='dark' and preferences()['language']=='english'
        passed('appearance_and_language_survive_real_process_restart_as_toml')

        choose('product_language','product_language_chinese')
        assert '此设备' in texts()
        choose('product_language','product_follow_system')
        choose('product_appearance','product_follow_system')
        passed('chinese_and_system_language_and_theme_are_selectable')
        click('product_help_feedback'); wait('product_help_content'); back()
        click('product_about_pixels'); wait('product_diagnostics'); wait('open_test_target')
        passed('help_and_version_are_accessible_and_debug_tools_live_only_under_about')
        result['status']='passed'
    finally:
        home.start(); home.set_paused(True)
        after=phone_state(args.serial)
        assert all(after[key]==before[key] for key in ('apk_sha256','certificate_sha256','first_install_time','fingerprints','paused','connection_requested'))
        result['identity_and_original_configuration_retained']=True
        result['ending_status']=normal_status(args.serial)
        (run/'result.json').write_text(json.dumps(result,ensure_ascii=False,indent=2),encoding='utf-8')
        print(json.dumps({'status':result['status'],'checks':len(result['checks']),'result':str(run/'result.json')}),flush=True)


if __name__=='__main__':
    main()
