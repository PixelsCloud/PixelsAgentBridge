"""Acceptance integrity checks: missing time or unrelated evidence must never pass."""
import hashlib
import json
from copy import deepcopy
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import accept_android
from analyze_android_soak import analyze
from android_acceptance_evidence import collect
from android_soak_monitor import health_event


class SoakIntegrityTest(unittest.TestCase):
    def test_padded_logcat_lines_detect_own_anr_and_ignore_other_processes(self):
        prefix = '         1792172638.201 1000 1000 I am_anr : '
        own = prefix + '[0,17,com.pixels.agent.debug,0,private reason]'
        event = health_event(own, 17, 1792172600)
        self.assertEqual(event['category'], 'anr')
        self.assertNotIn('private reason', json.dumps(event))
        self.assertIsNone(health_event(own.replace('agent.debug,', 'agent.debug.test,'), 17, 1792172600))
        crash = '1792172639.000 17 18 E AndroidRuntime: FATAL EXCEPTION: main'
        self.assertEqual(health_event(crash, 17, 1792172600)['category'], 'java_crash')
        self.assertIsNone(health_event(crash, 19, 1792172600))
        self.assertIsNone(health_event(own, 17, 1792172700))

    def events(self):
        result = [{'type':'start', 'pid':17, 'elapsed_ms':0, 'apk_sha256':'a' * 64, 'request_id':'test'}]
        for index in range(481):
            result.append({'type':'sample', 'index':index, 'elapsed_ms':index * 60000, 'pid':17,
                'pss_kib':100000, 'native_heap_bytes':20000000, 'fd_count':50, 'thread_count':20,
                'uid_rx_bytes':index * 100, 'uid_tx_bytes':index * 50, 'plugged':1,
                'temperature_tenths_c':300, 'battery_scale':100, 'battery_level':90, 'remote_ready':True})
        return result

    def test_full_telemetry_still_requires_resource_review_and_other_acceptance(self):
        report = analyze(self.events())
        self.assertTrue(report['telemetry_valid'])
        self.assertTrue(report['resource_trend_review_required'])
        self.assertFalse(report['final_acceptance'])

    def test_short_run_cannot_be_extrapolated(self):
        report = analyze(self.events()[:-1])
        self.assertFalse(report['telemetry_valid'])
        self.assertIn('less_than_eight_hours', report['errors'])

    def test_missing_sample_or_changed_process_invalidates_continuity(self):
        for mutation in ('missing', 'process', 'measurement'):
            with self.subTest(mutation=mutation):
                events = self.events()
                if mutation == 'missing':
                    del events[200]
                elif mutation == 'process':
                    events[200]['pid'] = 18
                else:
                    events[200]['fd_count'] = None
                self.assertFalse(analyze(events)['telemetry_valid'])


