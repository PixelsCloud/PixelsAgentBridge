#!/usr/bin/env python3
"""Unified release smoke runner. See smoke/README.md; no implicit deployment."""
from __future__ import annotations

import argparse
from contextlib import contextmanager
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tomllib
import uuid

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from smoke.adapters import command_for, counts_for, result_status

HOME = ROOT / "smoke"
TARGETS = ("windows", "macos", "linux", "linux-desktop", "android", "management", "relay", "server", "web")
STATUSES = ("pass", "fail", "blocked", "unconfirmed", "not_run")


def now():
    return datetime.now(timezone.utc).isoformat()


def digest(path):
    result = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for data in iter(lambda: stream.read(1024 * 1024), b""):
            result.update(data)
    return result.hexdigest()


def git(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT)


def source_state():
    # Include untracked, non-ignored sources, so a new test or implementation changes the binding.
    names = sorted(set(git("ls-files", "-z", "--cached", "--others", "--exclude-standard").split(b"\0")) - {b""})
    result = hashlib.sha256()
    counters = {b'build-version.json', b'product-versions.toml'}
    for raw in names:
        if raw in counters:
            continue
        path = ROOT / os.fsdecode(raw)
        result.update(raw + b"\0")
        result.update(digest(path).encode() if path.is_file() else b"MISSING")
    dirty = git("status", "--porcelain", "-z").split(b"\0")
    # Version counters are allocation data. The report binds the exact version and artifact.
    dirty = [os.fsdecode(row) for row in dirty if row and row[3:] not in counters]
    return {"commit": git("rev-parse", "HEAD").decode().strip(),
            "sha256": result.hexdigest(), "dirty": bool(dirty)}


def catalog():
    data = tomllib.loads((HOME / "cases.toml").read_text(encoding="utf-8"))
    cases = data["case"]
    ids = set()
    for case in cases:
        if not re.fullmatch(r"[A-Z]+-[0-9]{2}", case["id"]) or case["id"] in ids:
            raise ValueError("Invalid or duplicate case ID")
        ids.add(case["id"])
        if (case["level"] not in ("quick", "release", "soak")
                or not set(case["targets"]) <= set(TARGETS) or not case["targets"]
                or not case["steps"] or len(case["steps"]) != len(case["expected"])
                or not case.get("cleanup") or not case.get("prerequisite")):
            raise ValueError(f"Incomplete case definition: {case['id']}")
    return cases


def selected(target, profile):
    levels = {"quick": {"quick"}, "release": {"quick", "release"},
              "soak": {"quick", "release", "soak"}}[profile]
    return [case for case in catalog() if target in case["targets"] and case["level"] in levels]


def redact(text):
    text = re.sub(r"-----BEGIN [^-]*PRIVATE KEY-----[\s\S]*?-----END [^-]*PRIVATE KEY-----", "[REDACTED KEY]", text)
    text = re.sub(r"(?i)(authorization\s*[:=]\s*(?:bearer\s+)?|(?:password|access_token|refresh_token|secret)\s*[\"']?\s*[:=]\s*[\"']?)[^\s\"',}]+", r"\1[REDACTED]", text)
    text = re.sub(r"(https?://)[^\s/@:]+:[^\s/@]+@", r"\1[REDACTED]@", text)
    return text


def save(path, report):
    path.parent.mkdir(parents=True, exist_ok=True)
    temp = path.with_suffix(".tmp")
    temp.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    temp.replace(path)
    lines = ["# 发布冒烟结果", "", f"目标：{report['target']} / {report['arch']}；范围：{report['profile']}",
             f"提交：{report['source']['commit']}；产物 SHA-256：{report['artifact'].get('sha256', '未绑定')}",
             "", "| 用例 | 状态 | 证据类型 | 最近说明 |", "|---|---|---|---|"]
    for item in report["cases"]:
        last = item["attempts"][-1] if item["attempts"] else {}
        note = last.get("note", "").replace("|", "/").replace("\n", " ")
        lines.append(f"| {item['id']} | {item['status']} | {last.get('kind', '未执行')} | {note} |")
    lines += ["", "通过门禁需另行运行 gate；人工观察不等于自动验证。", ""]
    path.with_suffix(".md").write_text("\n".join(lines), encoding="utf-8")


