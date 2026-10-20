#!/usr/bin/env python3
"""Run and resume Android acceptance, retaining incomplete gates as non-passing.

Configuration is TOML. Reports are runtime JSON; they contain no credentials or screens.
Only this runner's recorded subprocess results can contribute automated evidence.
"""
from __future__ import annotations

import argparse
from contextlib import contextmanager
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import tomllib
import uuid

from test_android_remote import ROOT, installed_apk_sha256
from android_acceptance_evidence import collect

APK = ROOT / "apps/android/app/build/outputs/apk/debug/app-debug.apk"
GATES = {
    "A01": ("正常用户连接", ["normal_pairing", "phone_home_start", "default_server_no_certificate"]),
    "A02": ("正式前台服务", ["foreground_service", "activity_destroyed_180s", "connection_cycles_20", "home_disconnect", "notification_disconnect", "notification_permission_cases"]),
    "A03": ("真实拔线跨网", ["physical_usb_unplug", "no_adb_reverse", "wifi_20_rounds", "cellular_20_rounds", "forced_relay", "original_request_query"]),
    "A04": ("Direct 与回退", ["direct_20_rounds", "blocked_direct_relay_20_rounds", "actual_path_observed"]),
    "A05": ("基础操作与画面", ["all_primitives_10_rounds", "jpeg_decoded", "gesture_postcondition", "bounded_wait"]),
    "A06": ("完整 AI 任务", ["formal_mcp_task_chain", "multi_page_form_49_of_50", "scroll_find_49_of_50", "application_switch_49_of_50", "no_duplicate_or_wrong_target"]),
    "A07": ("异常与停止", ["bad_password", "revoked_account", "stale_reference", "parameter_conflict", "protected_page", "permission_denied", "timeout", "pause_stops_input", "disconnect_stops_input", "pause_persisted"]),
    "A08": ("切网与重连", ["physical_network_cycles_30", "reconnect_p95_at_most_30s", "no_replayed_input", "terminal_errors_do_not_retry"]),
    "A09": ("后台稳定性", ["continuous_8_hours", "screen_off_and_resume", "no_crash_or_anr", "no_duplicate_dispatch", "resource_trends", "temperature_battery_traffic", "sampling_complete"]),
    "A10": ("系统恢复边界", ["lock_and_unlock", "permission_regrant", "process_reclaim", "recent_task_removal", "force_stop_manual_restart", "phone_reboot", "identity_and_pause_retained", "original_request_not_replayed"]),
    "A11": ("执行事实与容量", ["dedup_20", "parameter_conflict", "queued_cancel", "multiple_controllers", "dispatch_boundary_failure", "archive_preserves_ids", "post_archive_new_work", "maintenance_failure_preserves_data"]),
    "A12": ("覆盖升级", ["two_cover_updates", "same_signature_identity", "credentials_settings_pause_retained", "original_request_query", "migration_failure_preserves_data"]),
    "A13": ("Android 9 回归", ["api28_foreground_service", "api28_direct", "api28_relay", "api28_primitives", "api28_system_capture", "api28_consent_revoked", "api28_pause_reconnect"]),
    "A14": ("现有功能兼容", ["shared_rust_tests", "desktop_auth_session_mcp", "legacy_wire_compatibility", "web_checks", "affected_platforms_validated"]),
    "A15": ("格式与构建", ["kotlin_format_lint_tests", "rust_format", "rust_tests", "arm64_build", "x86_64_build", "combined_build_budget_at_most_10", "warnings_reviewed"]),
    "A16": ("最终交付一致", ["all_other_gates_passed", "final_apk_installed", "normal_home_usable", "test_resources_cleaned", "documentation"]),
}


def atomic_json(path, value):
    pending = path.with_suffix(path.suffix + ".tmp")
    pending.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    pending.replace(path)


@contextmanager
def session_lock(config):
    run_id = str(uuid.UUID(config["acceptance"]["run_id"]))
    directory = ROOT / ".build/android/acceptance" / run_id
    directory.mkdir(parents=True, exist_ok=True)
    with (directory / "session.lock").open("a+b") as lock:
        if os.name == "nt":
            import msvcrt
            lock.seek(0, os.SEEK_END)
            if lock.tell() == 0:
                lock.write(b"0")
                lock.flush()
            lock.seek(0)
            try:
                msvcrt.locking(lock.fileno(), msvcrt.LK_NBLCK, 1)
            except OSError:
                raise RuntimeError("This acceptance session is already running; wait before resuming") from None
        else:
            import fcntl
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        yield


