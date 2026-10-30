"""Run Executor unit tests and, with explicit opt-in, each native test separately.

Separate processes matter: some native tests require an unregistered desktop helper.
An ignored native test is never silently converted into a passing unit test.
"""
from pathlib import Path
import argparse
import json
import os
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from smoke.adapters import counts_for
from smoke.cargo import environment, test_command


def execute(binary, directory, native, unit_user=None):
    directory.mkdir(parents=True, exist_ok=True)
    listing = subprocess.run([str(binary), "--ignored", "--list", "--format=terse"],
                             capture_output=True, text=True, encoding="utf-8", check=True)
    ignored = [line.removesuffix(": test") for line in listing.stdout.splitlines() if line.endswith(": test")]
    counts = {"passed": 0, "failed": 0, "skipped": 0}
    commands = [("unit", [str(binary), "--test-threads=1"])]
    if native:
        commands += [(f"native-{index:02}", [str(binary), name, "--exact", "--ignored", "--test-threads=1"])
                     for index, name in enumerate(ignored)]
    (directory / "native-tests.json").write_text(json.dumps(ignored, indent=2), encoding="utf-8")
    for label, command in commands:
        if label == "unit" and unit_user is not None:
            from smoke.windows_test_user import run
            result = run(command, directory / "desktop-user", unit_user)
        else:
            result = subprocess.run(command, capture_output=True, encoding="utf-8", errors="replace")
        output = result.stdout + result.stderr
        (directory / (label + ".log")).write_text(output, encoding="utf-8")
        row = counts_for("libtest", directory, output)
        if label == "unit" and native:
            if row["skipped"] != len(ignored):
                raise RuntimeError("Ignored test discovery differs from unit results")
            # Every discovered ignored test now has its own mandatory execution below.
            row["skipped"] = 0
        if result.returncode and row["failed"] == 0:
            row["failed"] = 1
        if label != "unit" and sum(row.values()) != 1:
            row["failed"] += 1
        if not sum(row.values()):
            row["failed"] = 1
        for key in counts:
            counts[key] += row[key]
        (directory / "counts.json").write_text(json.dumps(counts), encoding="utf-8")
        print(label, json.dumps(row), flush=True)
    return 0 if counts["failed"] == 0 else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--binary", type=Path, help="Prebuilt native Executor libtest executable on the test host")
    parser.add_argument("--native", action="store_true", help="Explicitly enable all native tests on a prepared test host")
    parser.add_argument("--unit-user", type=int, help="Windows interactive session for desktop unit tests; native tests keep the service identity")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    binary = args.binary
    if binary is None:
        result = subprocess.run(test_command(["-p", "pab-executor", "--lib",
                                 "--no-run", "--message-format=json"]), cwd=ROOT, capture_output=True,
                                encoding="utf-8", errors="replace", env=environment())
        (args.output / "build.log").write_text(result.stdout + result.stderr, encoding="utf-8")
        if result.returncode:
            raise RuntimeError("Executor test build failed; inspect build.log")
        binaries = []
        for line in result.stdout.splitlines():
            try:
                item = json.loads(line)
            except json.JSONDecodeError:
                continue
            if item.get("executable") and item.get("profile", {}).get("test"):
                binaries.append(Path(item["executable"]))
        if len(binaries) != 1:
            raise RuntimeError("Expected exactly one Executor libtest binary")
        binary = binaries[0]
    return execute(binary.resolve(strict=True), args.output, args.native, args.unit_user)


if __name__ == "__main__":
    raise SystemExit(main())