def local_report(path):
    path = Path(path).resolve()
    if not path.is_relative_to((HOME / "runs").resolve()) or path.name != "report.json":
        raise ValueError("Reports must be smoke/runs/<run-id>/report.json")
    return path


@contextmanager
def lock():
    folder = HOME / "runs"
    folder.mkdir(parents=True, exist_ok=True)
    # Kernel lock releases on crash; leave the harmless lock file in place.
    with (folder / "runner.lock").open("a+b") as stream:
        if os.name == "nt":
            import msvcrt
            stream.seek(0, 2)
            if stream.tell() == 0:
                stream.write(b"0"); stream.flush()
            stream.seek(0)
            try:
                msvcrt.locking(stream.fileno(), msvcrt.LK_NBLCK, 1)
            except OSError:
                raise ValueError("Another smoke writer is running") from None
        else:
            import fcntl
            try:
                fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except OSError:
                raise ValueError("Another smoke writer is running") from None
        try:
            yield
        finally:
            if os.name == "nt":
                stream.seek(0); msvcrt.locking(stream.fileno(), msvcrt.LK_UNLCK, 1)


def verify_binding(report):
    if report["source"] != source_state():
        raise ValueError("Source changed; create a new run instead of reusing old results")
    if report["catalog_sha256"] != digest(HOME / "cases.toml"):
        raise ValueError("Case definitions changed")
    if "COMPAT-01" in {c["id"] for c in report["cases"]}:
        if report.get("compatibility_sha256") != digest(HOME / "compatibility.toml"):
            raise ValueError("Compatibility matrix changed")


def required_results(report):
    required = selected(report["target"], report["profile"])
    actual = report["cases"]
    if len(actual) != len(required) or {x["id"] for x in actual} != {x["id"] for x in required}:
        raise ValueError("Missing, duplicate or unexpected case results")
    return required


def validate_release_report(path, *, platform, arch, artifact, version, commit=None):
    path = local_report(path)
    report = json.loads(path.read_text(encoding="utf-8"))
    if report.get("schema_version") != 1 or report["profile"] not in ("release", "soak"):
        raise ValueError("A release/soak report is required; quick checks cannot authorize publishing")
    if (report["target"], report["arch"], report["version"]) != (platform, arch, version):
        raise ValueError("Report target, architecture or version does not match the release")
    verify_binding(report)
    if report["source"]["dirty"] or (commit and report["source"]["commit"] != commit):
        raise ValueError("Release report must bind committed sources and the reserved commit")
    required = required_results(report)
    expected_artifact = report["artifact"]
    if (expected_artifact.get("sha256") != digest(artifact)
            or expected_artifact.get("bytes") != Path(artifact).stat().st_size):
        raise ValueError("Tested artifact differs from the release artifact")
    if report.get("unresolved_resources"):
        raise ValueError("Test resources require cleanup/reconciliation")
    by_id = {case["id"]: case for case in required}
    for item in report["cases"]:
        if item["status"] != "pass" or not item["attempts"]:
            raise ValueError(f"Required case has not passed: {item['id']}")
        last = item["attempts"][-1]
        if last.get("status") != "pass" or not last.get("finished_at"):
            raise ValueError("Incomplete attempt")
        evidence = (path.parent / last["evidence"]).resolve()
        if not evidence.is_relative_to(path.parent) or not evidence.is_file() or digest(evidence) != last["evidence_sha256"]:
            raise ValueError("Evidence missing or modified")
        if by_id[item["id"]].get("adapter"):
            if last["kind"] != "automated" or result_status(last["exit_code"], last["counts"]) != "pass":
                raise ValueError("Automated case needs real non-skipped test results")
            recorded = json.loads(evidence.read_text(encoding="utf-8"))
            if recorded != {"counts": last["counts"], "exit_code": last["exit_code"]}:
                raise ValueError("Automated counts do not match original evidence")
        else:
            observation = json.loads(evidence.read_text(encoding="utf-8"))
            validate_observation(observation, by_id[item["id"]], report)
        age = (datetime.now(timezone.utc) - datetime.fromisoformat(last["finished_at"])).total_seconds()
        if not 0 <= age <= 72 * 3600:
            raise ValueError("Evidence is future-dated or older than 72 hours")
    return report