class Acceptance:
    def __init__(self, config):
        self.config = config
        self.serial = config["device"]["serial"]
        self.emulator = config["device"].get("emulator", "emulator-5554")
        self.jobs = config.get("build", {}).get("jobs", 2)
        if type(self.jobs) is not int or not 1 <= self.jobs <= 10:
            raise ValueError("build.jobs must be an integer from 1 to 10")
        run_id = str(uuid.UUID(config["acceptance"]["run_id"]))
        self.root = ROOT / ".build/android/acceptance" / run_id
        self.root.mkdir(parents=True, exist_ok=True)
        self.path = self.root / "session.json"
        self.data = json.loads(self.path.read_text(encoding="utf-8")) if self.path.exists() else {
            "schema_version": 1, "run_id": run_id, "serial": self.serial,
            "apk_sha256": None, "started_at_unix": time.time(), "steps": [],
        }
        if self.data["schema_version"] != 1 or self.data["serial"] != self.serial:
            raise ValueError("Acceptance session belongs to another schema or device")
        if self.data["apk_sha256"] is not None:
            self.bind_apk()

    def bind_apk(self):
        digest = hashlib.sha256(APK.read_bytes()).hexdigest()
        previous = self.data["apk_sha256"]
        if previous is not None and previous != digest:
            raise ValueError("APK changed: start a new acceptance run; old results cannot be reused")
        self.data["apk_sha256"] = digest
        atomic_json(self.path, self.data)
        return digest

    def run(self, name, script, args, *, evidence=None):
        started = time.time()
        log_path = self.root / f"{len(self.data['steps']):03d}-{name}.log"
        print(f"Acceptance step: {name}; log: {log_path}", flush=True)
        before = set(evidence.glob("*/result.json")) if evidence else set()
        with log_path.open("w", encoding="utf-8") as log:
            result = subprocess.run([sys.executable, "-u", str(ROOT / "scripts" / script), *args],
                                    cwd=ROOT, stdout=log, stderr=subprocess.STDOUT)
        entry = {"name": name, "started_at_unix": started, "elapsed_seconds": round(time.time() - started, 3),
                 "exit_code": result.returncode, "log": str(log_path.relative_to(ROOT)), "evidence": None}
        if evidence:
            created = set(evidence.glob("*/result.json")) - before
            matching = []
            for path in created:
                value = json.loads(path.read_text(encoding="utf-8"))
                origin = value.get("provenance", {})
                if origin.get("serial") == args[args.index("--serial") + 1] and origin.get("apk_sha256") == self.data["apk_sha256"]:
                    matching.append(path)
            if len(matching) == 1:
                entry["evidence"] = str(matching[0].relative_to(ROOT))
        self.data["steps"].append(entry)
        atomic_json(self.path, self.data)
        print(f"Acceptance step finished: {name}; exit={result.returncode}", flush=True)
        return result.returncode == 0

    def verify(self):
        if self.data["apk_sha256"] is not None:
            raise ValueError("Build before binding an acceptance version; use a new run ID for rebuilding")
        commands = [("native-test", ["test-native"]), ("arm64-build", ["native", "--abi", "arm64-v8a"]),
                    ("x86_64-build", ["native", "--abi", "x86_64"]), ("android-verify", ["verify"]),
                    ("shared-regression", ["regression"]), ("shared-test", ["shared-test"]),
                    ("web-check", ["check-web"])]
        for name, args in commands:
            if not self.run(name, "build_android.py", [*args, "--jobs", str(self.jobs)]):
                return False
        self.bind_apk()
        return True

    def phone(self, emulator=False):
        self.bind_apk()
        serial = self.emulator if emulator else self.serial
        if installed_apk_sha256(serial) != self.data["apk_sha256"]:
            raise ValueError("Selected device does not have this acceptance APK; install with adb install -r")
        args = ["--serial", serial, "--background-seconds", "180"]
        if not emulator:
            args += ["--driver", "debug", "--preconfigured-accessibility", "--skip-lock-cycle",
                     "--connection-cycles", "20", "--operation-rounds", "10", "--scenario-rounds", "50"]
        return self.run("api28-server" if emulator else "phone-server", "test_android_server.py", args,
                        evidence=ROOT / ".build/android/server")

    def public_background(self):
        run_id = str(uuid.UUID(self.config['public']['run_id']))
        if not self.run('public-home-start', 'android_home.py', ['start', '--serial', self.serial]):
            return False
        return self.run('public-background', 'android_public_background.py',
            ['--serial', self.serial, '--public-run-id', run_id],
            evidence=ROOT / '.build/android/public' / run_id)

    def soak(self):
        public_id = str(uuid.UUID(self.config['public']['run_id']))
        soak = self.config['soak']
        run_id = str(uuid.UUID(soak['run_id']))
        seconds = soak.get('seconds', 28890)
        if type(seconds) is not int or not 28860 <= seconds <= 86400:
            raise ValueError('Formal standby requires 28860..86400 real seconds')
        good = self.run('standby-phone-monitor', 'android_soak_monitor.py',
            ['--serial', self.serial, '--public-run-id', public_id, '--run-id', run_id,
                '--seconds', str(seconds)])
        path = ROOT / '.build/android/acceptance' / run_id / 'monitor-result.json'
        if path.is_file():
            value = json.loads(path.read_text(encoding='utf-8'))
            origin = value.get('provenance', {})
            if origin.get('apk_sha256') == self.data['apk_sha256'] and origin.get('serial') == self.serial:
                self.data['steps'][-1]['evidence'] = str(path.relative_to(ROOT))
                atomic_json(self.path, self.data)
        return good

    def adopt(self, source, exit_code):
        """Record an already executed harness command and its observed process exit code."""
        self.bind_apk()
        path = source.resolve()
        directory = (ROOT / ".build/android/server").resolve()
        if not path.is_relative_to(directory) or path.name != "result.json":
            raise ValueError("Only an existing local Server acceptance result can be adopted")
        value = json.loads(path.read_text(encoding="utf-8"))
        origin = value.get("provenance", {})
        if (value.get("request_id") != path.parent.name or origin.get("apk_sha256") != self.data["apk_sha256"]
                or origin.get("serial") not in (self.serial, self.emulator)
                or origin.get("started_at_unix", 0) < self.data["started_at_unix"]):
            raise ValueError("Evidence belongs to another artifact, device, or acceptance interval")
        if any(step.get("evidence") == str(path.relative_to(ROOT)) for step in self.data["steps"]):
            return
        if exit_code == 0 and value.get("status") != "passed":
            raise ValueError("A failed harness report cannot be recorded as process success")
        self.data["steps"].append({"name":"api28-server" if origin["serial"] == self.emulator else "phone-server",
            "exit_code":exit_code, "source":"previously executed command", "started_at_unix":origin["started_at_unix"],
            "log":str((path.parent / "instrumentation.log").relative_to(ROOT)), "evidence":str(path.relative_to(ROOT))})
        atomic_json(self.path, self.data)

    def adopt_check(self, name, log, exit_code):
        """Record an observed external check; callers must supply its actual exit code."""
        allowed = {'native-test', 'arm64-build', 'x86_64-build', 'android-verify',
                   'shared-regression', 'shared-test', 'web-check', 'rust-format', 'clippy'}
        if name not in allowed:
            raise ValueError('Unsupported external check name')
        self.bind_apk()
        path = log.resolve()
        if not path.is_relative_to((ROOT / '.build/android').resolve()) or not path.is_file():
            raise ValueError('Check logs must be existing files under .build/android')
        relative = str(path.relative_to(ROOT))
        if any(step['name'] == name and step['log'] == relative for step in self.data['steps']):
            return
        self.data['steps'].append({'name':name, 'started_at_unix':path.stat().st_mtime,
            'elapsed_seconds':None, 'exit_code':exit_code, 'log':relative, 'evidence':None,
            'recorded_external_check':True, 'apk_sha256':self.data['apk_sha256']})
        atomic_json(self.path, self.data)

    def report(self):
        checks = {gate: {} for gate in GATES}

        def mark(gate, check, condition, source):
            if condition:
                checks[gate][check] = source

        # A successful subtest contributes only its measured assertions. A debug driver
        # cannot establish the normal home flow, physical unplug, Direct, or an 8h soak.
        for step in self.data["steps"]:
            if step["exit_code"] != 0:
                continue
            source = step.get("evidence")
            if step["name"] == "android-verify":
                mark("A15", "kotlin_format_lint_tests", True, step["log"])
            if step['name'] == 'native-test':
                native_log = (ROOT / step['log']).read_text(encoding='utf-8')
                native_contracts = {
                    'queued_cancel':'cancellation_and_restart_never_replay_dispatched_input',
                    'dispatch_boundary_failure':'cancellation_and_restart_never_replay_dispatched_input',
                    'multiple_controllers':'competing_controllers_cannot_cancel_or_replay_each_others_input',
                    'archive_preserves_ids':'capacity_maintenance_preserves_old_ids_and_accepts_new_work',
                    'post_archive_new_work':'capacity_maintenance_preserves_old_ids_and_accepts_new_work',
                    'maintenance_failure_preserves_data':'interrupted_archive_transaction_does_not_forget_an_accepted_action',
                }
                for check, test in native_contracts.items():
                    mark('A11', check, f'::{test} ... ok' in native_log, step['log'])
                mark('A07', 'disconnect_stops_input',
                    '::disconnected_scope_cannot_dispatch_queued_actions ... ok' in native_log,
                    step['log'])
            successful = {item["name"] for item in self.data["steps"] if item["exit_code"] == 0}
            if {"native-test", "shared-regression", "shared-test"} <= successful:
                mark("A15", "rust_tests", True, "recorded native and shared test steps")
                mark("A14", "shared_rust_tests", True, "recorded shared test steps")
            mark("A14", "web_checks", step["name"] == "web-check", step["log"])
            mark("A14", "desktop_auth_session_mcp", step["name"] == "shared-test", step["log"])
            mark("A14", "legacy_wire_compatibility", step["name"] == "shared-regression", step["log"])
            mark("A15", "rust_format", step["name"] == "rust-format", step["log"])
            for name, check in (("arm64-build", "arm64_build"), ("x86_64-build", "x86_64_build")):
                mark("A15", check, step["name"] == name, step["log"])
            if not source:
                continue
            value = json.loads((ROOT / source).read_text(encoding="utf-8"))
            origin = value.get("provenance", {})
            if value.get("status") != "passed" or origin.get("apk_sha256") != self.data["apk_sha256"]:
                continue
            if origin.get("serial") == self.serial:
                mark("A01", "normal_pairing", value.get("normal_device_pairing") is True, source)
                mark("A01", "phone_home_start", value.get("home_screen_start") is True, source)
                mark("A02", "foreground_service", value.get("foreground_service") is True, source)
                mark("A02", "activity_destroyed_180s", value.get("activity_closed_connection_survives") is True
                    and value.get('background_input_and_capture') is True
                    and value.get("background_seconds", 0) >= 180, source)
                mark("A02", "connection_cycles_20", value.get("connection_cycles", 0) >= 20, source)
                mark("A02", "home_disconnect", value.get("home_screen_disconnect") is True, source)
                primitive = value.get("primitive_results") or {}
                for key, field in (("jpeg_decoded", "jpeg_decoded"), ("gesture_postcondition", "gesture_postcondition"), ("bounded_wait", "bounded_wait_timeout")):
                    mark("A05", key, primitive.get(field) is True and primitive.get("rounds", 0) >= 10, source)
                mark("A05", "all_primitives_10_rounds", primitive.get("rounds", 0) >= 10, source)
                mark('A07', 'timeout', primitive.get('bounded_wait_timeout') is True, source)
                for scenario, outcome in (value.get("scenario_results") or {}).items():
                    key = f"{scenario}_49_of_50"
                    if key in GATES["A06"][1]:
                        mark("A06", key, outcome.get("attempted", 0) >= 50 and outcome.get("passed", 0) >= 49 and outcome["passed"] / outcome["attempted"] >= .98, source)
                scenarios = value.get("scenario_results") or {}
                complete_chain = set(scenarios) == {"multi_page_form", "scroll_find", "application_switch"}
                mark("A06", "formal_mcp_task_chain", complete_chain and value.get("normal_device_pairing") is True, source)
                mark("A06", "no_duplicate_or_wrong_target", complete_chain and all(
                    item.get("attempted", 0) >= 50 and item.get("passed") == item.get("attempted")
                    and not item.get("failures") for item in scenarios.values()), source)
                mark("A07", "pause_stops_input", value.get("pause_resume") is True, source)
            if origin.get("serial") == self.emulator and value.get("sdk") == 28:
                mark("A13", "api28_foreground_service", value.get("foreground_service") is True, source)
                mark("A13", "api28_relay", value.get("real_relay") is True and origin.get("forced_relay") is True, source)
                mark("A13", "api28_system_capture", value.get("system_consented_screenshot") is True, source)
                mark("A13", "api28_consent_revoked", value.get("stop_capture_without_accessibility") is True, source)
                mark('A13', 'api28_pause_reconnect', value.get('pause_resume') is True
                    and value.get('control_reconnect_same_identity') is True
                    and value.get('query_original_after_reconnect') is True, source)

        external = {**self.config, 'evidence':dict(self.config.get('evidence', {}))}
        for step in self.data['steps']:
            if step['name'] == 'public-background' and step['exit_code'] == 0 and step.get('evidence'):
                external['evidence']['public_background'] = step['evidence']
        for gate, check, source in collect(ROOT, external, self.data['apk_sha256'], self.serial):
            if gate in GATES and check in GATES[gate][1]:
                mark(gate, check, True, source)

        try:
            if self.data["apk_sha256"] is not None:
                mark("A16", "final_apk_installed", self.bind_apk() == installed_apk_sha256(self.serial), "installed APK SHA-256")
        except (OSError, RuntimeError, subprocess.SubprocessError):
            pass
        other_passed = all(all(check in checks[gate] for check in required) for gate, (_, required) in GATES.items() if gate != "A16")
        mark("A16", "all_other_gates_passed", other_passed, "A01–A15")
        gates = {}
        failed = [step for step in self.data["steps"] if step["exit_code"] != 0]
        for gate, (name, required) in GATES.items():
            missing = [check for check in required if check not in checks[gate]]
            gates[gate] = {"name": name, "status": "通过" if not missing else "待验",
                           "checks": checks[gate], "missing": missing}
        passed = all(value["status"] == "通过" for value in gates.values())
        report = {"schema_version": 1, "run_id": self.data["run_id"], "apk_sha256": self.data["apk_sha256"],
                  "serial": self.serial, "final_acceptance": passed, "failed_steps": failed, "gates": gates,
                  "generated_at": datetime.now(timezone.utc).isoformat()}
        atomic_json(self.root / "report.json", report)
        rows = [f"# Android 远程控制验收 · {self.data['run_id']}", "",
                f"整体：{'通过' if passed else '未通过，必验项目尚未全部满足'}。", "",
                f"APK SHA-256：`{self.data['apk_sha256']}`", "",
                "本机隔离服务使用 ADB reverse，不构成拔线跨网或公网验收。", "",
                "| 项目 | 状态 | 未满足的检查 |", "|---|---|---|"]
        rows += [f"| {gate} {value['name']} | {value['status']} | {', '.join(value['missing']) or '无'} |" for gate, value in gates.items()]
        rows += ["", f"失败的执行步骤：{len(failed)}。完整脱敏结果见同目录 report.json；原始日志仅保存在本机。", ""]
        (self.root / "report.md").write_text("\n".join(rows), encoding="utf-8")
        print(f"Acceptance report: {self.root / 'report.md'}; final_acceptance={passed}", flush=True)
        return passed


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--phase", choices=["all", "init", "verify", "phone", "emulator",
        "public-background", "soak", "report", "adopt", "adopt-check"], default="all")
    parser.add_argument("--result", type=Path, help="Already executed local Server result for --phase adopt")
    parser.add_argument("--exit-code", type=int, help="Observed subprocess exit code for --phase adopt")
    parser.add_argument("--check-name", help="Completed build/check name for --phase adopt-check")
    parser.add_argument("--log", type=Path, help="Existing command log for --phase adopt-check")
    options = parser.parse_args()
    config = tomllib.loads(options.config.read_text(encoding="utf-8"))
    with session_lock(config):
        acceptance = Acceptance(config)
        good = True
        try:
            if options.phase == 'init':
                digest = acceptance.bind_apk()
                if installed_apk_sha256(acceptance.serial) != digest:
                    raise ValueError('Initialize only after installing the final acceptance APK')
            if options.phase == "adopt":
                if options.result is None or options.exit_code is None:
                    parser.error("--phase adopt requires --result and the actual --exit-code")
                acceptance.adopt(options.result, options.exit_code)
            if options.phase == "adopt-check":
                if options.check_name is None or options.log is None or options.exit_code is None:
                    parser.error('adopt-check requires --check-name, --log and the actual --exit-code')
                acceptance.adopt_check(options.check_name, options.log, options.exit_code)
            if options.phase in ("all", "verify"):
                good = acceptance.verify()
            if good and options.phase in ("all", "phone"):
                good = acceptance.phone()
            if good and options.phase in ("all", "emulator"):
                good = acceptance.phone(emulator=True)
            if good and (options.phase == 'public-background'
                    or options.phase == 'all' and 'public' in config):
                good = acceptance.public_background()
            if good and (options.phase == 'soak' or options.phase == 'all' and 'soak' in config):
                good = acceptance.soak()
        finally:
            passed = acceptance.report()
    return 0 if good and passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
