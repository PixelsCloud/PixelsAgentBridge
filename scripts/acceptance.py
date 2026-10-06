"""Repeatable local regression reports. Never installs, logs out or restarts a device.

Native-host evidence can be recorded alongside automated tests with `record`.
No report includes tool arguments, output contents, environment variables or credentials.
"""
from argparse import ArgumentParser
from datetime import datetime, timezone
from pathlib import Path
import json
import os
import re
import subprocess
import sys
import uuid

ROOT = Path(__file__).resolve().parents[1]
STATUSES = ("pass", "fail", "skip", "blocked", "unconfirmed")
SUITES = ("isolated", "live-basic", "live-desktop", "lifecycle", "upgrade")


def now():
    return datetime.now(timezone.utc).isoformat()


def test_results(output):
    """Only stable libtest result lines; never collect panic/fixture contents."""
    return [{"case": match[0], "status": {"ok": "pass", "FAILED": "fail", "ignored": "skip"}[match[1]]}
            for match in re.findall(r"^test ([\w:]+) \.\.\. (ok|FAILED|ignored)\b", output, re.M)]


def save(directory, report):
    directory.mkdir(parents=True, exist_ok=True)
    target = directory / "report.json"
    temporary = directory / "report.json.tmp"
    temporary.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    temporary.replace(target)
    lines = [f"# Acceptance {report['run_id']}", "", f"Suite: {report['suite']}",
             f"Commit: {report['commit']}; version: {report['version']}", "",
             "| Case | Status | Evidence |", "|---|---|---|"]
    for case in report["cases"]:
        evidence = case.get("evidence", "")
        lines.append(f"| {case['case']} | {case['status']} | {evidence.replace('|', '/').replace(chr(10), ' ')} |")
    lines.extend(["", "Uncleaned resources: " + json.dumps(report["uncleaned_resources"], ensure_ascii=False), ""])
    (directory / "report.md").write_text("\n".join(lines), encoding="utf-8")


def create_report(suite):
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    return {"schema_version": 1, "run_id": str(uuid.uuid4()), "suite": suite,
            "commit": commit, "version": json.loads((ROOT / "build-version.json").read_text())["version"],
            "worktree_dirty": bool(subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT)),
            "started_at": now(), "cases": [], "uncleaned_resources": []}


def run_suite(suite, timeout):
    report = create_report(suite)
    directory = ROOT / ".build" / "acceptance" / report["run_id"]
    save(directory, report)
    env = os.environ.copy()
    env["PAB_TEST_CATALOG_PATH"] = str(directory / "catalog.json")
    if suite == "isolated":
        commands = [
            ["cargo", "test", "-p", "pab-protocol", "-p", "pab-desktop-control", "-p", "pab-terminal", "--lib"],
            ["cargo", "test", "-p", "pab-bridge", "--bin", "pab-mcp"],
            ["cargo", "test", "-p", "pab-bridge", "--test", "mcp_operations"],
            ["cargo", "test", "-p", "pab-executor", "--lib"],
        ]
    elif suite == "live-basic":
        required = [key for key in ("PAB_TEST_DEVICE_CODE", "PAB_TEST_PATH", "PAB_DATA_DIR") if not env.get(key)]
        if required:
            report["cases"].append({"case": "live prerequisites", "status": "blocked",
                                    "evidence": "Missing " + ", ".join(required)})
            report["finished_at"] = now()
            save(directory, report)
            return 2, directory
        commands = [["cargo", "test", "-p", "pab-bridge", "--test", "mcp_endpoint_live", "--", "--ignored",
                     "--skip", "monitor_click_reaches_owned_application_fixture"]]
    else:
        report["cases"].append({"case": "native host acceptance", "status": "blocked",
                                "evidence": "Requires native pixels.pab_* fixture observations; record them with the record command. No desktop/lifecycle mutations were executed."})
        report["finished_at"] = now()
        save(directory, report)
        return 2, directory
    for index, command in enumerate(commands):
        item = {"case": " ".join(command), "status": "unconfirmed", "started_at": now()}
        report["cases"].append(item)
        save(directory, report)  # Preserve unconfirmed if this driver is interrupted.
        log = directory / f"test-{index}.log"
        with log.open("w", encoding="utf-8") as stream:
            process = subprocess.Popen(command, cwd=ROOT, env=env, stdout=stream, stderr=subprocess.STDOUT)
            try:
                code = process.wait(timeout=timeout)
            except subprocess.TimeoutExpired:
                # Killing cargo cannot prove every child exited; record, never replay.
                process.kill()
                process.wait()
                item["evidence"] = "Driver timeout; inspect child processes and original operations. No automatic replay."
                report["uncleaned_resources"].append({"kind": "possible test descendants", "parent_pid": process.pid})
                save(directory, report)
                break
        output = log.read_text(encoding="utf-8", errors="replace")
        cases = test_results(output)
        item.update(status="pass" if code == 0 and any(c["status"] == "pass" for c in cases) else "fail",
                    exit_code=code, finished_at=now(), evidence=log.name, tests=cases)
        save(directory, report)
        if code != 0:
            break
    report["finished_at"] = now()
    save(directory, report)
    return (0 if all(c["status"] == "pass" for c in report["cases"]) else 1), directory


def main():
    parser = ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    run = commands.add_parser("run")
    run.add_argument("suite", choices=SUITES)
    run.add_argument("--timeout", type=int, default=3600)
    record = commands.add_parser("record", help="Record verified native-host observations; never include secrets or typed content")
    record.add_argument("suite", choices=SUITES)
    record.add_argument("--case", required=True)
    record.add_argument("--status", choices=STATUSES, required=True)
    record.add_argument("--evidence", required=True, help="Nonsecret result summary, not raw tool arguments/output")
    record.add_argument("--device-code", required=True)
    record.add_argument("--operation-id", action="append", default=[])
    record.add_argument("--uncleaned-resource", action="append", default=[])
    args = parser.parse_args()
    if args.command == "run":
        if args.timeout <= 0:
            parser.error("timeout must be positive")
        code, directory = run_suite(args.suite, args.timeout)
    else:
        if not re.fullmatch(r"\d{9}", args.device_code):
            parser.error("device code must contain 9 digits")
        for operation in args.operation_id:
            uuid.UUID(operation)
        report = create_report(args.suite)
        report.update(finished_at=now(), uncleaned_resources=args.uncleaned_resource)
        report["cases"].append({"case": args.case, "status": args.status, "evidence": args.evidence,
                                "device_code": args.device_code, "operation_ids": args.operation_id,
                                "source": "native_host_observation"})
        directory = ROOT / ".build" / "acceptance" / report["run_id"]
        save(directory, report)
        code = 0
    print(directory)
    return code


if __name__ == "__main__":
    sys.exit(main())