def validate_observation(value, case, report):
    if (value.get("run_id") != report["run_id"] or value.get("case") != case["id"]
            or value.get("status") not in ("pass", "fail", "blocked", "unconfirmed")
            or not str(value.get("operator", "")).strip()
            or not str(value.get("environment", "")).strip()
            or value.get("artifact_sha256") != report["artifact"].get("sha256")):
        raise ValueError("Observation must identify this run, case, tested artifact, operator and environment")
    if value["status"] == "pass":
        steps = value.get("steps", [])
        if (value.get("cleanup_verified") is not True or len(steps) != len(case["steps"])
                or any(s.get("step") != i or s.get("passed") is not True
                       or len(str(s.get("observed", "")).strip()) < 10 for i, s in enumerate(steps, 1))):
            raise ValueError("Every manual step needs an actual observation; cleanup must be verified")
        if case["id"] == "COMPAT-01":
            validate_compatibility(value)
        if case["id"] == "MCP-01":
            expected = set(json.loads((HOME / "tool-coverage.json").read_text(encoding="utf-8"))["tools"])
            tools = value.get("tools", [])
            if len(tools) != len(expected) or {t.get("name") for t in tools} != expected:
                raise ValueError("MCP observation requires one result for every registered tool")
            for tool in tools:
                if tool.get("status") not in ("pass", "not_applicable") or len(tool.get("observed", "").strip()) < 10:
                    raise ValueError("MCP tool results need actual postconditions or platform contract evidence")
                if tool["status"] == "not_applicable" and not tool.get("platform_contract"):
                    raise ValueError("Unsupported tools need explicit platform contract evidence")


def validate_compatibility(value):
    matrix = tomllib.loads((HOME / 'compatibility.toml').read_text(encoding='utf-8'))
    expected = {c['id']: c for c in matrix['combination']}
    observed = value.get('combinations', [])
    if len(observed) != len(expected) or {c.get('id') for c in observed} != set(expected):
        raise ValueError('Compatibility observation requires every declared combination exactly once')
    for cell in observed:
        if cell.get('outcome') not in ('pass', 'declared_rejection') or len(cell.get('observed', '').strip()) < 10:
            raise ValueError('Compatibility requires actual results for each combination')
        for role in ('management', 'relay', 'client'):
            artifact = cell.get(role, {})
            if not artifact.get('version') or not re.fullmatch(r'[a-f0-9]{64}', artifact.get('sha256', '')):
                raise ValueError('Compatibility must identify all three tested artifacts')
        if cell['outcome'] == 'declared_rejection' and (
                not expected[cell['id']]['allow_declared_rejection'] or not cell.get('unsupported_contract')):
            raise ValueError('Rejection needs an explicitly unsupported wire contract')