class ExternalEvidenceTest(unittest.TestCase):
    def facts(self, key, value):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'source.json').write_text(json.dumps(value), encoding='utf-8')
            return {(gate, check) for gate, check, _ in collect(root,
                {'evidence':{key:'source.json'}}, 'a' * 64, 'phone')}

    def test_password_input_protection_does_not_prove_secure_screenshot_rejection(self):
        value = {'status':'passed', 'provenance':{'apk_sha256':'a' * 64, 'serial':'phone'},
            'checks':{'password_input_redacted_and_rejected':True}}
        self.assertNotIn(('A07', 'protected_page'), self.facts('public_controls', value))
        value['checks'].update(secure_window_screenshot_rejected=True,
            screenshot_recovers_after_secure_window=True)
        self.assertIn(('A07', 'protected_page'), self.facts('public_controls', value))
        value['provenance']['apk_sha256'] = 'b' * 64
        self.assertFalse(self.facts('public_controls', value))

    def test_background_duration_requires_periodic_input_and_decoded_capture(self):
        rows = [{'index':index, 'elapsed_seconds':index, 'observe':True,
            'input_postcondition':True, 'jpeg_decoded':True, 'foreground_service_count':1}
            for index in range(19)]
        value = {'status':'passed', 'provenance':{'apk_sha256':'a' * 64, 'serial':'phone'},
            'background_seconds':180, 'initial_own_activity_count':1,
            'all_own_activities_destroyed_before':True, 'all_own_activities_destroyed_after':True,
            'samples':rows}
        self.assertFalse(self.facts('public_background', value))
        for row in rows:
            row['elapsed_seconds'] *= 10
        self.assertIn(('A02', 'activity_destroyed_180s'), self.facts('public_background', value))
        rows[8]['jpeg_decoded'] = False
        self.assertFalse(self.facts('public_background', value))

    def test_standby_review_flags_without_raw_eight_hours_cannot_pass(self):
        value = {'status':'passed', 'provenance':{'apk_sha256':'a' * 64, 'serial':'phone'},
            'continuous_seconds':28800, 'charging_disclosed':True,
            'resume':{'manual_unlock_confirmation':'unlocked', 'business_postcondition':True,
                'counter_unchanged_during_standby':True, 'fresh_save_delta':1,
                'original_request_query':'completed'},
            'health':{'pid_unchanged':True, 'exit_history_reviewed':True, 'crash_count':0, 'anr_count':0},
            'resource_review':{'quartiles_inspected':True, 'unresolved_sustained_growth':False,
                'interpretation':'purported review'}}
        self.assertFalse(self.facts('standby_review', value))

    def test_reboot_requires_distinct_observed_boot_ids(self):
        value = {'status':'passed', 'provenance':{'apk_sha256':'a' * 64, 'serial':'phone'},
            'physical_reboot_confirmed':True, 'user_confirmation':'rebooted', 'boot_id_changed':True,
            'before_boot_id':'same', 'after_boot_id':'same',
            'identity_credentials_settings_pause_retained':True,
            'original_request_query':'completed', 'paused_input_not_dispatched':True}
        self.assertFalse(self.facts('reboot', value))
        value['after_boot_id'] = 'different'
        self.assertIn(('A10', 'phone_reboot'), self.facts('reboot', value))

    def test_unplug_flags_without_observed_absence_and_campaigns_cannot_pass(self):
        value = {'status':'passed', 'provenance':{'apk_sha256':'a' * 64, 'serial':'phone'},
            'physical_usb_unplug_confirmed':True, 'network_environment_verified':True,
            'before_unplug':{'adb_connected':True, 'adb_reverse':False, 'usb_network':False}}
        self.assertFalse(self.facts('physical_network_witness', value))

    def test_replug_during_cellular_campaign_invalidates_only_that_campaign(self):
        provenance = {'apk_sha256':'a' * 64, 'serial':'phone'}
        witness = {'status':'passed', 'provenance':provenance,
            'before_unplug':{'adb_connected':True, 'adb_reverse':False, 'usb_network':False,
                'special_phone_http_proxy':False, 'observed_at_unix':1},
            'independent_network_confirmation':{'answer':'independent', 'operator_device_code':'123456789'},
            'confirmations':[{'phase':'wifi_unplugged', 'answer':'unplugged', 'adb_connected':False,
                'observed_at_unix':2}, {'phase':'cellular_unplugged', 'answer':'cellular',
                    'adb_connected':False, 'observed_at_unix':5}],
            'wifi_completion_usb_absence':{'adb_connected':False, 'observed_at_unix':5},
            'cellular_completion_usb_absence':{'adb_connected':False, 'observed_at_unix':9},
            'usb_events':[{'kind':'replugged', 'observed_at_unix':7}],
            'wifi_campaign':'wifi.json', 'cellular_campaign':'cellular.json'}
        rows = [{'index':index, 'postcondition':True, 'connection_paths':{'relay':10}} for index in range(20)]
        campaigns = {label:{'status':'passed', 'provenance':{**provenance,
            'network_label':label, 'forced_relay':True, 'normal_pairing':True}, 'rounds':rows,
            'original_request_query':'completed', 'started_at_unix':3 if label == 'wifi' else 6,
            'finished_at_unix':4 if label == 'wifi' else 8} for label in ('wifi', 'cellular')}
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for label, campaign in campaigns.items():
                (root / (label + '.json')).write_text(json.dumps(campaign), encoding='utf-8')

            def measured():
                (root / 'witness.json').write_text(json.dumps(witness), encoding='utf-8')
                return {(gate, check) for gate, check, _ in collect(root,
                    {'evidence':{'physical_network_witness':'witness.json'}}, 'a' * 64, 'phone')}

            facts = measured()
            self.assertIn(('A03', 'wifi_20_rounds'), facts)
            self.assertNotIn(('A03', 'cellular_20_rounds'), facts)
            witness['usb_events'] = []
            self.assertIn(('A03', 'cellular_20_rounds'), measured())

    def test_reconnect_p95_is_recomputed_from_actual_cycles(self):
        rows = [{'target':'WIFI' if index % 2 else 'CELLULAR', 'full_outage':index == 0,
            'fresh_control_client':True,
            'postcondition':True, 'prior_input_not_replayed':True, 'reconnect_ms':40000,
            'actual_network':{'transports':['WIFI' if index % 2 else 'CELLULAR']}}
            for index in range(30)]
        value = {'status':'passed', 'provenance':{'apk_sha256':'a' * 64, 'serial':'phone'},
            'cycles':rows, 'reconnect_p95_ms':1}
        facts = self.facts('network_cycles', value)
        self.assertIn(('A08', 'physical_network_cycles_30'), facts)
        self.assertNotIn(('A08', 'reconnect_p95_at_most_30s'), facts)
        rows[0]['fresh_control_client'] = False
        self.assertFalse(self.facts('network_cycles', value))
        rows[0]['fresh_control_client'] = True
        value['cycles'] = rows[:-1]
        self.assertFalse(self.facts('network_cycles', value))

    def test_upgrade_epochs_require_proven_identity_and_only_password_change(self):
        state = {'apk_sha256':'x' * 64, 'version_code':1, 'certificate_sha256':'certificate',
            'first_install_time':'original', 'paused':True, 'connection_requested':True,
            'fingerprints':{'settings':'s', 'intent':'i', 'seed':'k',
                'credentials-access.bin':'old', 'registration':'r'}}
        after = {**deepcopy(state), 'apk_sha256':'b' * 64, 'version_code':2,
            'original_request_query':'completed', 'paused_input_not_dispatched':True}
        rotated = deepcopy(after)
        rotated['fingerprints']['credentials-access.bin'] = 'new'
        delivered = {**deepcopy(rotated), 'apk_sha256':'a' * 64, 'version_code':3}
        shared = {'device_ref':{'id':'same'}, 'device_code':'123456789'}
        files = {'history.json':{**shared, 'phone':state, 'upgrades':[after]},
            'current.json':{**shared, 'phone':rotated, 'upgrades':[delivered]},
            'rotation.json':{'status':'passed', 'provenance':{'apk_sha256':'b' * 64},
                'checks':{'old_password_rejected':True, 'new_password_normal_pairing':True,
                    'identity_and_seed_retained':True}}}
        config = {'evidence':{'upgrades':'current.json', 'upgrade_history':['history.json'],
            'password_rotation':'rotation.json'}}
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)

            def measured(values):
                for name, value in values.items():
                    (root / name).write_text(json.dumps(value), encoding='utf-8')
                return {(gate, check) for gate, check, _ in collect(root, config, 'a' * 64, 'phone')}

            self.assertIn(('A12', 'two_cover_updates'), measured(files))
            for mutation in ('seed', 'identity', 'old_password', 'same_version'):
                invalid = deepcopy(files)
                if mutation == 'seed':
                    invalid['current.json']['phone']['fingerprints']['seed'] = 'changed'
                    invalid['current.json']['upgrades'][0]['fingerprints']['seed'] = 'changed'
                elif mutation == 'identity':
                    invalid['current.json']['device_ref'] = {'id':'different'}
                elif mutation == 'old_password':
                    invalid['rotation.json']['checks']['old_password_rejected'] = False
                else:
                    invalid['history.json']['upgrades'][0]['version_code'] = 1
                with self.subTest(mutation=mutation):
                    self.assertNotIn(('A12', 'two_cover_updates'), measured(invalid))


