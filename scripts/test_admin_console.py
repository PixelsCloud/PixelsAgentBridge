"""Run admin tests against an explicitly selected, isolated PostgreSQL instance.

Configuration is TOML. This helper neither installs Pixels nor alters production.
Example: python scripts/test_admin_console.py --config .build/admin-console/20261020/environment.toml --test relay_cluster
"""

import argparse
import hashlib
import json
import os
import re
from pathlib import Path
import subprocess
import sys
import time
import tomllib
import uuid
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[1]


def check_migration(config: dict, output: Path) -> int:
    """Preserve schema-7 fixtures through additive migrations and a backup restore."""
    container = config["container"]
    label = subprocess.check_output(["docker", "inspect", "--format", '{{index .Config.Labels "pixels.task"}}', container], text=True).strip()
    if label != "admin-console-20261020":
        raise RuntimeError("migration checks require this task's dedicated PostgreSQL container")
    database = "admin_migration_" + uuid.uuid4().hex
    restored = "admin_restore_" + uuid.uuid4().hex

    def sql(name: str, statement: str) -> bytes:
        return subprocess.check_output(["docker", "exec", "-i", container, "psql", "-X", "-v", "ON_ERROR_STOP=1", "-At", "-U", "postgres", "-d", name], input=statement.encode())

    sql("postgres", f'CREATE DATABASE "{database}"; CREATE DATABASE "{restored}";')
    migrations = sorted((ROOT / "crates/server/migrations").glob("*.sql"))
    for migration in migrations[:7]:
        sql(database, migration.read_text(encoding="utf-8"))
    user = str(uuid.uuid4())
    operation = str(uuid.uuid4())
    sql(database, f"""INSERT INTO server_settings DEFAULT VALUES;
      INSERT INTO users(id,username,username_key,password_hash,server_admin) VALUES('{user}','migration-fixture','migration-fixture','fixture-hash',true);
      INSERT INTO web_sessions(token_hash,user_id) VALUES(repeat('a',64),'{user}');
      INSERT INTO relay_cluster_nodes(node_id,public_url,weight,capacity_mbps) VALUES('migration-relay','https://migration.example/',100,100);
      INSERT INTO relay_maintenance_operations(id,node_id,action,phase,deadline_at,actor_id) VALUES('{operation}','migration-relay','maintenance','waiting',clock_timestamp()+interval '30 seconds','{user}');
      INSERT INTO web_admin_events(actor_id,action,resource_id) VALUES('{user}','legacy.fixture','{user}');""")
    fingerprint_sql = """SELECT jsonb_build_object('users',(SELECT jsonb_agg(jsonb_build_array(id,username,password_hash,server_admin,status,auth_revision)) FROM users),
      'sessions',(SELECT jsonb_agg(jsonb_build_array(token_hash,user_id,created_at)) FROM web_sessions),
      'operations',(SELECT jsonb_agg(jsonb_build_array(id,node_id,phase,deadline_at,actor_id)) FROM relay_maintenance_operations),
      'audit',(SELECT jsonb_agg(jsonb_build_array(id,actor_id,action,resource_id,created_at)) FROM web_admin_events));"""
    before = sql(database, fingerprint_sql)
    backup = subprocess.check_output(["docker", "exec", container, "pg_dump", "-U", "postgres", "-d", database, "-Fc"])
    (output / "migration-before.dump").write_bytes(backup)
    for migration in migrations[7:]:
        sql(database, migration.read_text(encoding="utf-8"))
    after = sql(database, fingerprint_sql)
    assert before == after, "existing accounts, session tokens or operations changed"
    assert sql(database, "SELECT offline_seconds IS NULL FROM relay_maintenance_operations;").strip() == b"t"
    assert sql(database, "SELECT id IS NOT NULL AND channel='unknown' AND last_active_at IS NULL FROM web_sessions;").strip() == b"t"
    assert sql(database, "SELECT format_version=0 AND actor_name IS NULL AND resource_name IS NULL AND before_values IS NULL AND after_values IS NULL AND request_id IS NULL AND insert_xid IS NULL FROM web_admin_events;").strip() == b"t"
    assert sql(database, "SELECT registration_override IS NULL AND registration_revision=1 FROM server_settings;").strip() == b"t"
    subprocess.run(["docker", "exec", "-i", container, "pg_restore", "-U", "postgres", "-d", restored], input=backup, check=True, capture_output=True)
    assert sql(restored, fingerprint_sql) == before, "isolated backup restore changed fixture data"
    report = {"suite": "migration", "status": "pass", "from_version": 7, "to_version": int(migrations[-1].name.split('_')[0]), "account_session_operation_fingerprint": hashlib.sha256(before).hexdigest(), "legacy_capacity": "unknown", "backup_restore": "pass", "application_rollback": "unconfirmed"}
    (output / "migration-result.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
    # Only these exact, freshly generated test databases are removed. Failed runs remain for diagnosis.
    sql("postgres", f'DROP DATABASE "{database}"; DROP DATABASE "{restored}";')
    print(json.dumps(report))
    return 0


def build_environment(database_url: str) -> dict[str, str]:
    env = dict(os.environ)
    if os.name == "nt":
        vswhere = Path(os.environ.get("ProgramFiles(x86)", "C:/Program Files (x86)")) / "Microsoft Visual Studio/Installer/vswhere.exe"
        installation = subprocess.check_output(
            [str(vswhere), "-latest", "-products", "*", "-requires", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64", "-property", "installationPath"],
            text=True,
        ).strip()
        if not installation:
            raise RuntimeError("Visual C++ build tools are required")
        devcmd = Path(installation) / "Common7/Tools/VsDevCmd.bat"
        result = subprocess.check_output(
            f'cmd.exe /d /s /c ""{devcmd}" -no_logo -arch=x64 -host_arch=x64 >nul && set"',
            text=True,
        )
        for line in result.splitlines():
            key, separator, value = line.partition("=")
            if separator and key:
                # Windows environment names are case insensitive; Python dictionaries are not.
                for previous in list(env):
                    if previous.lower() == key.lower():
                        del env[previous]
                env[key] = value
        env["CMAKE_GENERATOR"] = "Ninja"
    env.update(
        DATABASE_URL=database_url,
        CARGO_BUILD_JOBS="2",
        CMAKE_BUILD_PARALLEL_LEVEL="1",
        RAYON_NUM_THREADS="2",
        CARGO_PROFILE_DEV_DEBUG="0",
        CARGO_PROFILE_DEV_INCREMENTAL="false",
    )
    return env


def source_hashes() -> dict[str, str]:
    files = (list((ROOT / "crates/server/src").rglob("*.rs"))
             + list((ROOT / "crates/server/migrations").glob("*.sql"))
             + list((ROOT / "crates/server/tests").rglob("*.rs"))
             + [ROOT / "crates/server/Cargo.toml", ROOT / "Cargo.lock"])
    if (ROOT / "crates/server/build.rs").is_file():
        files.append(ROOT / "crates/server/build.rs")
    return {str(path.relative_to(ROOT)).replace('\\', '/'): hashlib.sha256(path.read_bytes()).hexdigest() for path in files}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--test", default="relay_cluster", choices=["relay_cluster", "relay_cluster_e2e", "control_tls", "web_management", "web_lib", "server_settings", "migration", "all"])
    parser.add_argument("--test-threads", type=int, choices=[1, 2], default=2,
                        help="Use one runtime test worker when measuring deadline-sensitive Relay batches")
    args = parser.parse_args()
    args.config = args.config.resolve()
    config = tomllib.loads(args.config.read_text(encoding="utf-8"))
    output = args.config.resolve().parent
    # Always require the dedicated test database contract to avoid running on a default URL.
    if config.get("isolated") is not True or not config["database_url"].startswith("postgres://postgres@127.0.0.1:"):
        parser.error("an explicit isolated loopback test database is required")
    if config.get("container"):
        info = json.loads(subprocess.check_output(["docker", "inspect", config["container"]], text=True))[0]
        bindings = info["NetworkSettings"]["Ports"].get("5432/tcp") or []
        if not info["State"]["Running"] or not any(b["HostIp"] == "127.0.0.1" and int(b["HostPort"]) == urlsplit(config["database_url"]).port for b in bindings):
            parser.error("the test database container is stopped or its mapped port changed; update the test TOML before running")
    if args.test == "all":
        for suite in ["web_management", "web_lib", "control_tls", "relay_cluster", "server_settings", "relay_cluster_e2e", "migration"]:
            result = subprocess.run([sys.executable, __file__, "--config", str(args.config), "--test", suite, "--test-threads", str(args.test_threads)], cwd=ROOT)
            if result.returncode:
                return result.returncode
        return 0
    if args.test == "migration":
        return check_migration(config, output)
    if os.name == "nt":
        announced = False
        while subprocess.check_output([
            "powershell.exe", "-NoProfile", "-Command",
            "@(Get-CimInstance Win32_Process -Filter \"Name='cargo.exe' OR Name='rustc.exe' OR Name='cmake.exe' OR Name='ninja.exe'\" | Where-Object { $_.Name -ne 'cargo.exe' -or $_.CommandLine -match '\\s(build|test|check|clippy|rustc)\\s' }).Count",
        ], text=True).strip() != "0":
            if not announced:
                print("Waiting for other Cargo builds to finish; total build concurrency must stay within 10.", flush=True)
                announced = True
            time.sleep(5)
    started = time.monotonic()
    # Some native build scripts pass NUM_JOBS directly to CMake, overriding its
    # environment limit. Two Cargo jobs keep those nested builds bounded too.
    selector = ["--lib"] if args.test == "web_lib" else ["--test", args.test]
    command = ["cargo", "test", "-p", "pab-server", *selector, "--jobs", "2", "--", f"--test-threads={args.test_threads}", "--nocapture"]
    before = source_hashes()
    with (output / f"{args.test}.log").open("w", encoding="utf-8") as log:
        result = subprocess.run(command, cwd=ROOT, env=build_environment(config["database_url"]), stdout=log, stderr=subprocess.STDOUT)
    summary = re.search(r"test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored", (output / f"{args.test}.log").read_text(encoding="utf-8"))
    after = source_hashes()
    report = {"suite": args.test, "command": command, "status": "pass" if result.returncode == 0 and before == after else "fail", "exit_code": result.returncode, "elapsed_seconds": round(time.monotonic() - started, 2), "log": f"{args.test}.log", "source_changed_during_test": before != after, "tests": {"passed": int(summary[1]), "failed": int(summary[2]), "ignored": int(summary[3])} if summary else None}
    (output / f"{args.test}-source.json").write_text(json.dumps(before, indent=2), encoding="utf-8")
    (output / f"{args.test}-result.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
    print(json.dumps(report))
    return result.returncode if before == after else 1


if __name__ == "__main__":
    raise SystemExit(main())