def run_case(case, item, path, report, lab):
    directory = path.parent / case["id"] / str(len(item["attempts"]) + 1)
    directory.mkdir(parents=True)
    attempt = {"kind": "automated", "status": "unconfirmed", "started_at": now()}
    item["attempts"].append(attempt)
    item["status"] = "unconfirmed"
    save(path, report)
    try:
        if case["adapter"].startswith("cargo-") and other_builds_running():
            raise ValueError("Another native build is active; wait for the shared build budget")
        if case["adapter"] == "android-relay":
            apk = ROOT / lab.get("android", {}).get("apk", "missing-artifact")
            if not apk.is_file() or digest(apk) != report["artifact"].get("sha256"):
                raise ValueError("Android test APK must match this report's candidate artifact")
        command, cwd, env, parser = command_for(case["adapter"], directory, lab)
    except (ValueError, OSError) as error:
        attempt.update(status="blocked", note=str(error), finished_at=now())
        item["status"] = "blocked"; save(path, report); return
    timeout = lab.get("execution", {}).get("timeout_seconds", 1800)
    if type(timeout) is not int or not 1 <= timeout <= 86400:
        raise ValueError("timeout_seconds must be 1..86400")
    raw = directory / "output.log"
    try:
        with raw.open("w", encoding="utf-8") as output:
            process = subprocess.Popen(command, cwd=cwd, env=env, stdout=output, stderr=subprocess.STDOUT,
                                       creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
            attempt["pid"] = process.pid
            save(path, report)
            try:
                returncode = process.wait(timeout=timeout)
            except (subprocess.TimeoutExpired, KeyboardInterrupt):
                process.kill(); process.wait()
                report["unresolved_resources"].append({"case": case["id"], "pid": process.pid,
                    "note": "Runner interrupted; inspect descendant processes and operations before resuming"})
                attempt.update(status="unconfirmed", note="Interrupted; no automatic retry", finished_at=now())
                return
        output = redact(raw.read_text(encoding="utf-8", errors="replace"))
        raw.write_text(output, encoding="utf-8")
        counts = counts_for(parser, directory, output)
        status = result_status(returncode, counts)
        evidence = directory / "result.json"
        evidence.write_text(json.dumps({"counts": counts, "exit_code": returncode}), encoding="utf-8")
        attempt.update(status=status, counts=counts, exit_code=returncode, finished_at=now(),
                       note=f"{counts['passed']} passed, {counts['failed']} failed, {counts['skipped']} skipped",
                       evidence=evidence.relative_to(path.parent).as_posix(), evidence_sha256=digest(evidence))
    except (OSError, ValueError, KeyError) as error:
        attempt.update(status="unconfirmed", note=redact(f"Result unavailable: {type(error).__name__}"), finished_at=now())
    finally:
        if raw.is_file():
            raw.write_text(redact(raw.read_text(encoding="utf-8", errors="replace")), encoding="utf-8")
        item["status"] = attempt["status"]
        save(path, report)


def other_builds_running():
    if os.name == "nt":
        output = subprocess.check_output(["powershell.exe", "-NoProfile", "-Command",
            "@(Get-CimInstance Win32_Process -Filter \"Name='rustc.exe' OR Name='cmake.exe' OR Name='ninja.exe' OR Name='cargo.exe'\" | "
            "Where-Object { $_.Name -ne 'cargo.exe' -or $_.CommandLine -match '\\s(build|test|check|clippy|rustc)\\s' }).Count"], text=True)
        return int(output.strip()) > 0
    output = subprocess.check_output(["ps", "-eo", "comm="], text=True)
    return any(Path(line.strip()).name in ("rustc", "cargo", "cmake", "ninja") for line in output.splitlines())


def main():
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8")
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="action", required=True)
    for action in ("plan", "preflight", "run"):
        cmd = sub.add_parser(action)
        cmd.add_argument("--target", choices=TARGETS, required=True)
        cmd.add_argument("--profile", choices=("quick", "release", "soak"), default="quick")
        cmd.add_argument("--lab", type=Path)
        if action == "run":
            cmd.add_argument("--arch", choices=("x86_64", "aarch64"), required=True)
            cmd.add_argument("--version", default="development")
            cmd.add_argument("--artifact", type=Path)
            cmd.add_argument("--case", action="append", default=[])
    for action in ("resume", "report", "record", "observation-template", "gate"):
        cmd = sub.add_parser(action)
        cmd.add_argument("--report", type=Path, required=True)
        if action == "resume":
            cmd.add_argument("--lab", type=Path)
            cmd.add_argument("--case", action="append", required=True)
        if action == "record":
            cmd.add_argument("--observation", type=Path, required=True)
        if action == "observation-template":
            cmd.add_argument("--case", required=True)
        if action == "gate":
            cmd.add_argument("--artifact", type=Path, required=True)
    args = parser.parse_args()
    lab = tomllib.loads(args.lab.read_text(encoding="utf-8")) if getattr(args, "lab", None) else {}
    if args.action in ("plan", "preflight"):
        blocked = False
        for case in selected(args.target, args.profile):
            note = "manual observation" if not case.get("adapter") else case["adapter"]
            if args.action == "preflight" and case.get("adapter"):
                try:
                    command_for(case["adapter"], HOME / "tmp/preflight", lab)
                except (ValueError, OSError, subprocess.SubprocessError) as error:
                    note = f"BLOCKED: {error}"; blocked = True
            print(f"{case['id']}  {case['title']}  [{note}]")
        return 2 if blocked else 0
    with lock():
        if args.action == "run":
            if args.profile != "quick" and (not args.artifact or args.version == "development"):
                raise ValueError("Release/soak runs require --artifact and --version")
            cases = selected(args.target, args.profile)
            report = {"schema_version": 1, "run_id": str(uuid.uuid4()), "started_at": now(),
                      "target": args.target, "arch": args.arch, "profile": args.profile, "version": args.version,
                      "source": source_state(), "catalog_sha256": digest(HOME / "cases.toml"),
                      "compatibility_sha256": digest(HOME / "compatibility.toml"),
                      "artifact": {"sha256": digest(args.artifact), "bytes": args.artifact.stat().st_size,
                                   "name": args.artifact.name} if args.artifact else {},
                      "cases": [{"id": c["id"], "status": "not_run", "attempts": []} for c in cases],
                      "unresolved_resources": []}
            path = HOME / "runs" / report["run_id"] / "report.json"
            save(path, report)
        else:
            path = local_report(args.report)
            report = json.loads(path.read_text(encoding="utf-8"))
            cases = required_results(report)
        if args.action == "report":
            print(path.with_suffix(".md").read_text(encoding="utf-8")); return 0
        if args.action == "gate":
            validate_release_report(path, platform=report["target"], arch=report["arch"], artifact=args.artifact,
                                    version=report["version"])
            print("PASS: release gate, artifact and evidence match"); return 0
        verify_binding(report)
        if args.action == "observation-template":
            case = next(c for c in cases if c["id"] == args.case)
            if case.get("adapter"):
                raise ValueError("Automated cases cannot use manual observations")
            value = {"run_id": report["run_id"], "case": case["id"], "artifact_sha256": report["artifact"].get("sha256"),
                     "operator": "", "environment": "", "status": "unconfirmed", "cleanup_verified": False,
                     "steps": [{"step": i, "action": step, "expected": case["expected"][i-1],
                                "passed": False, "observed": ""} for i, step in enumerate(case["steps"], 1)]}
            if case["id"] == "MCP-01":
                value["tools"] = [{"name": name, "status": "unconfirmed", "observed": "", "operation_id": "",
                                   "platform_contract": ""} for name in json.loads((HOME / "tool-coverage.json").read_text(encoding="utf-8"))["tools"]]
            if case['id'] == 'COMPAT-01':
                matrix = tomllib.loads((HOME / 'compatibility.toml').read_text(encoding='utf-8'))
                value['combinations'] = [dict(id=c['id'], outcome='unconfirmed', observed='', unsupported_contract='',
                    **{role: {'version': '', 'sha256': ''} for role in ('management', 'relay', 'client')})
                    for c in matrix['combination']]
            output = path.parent / f"{case['id']}-observation-template.json"
            output.write_text(json.dumps(value, ensure_ascii=False, indent=2), encoding="utf-8")
            print(output); return 0
        if args.action == "record":
            observation = json.loads(args.observation.read_text(encoding="utf-8"))
            case = next(c for c in cases if c["id"] == observation["case"])
            if case.get("adapter"):
                raise ValueError("Cannot replace automated checks with manual pass statements")
            validate_observation(observation, case, report)
            item = next(i for i in report["cases"] if i["id"] == case["id"])
            dest = path.parent / case["id"] / str(len(item["attempts"]) + 1) / "observation.json"
            dest.parent.mkdir(parents=True)
            dest.write_text(redact(json.dumps(observation, ensure_ascii=False, indent=2)), encoding="utf-8")
            item["status"] = observation["status"]
            item["attempts"].append({"kind": "manual", "status": observation["status"], "finished_at": now(),
                                     "note": "Recorded step-by-step native observations",
                                     "evidence": dest.relative_to(path.parent).as_posix(), "evidence_sha256": digest(dest)})
            save(path, report); print(path); return 0
        ids = set(args.case)
        if ids - {case["id"] for case in cases}:
            raise ValueError("Unknown or inapplicable --case")
        if report["unresolved_resources"]:
            raise ValueError("Prior interrupted run needs resource reconciliation; do not replay mutations")
        for case in cases:
            if not case.get("adapter") or (ids and case["id"] not in ids):
                continue
            item = next(i for i in report["cases"] if i["id"] == case["id"])
            if item["status"] == "pass":
                continue
            print(f"Running {case['id']}: {case['title']}", flush=True)
            run_case(case, item, path, report, lab)
            if report["unresolved_resources"]:
                break
        if report["source"] != source_state():
            report["unresolved_resources"].append({"note": "Source changed during run; all results require a new run"})
        save(path, report)
        print(path)
        return 0 if all(c["status"] == "pass" for c in report["cases"]) and not report["unresolved_resources"] else 2


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (ValueError, OSError, KeyError, StopIteration) as error:
        print(f"Smoke blocked: {redact(str(error))}", file=sys.stderr)
        raise SystemExit(2)
