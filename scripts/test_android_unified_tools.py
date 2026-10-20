#!/usr/bin/env python3
"""USB target setup only; unified UI and notification acceptance use authenticated public relay."""
from __future__ import annotations
import argparse,base64,hashlib,io,json,time,uuid
from pathlib import Path
from PIL import Image
from android_home import Home,normal_status
from android_upgrade import phone_state
from android_public import PublicMcp
from test_android_remote import ROOT,PACKAGE,adb


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial',choices=['e2b3b128'],required=True)
    parser.add_argument('--mcp-executable',type=Path,required=True)
    parser.add_argument('--prepare',action='store_true')
    args=parser.parse_args()
    root=ROOT/'.build/android/unified-notifications';run=root/str(uuid.uuid4());run.mkdir(parents=True)
    before=phone_state(args.serial)
    result={'status':'failed','serial':args.serial,'version_code':before['version_code'],'apk_sha256':before['apk_sha256'],'mcp_sha256':hashlib.sha256(args.mcp_executable.read_bytes()).hexdigest(),'forced_relay':True,'checks':[],'whole_android_accepted':False}
    home=Home(args.serial);client=None;window=None

    def passed(name,**fields):
        result['checks'].append({'name':name,'passed':True,**fields})
        (run/'result.json').write_text(json.dumps(result,ensure_ascii=False,indent=2),encoding='utf-8')
        print(json.dumps({'check':name,'passed':True}),flush=True)

    def connect():
        nonlocal client
        client=PublicMcp(ROOT/'.build/android/public/e8e3df87-90cb-4fe5-b45e-6a4c3d78cd83/mcp.toml','919383159',True,run/('mcp-'+uuid.uuid4().hex+'.log'),args.mcp_executable)
        value,raw=client.connect();assert value.get('connected') and not raw.get('isError')

    def call(name,fields=None,error=None,poll=True,allow_stale=False):
        value,raw=client.tool(name,{'device_code':'919383159',**(fields or {})},allow_error=True)
        deadline=time.monotonic()+120
        while poll and value.get('result',{}).get('state')=='running':
            assert time.monotonic()<deadline,'Query original operation, never replay'
            value,raw=client.tool('pab_get_operation',{'device_code':'919383159','operation_id':value['operation_ref']['operation_id'],'wait_ms':1000},allow_error=True)
        reply=value.get('result',{})
        code=reply.get('error') or value.get('error')
        if isinstance(code,dict):code=code.get('code')
        with (run/'trace.jsonl').open('a',encoding='utf-8') as stream:stream.write(json.dumps({'tool':name,'request_id':(fields or {}).get('request_id'),'operation_ref':value.get('operation_ref'),'state':reply.get('state'),'error':code})+'\n')
        if allow_stale and code=='target_stale' and reply.get('state')=='failed':
            return value,raw
        if error:
            assert code==error,(name,code)
        else:
            assert not raw.get('isError'),(name,value.get('error'))
            if poll and reply:assert reply['state']=='completed',(name,reply.get('error'),reply.get('state'))
        return value,raw

    def desktop(name,fields=None,error=None):
        value,_=call(name,fields,error)
        return None if error else value['result']['data']['snapshot']
    def windows():
        nonlocal window
        snapshot=desktop('pab_list_windows');assert snapshot['backend']=='android_accessibility'
        assert len(snapshot['windows'])==1 and snapshot['windows'][0]['focused']
        window=snapshot['windows'][0]['window_ref'];return snapshot
    def query(selector=None,**extra):
        snapshot=desktop('pab_ui_query',{'scope':{'type':'window','window_ref':window},'selector':selector or {},'limits':{'limit':128,'max_depth':12,'max_visited':2000,'timeout_ms':3000},**extra})
        return snapshot['ui']
    def find(name):
        ui=query({'name':name});assert len(ui['elements'])==1,(name,len(ui['elements']))
        return ui['elements'][0]
    def get(name,include_value=False):
        for _ in range(3):
            node=find(name)
            value,_=call('pab_ui_get',{'element_ref':node['element_ref'],'include_value':include_value},allow_stale=True)
            reply=value['result']
            if reply.get('state')=='failed' and reply.get('error')=='target_stale':
                time.sleep(.2);continue
            return reply['data']['snapshot']['ui']['elements'][0]
        raise AssertionError('Read target stayed stale')
    def action(name,command=None,expected_rejection=None,**extra):
        # Retry only explicit stale-target rejection before dispatch, never unknown outcomes.
        for _ in range(3):
            node=find(name)
            value,_=call('pab_ui_action',{'element_ref':node['element_ref'],'action':command or {'type':'invoke'},**extra},poll=True,allow_stale=True)
            reply=value['result'];ui=(reply.get('data') or {}).get('snapshot',{}).get('ui')
            if reply.get('state')=='failed' and reply.get('error')=='target_stale':
                time.sleep(.2);continue
            if ui and ui['outcome']=='completed':assert ui['verification'] in ('native_returned','matched');return ui
            if ui and ui['outcome']=='rejected' and ui['error_code']==expected_rejection and ui['action_dispatched'] is False:return ui
            if ui and ui['outcome']=='rejected' and ui['error_code']=='target_stale' and ui['action_dispatched'] is False:continue
            raise AssertionError(('Input outcome must not be replayed',reply.get('error'),ui))
        raise AssertionError('Target stayed stale')
    def wait_text(text):
        ui=desktop('pab_ui_wait',{'scope':{'type':'window','window_ref':window},'selector':{'name_contains':text},'condition':{'type':'exists'},'timeout_ms':5000,'poll_ms':200})['ui']
        assert ui['outcome']=='matched';return ui
    def notifications():
        value,_=call('pab_list_notifications',{'package_id':PACKAGE,'limit':32})
        return value['result']['data']['snapshot']
    def notice(snapshot,text):
        return next(n for n in snapshot['entries'] if n['title']=='Pixels QA '+run.name and n['text']==text)
    def wait_notice(text,after_revision=None):
        deadline=time.monotonic()+5
        while time.monotonic()<deadline:
            sample=notifications()
            rows=[n for n in sample['entries'] if n['title']=='Pixels QA '+run.name and n['text']==text]
            if rows and (after_revision is None or sample['revision']>after_revision):return sample,rows[0]
            time.sleep(.15)
        raise AssertionError('Own notification not observed: '+text)
    def fixture():
        # Only prepare this own debug target; this is not proof of a remote action.
        adb(args.serial,'shell','am','start','-n',PACKAGE+'/com.pixels.agent.fixture.UnifiedToolsFixtureActivity','--es','qa_run',run.name)
        time.sleep(.6);windows();wait_text('unified_fixture:'+run.name)

    try:
        home.start();home.set_paused(True);connect()
        catalog=client.rpc('tools/list',{});assert len(catalog['tools'])==121
        (run/'catalog.json').write_text(json.dumps(catalog,ensure_ascii=False,indent=2),encoding='utf-8')
        call('pab_notification_status',error='control_paused')
        call('pab_list_windows',error='control_paused')
        home.set_paused(False)
        value,_=call('pab_notification_status');status=value['result']['data']['snapshot']
        result['notification_status']=status
        passed('catalog_121_and_shared_pause_gates')
        if args.prepare:
            result['status']='prepared' if status['access_granted'] and status['listener_connected'] else 'permission_required'
            return
        assert status['access_granted'] and status['listener_connected'],'Finish ordinary notification access setup'
        fixture();screen=query();assert screen['elements'] and screen['visited_count']<=2000
        identifier=next((n['identifier'] for n in screen['elements'] if n.get('identifier')),None)
        assert identifier and query({'identifier':identifier})['elements']
        assert desktop('pab_list_monitors')['monitors'][0]['id']==0
        windows()
        focused=desktop('pab_focus_window',{'window_ref':window})
        assert focused['window_ref']==window and not focused['action_started'] and focused['verification']=='matched'
        passed('active_window_display_query_focus_and_native_identifiers')
        action('unified_input',{'type':'focus'})
        desktop('pab_type_text',{'window_ref':window,'text':'跨平台统一输入-'+run.name})
        got=get('unified_input',True)
        assert got['value']=='跨平台统一输入-'+run.name
        assert get('unified_input')['value'] is None
        passed('focused_type_text_explicit_value_get_and_default_redaction')
        desktop('pab_type_text',{'window_ref':window,'text':'-追加'})
        assert get('unified_input',True)['value']=='跨平台统一输入-'+run.name+'-追加'
        passed('type_text_inserts_at_caret_and_preserves_existing_content')
        action('unified_input',{'type':'set_value','value':'abCDyz'})
        action('unified_select_range');wait_text('selection_ready')
        desktop('pab_type_text',{'window_ref':window,'text':'中'})
        assert get('unified_input',True)['value']=='ab中yz'
        passed('type_text_replaces_selected_range_and_preserves_prefix_suffix')
        action('unified_input',{'type':'set_value','value':'统一控件设置'})
        ui=desktop('pab_ui_wait',{'scope':{'type':'window','window_ref':window},'selector':{'name':'unified_input'},'condition':{'type':'value_equals','value':'统一控件设置'},'timeout_ms':2000,'poll_ms':200})['ui'];assert ui['outcome']=='matched'
        node=find('unified_save');request=str(uuid.uuid4());fields={'element_ref':node['element_ref'],'action':{'type':'invoke'},'request_id':request}
        first,_=call('pab_ui_action',fields);assert first['result']['data']['snapshot']['ui']['outcome']=='completed'
        call('pab_ui_action',fields);wait_text('unified_saved:1:统一控件设置')
        passed('ui_set_value_wait_and_mutation_dedup',request_id=request)
        ui=action('unified_check',{'type':'set_checked','checked':True},expected_rejection='precondition_failed',expected={'checked':'on'});assert ui['outcome']=='rejected' and ui['error_code']=='precondition_failed' and ui['action_dispatched'] is False
        action('unified_check',{'type':'set_checked','checked':True});assert find('unified_check')['checked']=='on'
        passed('atomic_expected_state_and_native_checkbox_action')
        stale=find('unified_save')['element_ref'];query()
        desktop('pab_ui_action',{'element_ref':stale,'action':{'type':'invoke'}},error='target_stale')
        ui=desktop('pab_ui_wait',{'scope':{'type':'window','window_ref':window},'selector':{'name':'definitely-absent-'+run.name},'condition':{'type':'exists'},'timeout_ms':300,'poll_ms':100})['ui'];assert ui['outcome']=='timed_out'
        passed('stale_ui_reference_rejected_and_bounded_wait_timeout')
        value,raw=call('pab_capture_screenshot',{'window_ref':window,'include_image':True})
        encoded=next(c for c in raw['content'] if c['type']=='image');data=base64.b64decode(encoded['data'])
        assert hashlib.sha256(data).hexdigest()==value['sha256']
        assert value['preview_to_desktop']['coordinate_space']=='android_screen_pixels' and value['source_frame_format']=='jpeg'
        image=Image.open(io.BytesIO(data));image.load();assert image.size==(value['width'],value['height'])
        (run/'screenshot.jpg').write_bytes(data)
        passed('shared_screenshot_wire_jpeg_hash_dimensions_and_window_mapping',width=value['width'],height=value['height'])
        cropped,_=call('pab_capture_screenshot',{'monitor_id':0,'region':{'x':10,'y':20,'width':128,'height':128},'include_image':False,'destination':str(run/'crop.jpg')})
        assert (cropped['width'],cropped['height'])==(128,128)
        assert cropped['preview_to_desktop']['origin_x']==10 and cropped['preview_to_desktop']['origin_y']==20
        assert hashlib.sha256((run/'crop.jpg').read_bytes()).hexdigest()==cropped['sha256']
        passed('screenshot_crop_preserves_screen_pixel_origin')
        adb(args.serial,'shell','am','start','-n',PACKAGE+'/com.pixels.agent.fixture.AutomationFixtureActivity','--ez','secure_fixture','true')
        time.sleep(.5);windows();protected=query()
        assert 'fixture-only-secret' not in json.dumps(protected)
        secure=next(n for n in protected['elements'] if n['protected'])
        assert secure['name'] is None and not secure['supported_actions']
        got=desktop('pab_ui_get',{'element_ref':secure['element_ref'],'include_value':True})['ui']['elements'][0];assert got['value'] is None
        ui=desktop('pab_ui_action',{'element_ref':secure['element_ref'],'action':{'type':'set_value','value':'must-not-be-written'}})['ui'];assert ui['outcome']=='rejected' and ui['action_dispatched'] is False
        rejected,raw=client.tool('pab_capture_screenshot',{'device_code':'919383159','window_ref':window},allow_error=True)
        assert raw.get('isError') and not any(c.get('type')=='image' for c in raw.get('content',[]))
        fixture();passed('protected_control_redacted_and_secure_window_capture_rejected')
        action('notification_post');sample,entry=wait_notice('v1');assert entry['clearable'] and entry['can_open']
        passed('real_notification_sample_for_explicit_app',revision=sample['revision'])
        old=entry['notification_ref'];action('notification_update');updated,_=wait_notice('v2',sample['revision']);assert notice(updated,'v2') and updated['revision']>sample['revision']
        call('pab_open_notification',{'notification_ref':old},error='notification_reference_expired')
        passed('notification_update_invalidates_old_reference')
        sample=notifications();action('notification_delayed')
        value,_=call('pab_wait_notification',{'package_id':PACKAGE,'after_revision':sample['revision'],'timeout_ms':10000,'limit':32});changed=value['result']['data']['snapshot'];assert changed['outcome']=='changed';notice(changed,'delayed')
        passed('wait_observes_real_delayed_notification')
        entry=notice(notifications(),'v2')
        value,_=call('pab_open_notification',{'notification_ref':entry['notification_ref'],'request_id':str(uuid.uuid4())});assert value['result']['data']['snapshot']['outcome']=='dispatched'
        windows();wait_text('notification_opened:'+run.name)
        passed('notification_content_intent_dispatch_and_observed_target')
        entry=notice(notifications(),'v2');value,_=call('pab_dismiss_notification',{'notification_ref':entry['notification_ref'],'request_id':str(uuid.uuid4())});assert value['result']['data']['snapshot']['outcome']=='removed'
        assert not [n for n in notifications()['entries'] if n['title']=='Pixels QA '+run.name and n['text']=='v2']
        call('pab_dismiss_notification',{'notification_ref':entry['notification_ref']},error='notification_reference_expired')
        passed('dismiss_verified_absent_and_old_reference_rejected')
        action('notification_ongoing');_,ongoing=wait_notice('ongoing');assert not ongoing['clearable']
        call('pab_dismiss_notification',{'notification_ref':ongoing['notification_ref']},error='notification_not_clearable')
        passed('ongoing_notification_protected')
        absent='com.pixels.acceptance.absent'
        value,_=call('pab_list_notifications',{'package_id':absent});empty=value['result']['data']['snapshot'];assert not empty['entries']
        waiting,_=call('pab_wait_notification',{'package_id':absent,'after_revision':empty['revision'],'timeout_ms':10000},poll=False);assert waiting['result']['state']=='running'
        assert query({'name':'unified_input'})['elements']
        done,_=call('pab_get_operation',{'operation_id':waiting['operation_ref']['operation_id']});assert done['result']['data']['snapshot']['outcome']=='timed_out'
        passed('unrelated_app_filter_and_wait_does_not_block_ui')
        old_window=window;old_notice=notice(notifications(),'ongoing')['notification_ref']
        client.tool('pab_disconnect',{},allow_error=True);client.close();client=None;connect()
        desktop('pab_ui_query',{'scope':{'type':'window','window_ref':old_window}},error='target_stale')
        call('pab_open_notification',{'notification_ref':old_notice},error='notification_reference_expired')
        windows();assert notice(notifications(),'ongoing')
        passed('reconnect_expires_ui_and_notification_refs_without_replay')
        value,_=call('pab_list_notifications',{'package_id':absent});revision=value['result']['data']['snapshot']['revision']
        waiting,_=call('pab_wait_notification',{'package_id':absent,'after_revision':revision,'timeout_ms':30000},poll=False);assert waiting['result']['state']=='running'
        home.set_paused(True)
        call('pab_get_operation',{'operation_id':waiting['operation_ref']['operation_id']},error='control_paused_or_disconnected')
        passed('pause_interrupts_inflight_notification_wait')
        home.set_paused(False);fixture();action('notification_cleanup');wait_text('notifications_cleaned')
        assert not [n for n in notifications()['entries'] if n['title']=='Pixels QA '+run.name]
        passed('own_test_notifications_cleaned')
        result['status']='passed'
    finally:
        if client is not None:
            try:client.tool('pab_disconnect',{},allow_error=True)
            finally:client.close()
        home.set_paused(True)
        after=phone_state(args.serial)
        assert all(after[k]==before[k] for k in ('apk_sha256','certificate_sha256','first_install_time','fingerprints','paused','connection_requested'))
        result['ending_status']=normal_status(args.serial)
        result['identity_and_original_configuration_retained']=True
        (run/'result.json').write_text(json.dumps(result,ensure_ascii=False,indent=2),encoding='utf-8')
        print(json.dumps({'status':result['status'],'checks':len(result['checks']),'result':str(run/'result.json')}),flush=True)


if __name__=='__main__':main()
