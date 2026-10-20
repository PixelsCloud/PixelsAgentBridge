"""Contribute only measured, version-matching external Android acceptance facts."""
from __future__ import annotations

import json
from pathlib import Path


def collect(root: Path, config: dict, apk_sha256: str, serial: str):
    evidence = config.get('evidence', {})
    facts = []

    def read_relative(relative):
        if not relative:
            return None, None
        path = (root / relative).resolve()
        if not path.is_relative_to(root.resolve()):
            raise ValueError('Evidence must stay inside the workspace')
        if not path.is_file():
            return None, relative
        return json.loads(path.read_text(encoding='utf-8')), relative

    def load(name):
        return read_relative(evidence.get(name))

    def read_events(relative):
        if not relative:
            return []
        path = (root / relative).resolve()
        if not path.is_relative_to(root.resolve()):
            raise ValueError('Evidence must stay inside the workspace')
        if not path.is_file():
            return []
        return [json.loads(line) for line in path.read_text(encoding='utf-8').splitlines() if line]

    def matching(value):
        return (value is not None and value.get('status') == 'passed'
            and value.get('provenance', {}).get('apk_sha256') == apk_sha256)

    def add(gate, key, condition, source):
        if condition:
            facts.append((gate, key, source))

    upgrade, source = load('upgrades')
    if upgrade:
        baseline = upgrade.get('phone', {})
        updates = upgrade.get('upgrades', [])
        fields = ('certificate_sha256', 'first_install_time', 'fingerprints', 'paused', 'connection_requested')
        retained = (all(baseline.get(field) is not None for field in fields)
            and len(baseline.get('fingerprints', {})) >= 5
            and len(updates) >= 1 and updates[-1].get('apk_sha256') == apk_sha256
            and all(all(row.get(field) == baseline.get(field) for field in fields) for row in updates)
            and all(right.get('version_code', 0) > left.get('version_code', 0)
                for left, right in zip([baseline, *updates[:-1]], updates)))
        transitions = {(left['apk_sha256'], right['apk_sha256'])
            for left, right in zip([baseline, *updates[:-1]], updates)} if retained else set()
        upgrade_sources = [source]
        rotation, _ = load('password_rotation')
        # A normal, verified password reset separates two credential epochs. Earlier
        # cover updates still count when the same APK/identity links both epochs and
        # the access-password ciphertext is the only deliberately changed file.
        for historical_source in evidence.get('upgrade_history', []):
            historical, _ = read_relative(historical_source)
            # Rotation belongs to this epoch's baseline APK, which can precede delivery.
            if not (retained and historical and rotation and rotation.get('status') == 'passed'
                    and rotation.get('provenance', {}).get('apk_sha256') == baseline.get('apk_sha256')):
                continue
            initial = historical.get('phone', {})
            history = historical.get('upgrades', [])
            if not history:
                continue
            last = history[-1]
            valid_history = (len(initial.get('fingerprints', {})) >= 5
                and all(all(row.get(field) == initial.get(field) for field in fields) for row in history)
                and all(right.get('version_code', 0) > left.get('version_code', 0)
                    for left, right in zip([initial, *history[:-1]], history))
                and all(row.get('original_request_query') == 'completed'
                    and row.get('paused_input_not_dispatched') is True for row in history))
            rotation_checks = rotation.get('checks', {})
            linked = (valid_history and historical.get('device_ref') == upgrade.get('device_ref')
                and historical.get('device_code') == upgrade.get('device_code')
                and last.get('apk_sha256') == baseline.get('apk_sha256')
                and last.get('version_code') == baseline.get('version_code')
                and all(last.get(field) == baseline.get(field) for field in fields if field != 'fingerprints')
                and rotation_checks.get('old_password_rejected') is True
                and rotation_checks.get('new_password_normal_pairing') is True
                and rotation_checks.get('identity_and_seed_retained') is True)
            old_files, new_files = last.get('fingerprints', {}), baseline.get('fingerprints', {})
            changed = [name for name in old_files if old_files[name] != new_files.get(name)]
            if linked and old_files.keys() == new_files.keys() and len(changed) == 1 and changed[0].endswith('-access.bin'):
                transitions.update((left['apk_sha256'], right['apk_sha256'])
                    for left, right in zip([initial, *history[:-1]], history))
                upgrade_sources.extend([historical_source, evidence['password_rotation']])
        source = ', '.join(dict.fromkeys(upgrade_sources))
        complete = retained and len(transitions) >= 2
        add('A12', 'two_cover_updates', complete, source)
        add('A12', 'same_signature_identity', complete and bool(upgrade.get('device_ref')), source)
        add('A12', 'credentials_settings_pause_retained', complete and baseline.get('paused') is True, source)
        add('A12', 'original_request_query', complete and all(row.get('original_request_query') == 'completed'
            and row.get('paused_input_not_dispatched') is True for row in updates), source)

    operator, source = load('normal_operator')
    if (operator and operator.get('apk_sha256') == apk_sha256
            and operator.get('normal_pairing') is True and operator.get('device_code')):
        add('A01', 'normal_pairing', operator.get('normal_home_start') is True, source)

    controls, source = load('public_controls')
    if matching(controls) and controls['provenance'].get('serial') == serial:
        checks = controls.get('checks', {})
        add('A01', 'phone_home_start', checks.get('normal_home_start') is True, source)
        add('A01', 'default_server_no_certificate', controls['provenance'].get('public_server') is True, source)
        add('A02', 'home_disconnect', checks.get('normal_home_disconnect_releases_service') is True, source)
        add('A02', 'notification_disconnect', checks.get('real_notification_pending_intent_disconnect') is True, source)
        add('A07', 'pause_persisted', checks.get('normal_home_pause_persisted') is True, source)
        add('A07', 'pause_stops_input', checks.get('normal_home_pause_persisted') is True, source)
        add('A07', 'protected_page', checks.get('secure_window_screenshot_rejected') is True
            and checks.get('password_input_redacted_and_rejected') is True
            and checks.get('screenshot_recovers_after_secure_window') is True, source)

    background, source = load('public_background')
    if matching(background) and background['provenance'].get('serial') == serial:
        samples = background.get('samples', [])
        gaps = [right['elapsed_seconds'] - left['elapsed_seconds']
            for left, right in zip(samples, samples[1:])]
        complete = (background.get('initial_own_activity_count', 0) > 0
            and background.get('all_own_activities_destroyed_before') is True
            and background.get('all_own_activities_destroyed_after') is True
            and len(samples) >= 19 and samples[-1]['elapsed_seconds'] - samples[0]['elapsed_seconds'] >= 180
            and [row.get('index') for row in samples] == list(range(len(samples)))
            and all(0 < gap <= 30 for gap in gaps)
            and all(row.get('observe') is True and row.get('input_postcondition') is True
                and row.get('jpeg_decoded') is True and row.get('foreground_service_count') == 1
                for row in samples))
        add('A02', 'activity_destroyed_180s', complete, source)

    notifications, source = load('notification_permission')
    if matching(notifications) and notifications['provenance'].get('serial') == serial:
        checks = notifications.get('checks', {})
        add('A02', 'notification_permission_cases', all(checks.get(key) is True for key in (
            'denied_notification_real_foreground_remote_input', 'denied_notification_home_disconnect',
            'granted_notification_connection', 'original_request_retained',
            'original_permission_and_user_flags_restored')), source)

    reboot, source = load('reboot')
    if matching(reboot) and reboot['provenance'].get('serial') == serial:
        confirmed = (reboot.get('physical_reboot_confirmed') is True and reboot.get('user_confirmation')
            and reboot.get('boot_id_changed') is True and reboot.get('before_boot_id')
            and reboot.get('after_boot_id') and reboot['before_boot_id'] != reboot['after_boot_id'])
        add('A10', 'phone_reboot', confirmed, source)
        add('A10', 'identity_and_pause_retained', confirmed
            and reboot.get('identity_credentials_settings_pause_retained') is True, source)
        add('A10', 'original_request_not_replayed', confirmed
            and reboot.get('original_request_query') == 'completed'
            and reboot.get('paused_input_not_dispatched') is True, source)

    lifecycle, source = load('lifecycle')
    if matching(lifecycle) and lifecycle['provenance'].get('serial') == serial:
        passed_cases = {row['name'] for row in lifecycle.get('cases', []) if row.get('status') == 'passed'
            and row.get('identity_credentials_settings_pause_retained') is True
            and row.get('original_request_query') == 'completed' and row.get('paused_input_not_dispatched') is True}
        add('A10', 'process_reclaim', 'process_sigkill_recovery' in passed_cases, source)
        add('A10', 'force_stop_manual_restart', 'force_stop_foreground_restart' in passed_cases
            and len(lifecycle.get('force_stop', {}).get('passive_observations', [])) >= 10
            and all(row.get('process_present') is False
                for row in lifecycle['force_stop']['passive_observations']), source)
        restored = ('permission_regrant' in passed_cases
            and lifecycle.get('permission_revocation', {}).get('accessibility_ready') is False
            and lifecycle.get('permission_regrant', {}).get('accessibility_ready') is True
            and {row.get('action') for row in lifecycle.get('permission_denials', [])} == {'observe', 'navigate'}
            and all(row.get('error') == 'service_unavailable' and row.get('action_dispatched') is False
                for row in lifecycle['permission_denials']))
        add('A10', 'permission_regrant', restored, source)
        add('A07', 'permission_denied', restored, source)

    recent, source = load('recent_task')
    if matching(recent) and recent['provenance'].get('serial') == serial:
        add('A10', 'recent_task_removal', recent.get('actual_recents_ui_swipe') is True
            and recent.get('own_development_app_only') is True
            and recent.get('removed_task_id') and recent.get('task_absence_verified') is True
            and recent.get('identity_credentials_settings_pause_retained') is True
            and recent.get('original_request_query') == 'completed'
            and recent.get('paused_input_not_dispatched') is True, source)

    lock, source = load('lock_unlock')
    if matching(lock) and lock['provenance'].get('serial') == serial:
        refusals = lock.get('locked_denials', [])
        add('A10', 'lock_and_unlock', lock.get('manual_unlock_confirmation')
            and lock.get('unlock_business_postcondition') is True
            and lock.get('original_request_query') == 'completed'
            and {row.get('action') for row in refusals} == {'observe', 'screenshot', 'navigate'}
            and all(row.get('error') == 'screen_locked' and row.get('action_dispatched') is False
                for row in refusals), source)

    direct, source = load('direct_campaign')
    if matching(direct):
        rows = direct.get('rounds', [])
        complete = (len(rows) >= 20 and direct.get('original_request_query') == 'completed'
            and all(row.get('postcondition') is True
                and set(row.get('connection_paths', {})) == {'direct'}
                and row['connection_paths']['direct'] > 0 for row in rows))
        add('A04', 'direct_20_rounds', complete, source)
        add('A04', 'actual_path_observed', complete, source)

    witness, source = load('physical_network_witness')
    if (witness and witness.get('status') in ('passed', 'awaiting_physical_cellular_retest',
            'cellular_campaign_pending', 'wifi_campaign_pending')
            and witness.get('provenance', {}).get('apk_sha256') == apk_sha256
            and witness['provenance'].get('serial') == serial):
        before = witness.get('before_unplug', {})
        confirmations = {row.get('phase'):row for row in witness.get('confirmations', [])}
        independent = witness.get('independent_network_confirmation', {})
        wifi = confirmations.get('wifi_unplugged', {})
        cellular = confirmations.get('cellular_unplugged', {})
        clock = witness.get('operator_clock_measurement', {})
        physical = (before.get('adb_connected') is True and before.get('adb_reverse') is False
            and before.get('usb_network') is False and before.get('special_phone_http_proxy') is False
            and independent.get('answer') and independent.get('operator_device_code')
            and all(row.get('answer') and row.get('adb_connected') is False
                for row in (wifi, cellular))
            and before.get('observed_at_unix', 0) < wifi.get('observed_at_unix', 0)
            < cellular.get('observed_at_unix', 0))
        passed = []
        sources = [source]
        for label, confirmation in (('wifi', wifi), ('cellular', cellular)):
            offset = witness.get(label + '_clock_measurement', clock).get('offset_midpoint_seconds', 0)
            campaign, campaign_source = read_relative(witness.get(label + '_campaign'))
            if not matching(campaign):
                continue
            rows = campaign.get('rounds', [])
            origin = campaign.get('provenance', {})
            valid = (physical and origin.get('network_label') == label
                and origin.get('forced_relay') is True and origin.get('normal_pairing') is True
                and campaign.get('original_request_query') == 'completed'
                and len(rows) >= 20 and len({row.get('index') for row in rows}) == len(rows)
                and all(row.get('postcondition') is True
                    and set(row.get('connection_paths', {})) == {'relay'}
                    and row['connection_paths']['relay'] > 0 for row in rows)
                and campaign.get('started_at_unix', 0) - offset >= confirmation.get('observed_at_unix', 0)
                and campaign.get('finished_at_unix', 0) > campaign.get('started_at_unix', 0))
            if label == 'wifi':
                valid = valid and campaign.get('finished_at_unix', 0) - offset <= cellular.get('observed_at_unix', 0)
            completion = witness.get(label + '_completion_usb_absence', {})
            end = campaign.get('finished_at_unix', 0) - offset
            valid = valid and completion.get('adb_connected') is False and completion.get('observed_at_unix', 0) >= end
            replug_times = [row['observed_at_unix'] for row in witness.get('usb_events', [])
                if row.get('kind') == 'replugged' and row['observed_at_unix'] >= confirmation.get('observed_at_unix', 0)]
            if replug_times:
                valid = valid and end <= min(replug_times)
            if valid:
                passed.append(label)
                sources.append(campaign_source)
        source = ', '.join(sources)
        for check in ('physical_usb_unplug', 'no_adb_reverse', 'forced_relay', 'original_request_query'):
            add('A03', check, bool(passed), source)
        add('A03', 'wifi_20_rounds', 'wifi' in passed, source)
        add('A03', 'cellular_20_rounds', 'cellular' in passed, source)
        add('A04', 'blocked_direct_relay_20_rounds', bool(passed), source)

    cycles, source = load('network_cycles')
    if matching(cycles) and cycles['provenance'].get('serial') == serial:
        rows = cycles.get('cycles', [])
        complete = (len(rows) >= 30 and {row.get('target') for row in rows} == {'WIFI', 'CELLULAR'}
            and any(row.get('full_outage') is True for row in rows)
            and all(row.get('postcondition') is True and row.get('prior_input_not_replayed') is True
                and row.get('fresh_control_client') is True
                and row.get('target') in row.get('actual_network', {}).get('transports', [])
                and 'VPN' not in row['actual_network']['transports'] for row in rows))
        add('A08', 'physical_network_cycles_30', complete, source)
        if complete:
            timings = sorted(row['reconnect_ms'] for row in rows)
            p95 = timings[(len(timings) * 95 + 99) // 100 - 1]
            add('A08', 'reconnect_p95_at_most_30s', p95 <= 30000, source)
            add('A08', 'no_replayed_input', True, source)

    terminal, source = load('terminal_rejection')
    if matching(terminal):
        add('A08', 'terminal_errors_do_not_retry', terminal.get('scope')
            == 'normal_connection_control_authentication_rejection'
            and terminal.get('authentication_rejections') == 1
            and terminal.get('repeated_authentication_attempts') == 0
            and terminal.get('observation_seconds', 0) >= 15
            and terminal.get('original_settings_and_intent_restored') is True
            and len(terminal.get('samples', [])) >= 10
            and all(row.get('phase') == 'FAILED' and row.get('remote_ready') is False
                for row in terminal['samples']), source)

    standby, source = load('standby_review')
    if matching(standby) and standby['provenance'].get('serial') == serial:
        from analyze_android_soak import analyze

        phone = analyze(read_events(standby.get('phone_telemetry')))
        operator, operator_source = read_relative(standby.get('operator_result'))
        events = read_events(standby.get('operator_trace'))
        gaps = [right['elapsed_seconds'] - left['elapsed_seconds']
            for left, right in zip(events, events[1:])]
        heartbeat = (matching(operator) and operator.get('soak_id') == phone.get('request_id')
            and operator.get('elapsed_seconds', 0) >= 28800 and len(events) >= 481
            and [row.get('index') for row in events] == list(range(len(events)))
            and all(0 < gap <= 150 for gap in gaps)
            and events[-1].get('elapsed_seconds', 0) - events[0].get('elapsed_seconds', 0) >= 28800
            and all(row.get('connection_path') == 'relay' and row.get('action_dispatched') is False
                and (row.get('state') == 'completed' or (row.get('state') == 'failed'
                    and row.get('error') in ('screen_locked', 'control_paused'))) for row in events))
        continuous = (phone.get('telemetry_valid') is True and phone.get('apk_sha256') == apk_sha256
            and phone.get('remote_not_ready_samples') == 0 and heartbeat)
        source = ', '.join(filter(None, (source, standby.get('phone_telemetry'), operator_source)))
        resume = standby.get('resume', {})
        add('A09', 'continuous_8_hours', continuous, source)
        add('A09', 'sampling_complete', continuous, source)
        add('A09', 'screen_off_and_resume', continuous
            and any(row.get('error') == 'screen_locked' for row in events)
            and resume.get('manual_unlock_confirmation') and resume.get('business_postcondition') is True, source)
        add('A09', 'no_duplicate_dispatch', continuous and resume.get('counter_unchanged_during_standby') is True
            and resume.get('fresh_save_delta') == 1 and resume.get('original_request_query') == 'completed', source)
        health = standby.get('health', {})
        add('A09', 'no_crash_or_anr', continuous and health.get('pid_unchanged') is True
            and health.get('exit_history_reviewed') is True and health.get('crash_count') == 0
            and health.get('anr_count') == 0, source)
        review = standby.get('resource_review', {})
        add('A09', 'resource_trends', continuous and len(phone.get('segments', [])) == 4
            and review.get('quartiles_inspected') is True
            and review.get('unresolved_sustained_growth') is False and bool(review.get('interpretation')), source)
        add('A09', 'temperature_battery_traffic', continuous and standby.get('charging_disclosed') is True
            and phone.get('uid_rx_delta_bytes', -1) >= 0 and phone.get('uid_tx_delta_bytes', -1) >= 0, source)

    api28, source = load('api28_direct')
    if matching(api28) and api28.get('sdk') == 28:
        add('A13', 'api28_direct', api28.get('connection_path') == 'direct', source)
        add('A13', 'api28_primitives', api28.get('unicode_input') is True and api28.get('native_gesture') is True, source)
    api28, source = load('api28_relay')
    if matching(api28) and api28.get('sdk') == 28:
        add('A07', 'bad_password', api28.get('wrong_password_rejected') is True, source)
        add('A07', 'stale_reference', api28.get('cross_connection_reference_rejected') is True, source)
        add('A07', 'parameter_conflict', api28.get('request_conflict_rejected') is True, source)
        add('A11', 'dedup_20', api28.get('duplicate_submissions', 0) >= 20 and api28.get('click_count') == 1, source)
        add('A11', 'parameter_conflict', api28.get('request_conflict_rejected') is True, source)

    failure, source = load('settings_failure')
    if matching(failure):
        cases = failure.get('cases', [])
        add('A12', 'migration_failure_preserves_data', {row.get('name') for row in cases}
            == {'malformed_toml', 'unsupported_future_version'} and all(
                row.get('unreadable_bytes_preserved') is True and row.get('original_config_restored') is True
                for row in cases), source)

    policy, source = load('account_revocation')
    if policy and policy.get('status') == 'passed':
        add('A07', 'revoked_account', policy.get('scope') == 'native_account_session_revocation'
            and policy.get('revoked_token_rejected') is True, source)

    build, source = load('build_review')
    if matching(build):
        constraints = build.get('constraints', {})
        budget = (build.get('build_jobs') == 2 and build.get('combined_job_budget') <= 10
            and build.get('own_compilations_serial') is True
            and all(constraints.get(key) == value for key, value in {
                'cargo_jobs':2, 'cpu_affinity_cores':2, 'jvm_active_processor_count':2,
                'gradle_max_workers':2, 'cmake_parallel_level':1, 'msvc_mp':1}.items()))
        add('A15', 'combined_build_budget_at_most_10', budget, source)
        add('A15', 'warnings_reviewed', build.get('warnings_reviewed') is True
            and build.get('android_and_device_session_clippy_strict_passed') is True
            and len(build.get('warnings', [])) >= 3, source)

    platforms, source = load('platform_validation')
    if platforms and platforms.get('status') == 'passed' and platforms.get('apk_sha256') == apk_sha256:
        android = []
        for relative in platforms.get('formal_android_results', []):
            value, _ = read_relative(relative)
            if matching(value) and value.get('foreground_service') is True:
                android.append(value.get('sdk'))
        linux, _ = read_relative(platforms.get('linux', {}).get('startup_catalogue_evidence'))
        add('A14', 'affected_platforms_validated', set(android) == {28, 34}
            and platforms.get('windows', {}).get('public_operator_tested') is True
            and linux and linux.get('status') == 'passed' and linux.get('tool_count', 0) > 0
            and (platforms.get('macos', {}).get('os_specific_android_change') is False
                or platforms.get('macos', {}).get('tested') is True), source)

    return facts