class ReportIntegrityTest(unittest.TestCase):
    def run_report(self, *, mismatch=False, exit_code=0):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            apk = root / 'test.apk'
            apk.write_bytes(b'acceptance artifact')
            digest = hashlib.sha256(apk.read_bytes()).hexdigest()
            config = {'device':{'serial':'phone', 'emulator':'emulator'},
                'acceptance':{'run_id':'22392f72-e35a-47ed-9eba-17ce8727f04e'}}
            with patch.multiple(accept_android, ROOT=root, APK=apk,
                    installed_apk_sha256=lambda serial: digest):
                acceptance = accept_android.Acceptance(config)
                acceptance.bind_apk()
                report = {'status':'passed', 'provenance':{'serial':'phone',
                    'apk_sha256':'b' * 64 if mismatch else digest}, 'normal_device_pairing':True,
                    'primitive_results':{'rounds':10, 'jpeg_decoded':True,
                        'gesture_postcondition':True, 'bounded_wait_timeout':True},
                    'scenario_results':{name:{'attempted':50, 'passed':50, 'failures':[]}
                        for name in ('multi_page_form', 'scroll_find', 'application_switch')}}
                source = root / 'source.json'
                source.write_text(json.dumps(report), encoding='utf-8')
                acceptance.data['steps'] = [{'name':'phone-server', 'exit_code':exit_code,
                    'evidence':'source.json', 'log':'test.log'}]
                self.assertFalse(acceptance.report())
                return json.loads((acceptance.root / 'report.json').read_text(encoding='utf-8'))

    def test_valid_task_campaign_does_not_prove_physical_unplug_or_eight_hours(self):
        report = self.run_report()
        self.assertEqual(report['gates']['A05']['status'], '通过')
        self.assertEqual(report['gates']['A06']['status'], '通过')
        self.assertEqual(report['gates']['A03']['status'], '待验')
        self.assertEqual(report['gates']['A09']['status'], '待验')
        self.assertFalse(report['final_acceptance'])

    def test_changed_apk_or_failed_driver_cannot_contribute_success(self):
        for values in ({'mismatch':True}, {'exit_code':1}):
            with self.subTest(values=values):
                report = self.run_report(**values)
                self.assertEqual(report['gates']['A05']['status'], '待验')
                self.assertEqual(report['gates']['A06']['status'], '待验')


if __name__ == '__main__':
    unittest.main()
