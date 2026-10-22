"""Exercise real old/new server processes on one task-owned database, without restore.

The unmodified baseline is tested first. The rollback build adds the new SQL
migrations, their startup compatibility adapter and directory tracking. Existing
business code remains unchanged.
All processes, credentials, backups and configuration stay in the selected fixture.
"""
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time
import tomllib
from urllib.parse import urlsplit
import uuid

import requests

from service_config import write_config
from test_admin_console import ROOT, build_environment


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--environment', type=Path, required=True)
    parser.add_argument('--new-binary-dir', type=Path, required=True)
    parser.add_argument('--old-source', type=Path, required=True)
    parser.add_argument('--old-target', type=Path, required=True)
    parser.add_argument('--old-binary', type=Path)
    parser.add_argument('--rollback-profile', choices=['debug', 'release'], default='debug')
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    environment = args.environment.resolve()
    environment.relative_to(ROOT / '.build/admin-console')
    output = args.output.resolve() if args.output else environment.parent / 'upgrade'
    output.relative_to(environment.parent)
    output.mkdir(exist_ok=False)
    old_source = args.old_source.resolve()
    old_target = args.old_target.resolve()
    for path in [old_source, old_target]:
        path.relative_to(environment.parent)
    assert (environment.parent / 'old-build-result.json').is_file()
    assert json.loads((environment.parent / 'old-build-result.json').read_text())['status'] == 'pass'
    fixture_environment = output / 'environment.toml'
    shutil.copy2(environment, fixture_environment)
    # A previous compatibility build may have replaced old-target/debug. Use
    # the preserved unmodified binary for both initialization and startup.
    old_binary = args.old_binary.resolve() if args.old_binary else old_target / 'debug/pab-server.exe'
    old_binary.relative_to(environment.parent)
    baseline_bins = output / 'baseline-bin'
    baseline_bins.mkdir()
    shutil.copy2(old_binary, baseline_bins / 'pab-server.exe')
    subprocess.run([
        sys.executable, str(ROOT / 'scripts/serve_admin_console_test.py'),
        '--config', str(fixture_environment), '--port', '38625',
        '--binary-dir', str(baseline_bins), '--prepare-only',
    ], check=True, cwd=ROOT)
    config_path = output / 'https/pab-server.toml'
    settings = tomllib.loads(config_path.read_text(encoding='utf-8'))
    env = tomllib.loads(environment.read_text(encoding='utf-8'))
    database = urlsplit(settings['database']['url']).path[1:]
    assert database.startswith('admin_browser_')
    baseline = output / 'old-original.exe'
    current = output / 'current.exe'
    old_binary = args.old_binary.resolve() if args.old_binary else old_target / 'debug/pab-server.exe'
    old_binary.relative_to(environment.parent)
    shutil.copy2(old_binary, baseline)
    shutil.copy2(args.new_binary_dir.resolve() / 'pab-server.exe', current)
    report = {'status': 'running', 'checks': {}, 'database_restore_during_rollback': False}
    processes = []
    logs = []

    def sql(statement, database_name=database):
        return subprocess.check_output([
            'docker', 'exec', '-i', env['container'], 'psql', '-X', '-At',
            '-v', 'ON_ERROR_STOP=1', '-U', 'postgres', '-d', database_name,
        ], input=statement.encode()).decode().strip()

    def launch(binary, label):
        log = (output / f'{label}.log').open('ab')
        logs.append(log)
        process = subprocess.Popen(
            [str(binary), 'serve', '--config', str(config_path)], cwd=ROOT,
            stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT,
            creationflags=subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0,
        )
        processes.append(process)
        deadline = time.monotonic() + 30
        while process.poll() is None:
            try:
                if requests.get('https://127.0.0.1:38625/health', verify=settings['tls']['cert'], timeout=1).status_code == 204:
                    return process
            except requests.RequestException:
                pass
            assert time.monotonic() < deadline, f'{label} startup timed out'
            time.sleep(.1)
        raise AssertionError(f'{label} exited before readiness')

    def stop(process):
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=15)

    def request(client, method, path, expected=200, **kwargs):
        response = client.request(
            method, 'https://127.0.0.1:38625' + path,
            verify=settings['tls']['cert'], timeout=20,
            headers={'Origin': 'https://127.0.0.1:38625'}, **kwargs,
        )
        assert response.status_code == expected, (method, path, response.status_code)
        return response

    def digest(text):
        return hashlib.sha256(text.encode()).hexdigest()

    identity_sql = """SELECT jsonb_build_object(
      'users',(SELECT jsonb_agg(jsonb_build_array(id,username,password_hash,server_admin,status,auth_revision) ORDER BY id) FROM users),
      'tenants',(SELECT jsonb_agg(jsonb_build_array(id,kind,created_by_user_id) ORDER BY id) FROM tenants),
      'devices',(SELECT jsonb_agg(jsonb_build_array(id,code,tenant_id,revision,name) ORDER BY id) FROM devices),
      'endpoints',(SELECT jsonb_agg(jsonb_build_array(encode(endpoint_key,'hex'),tenant_id,device_id,status,revision) ORDER BY endpoint_key) FROM endpoints),
      'sessions',(SELECT jsonb_agg(jsonb_build_array(token_hash,user_id,created_at) ORDER BY token_hash) FROM web_sessions));"""
    try:
        old = launch(baseline, 'old-before-upgrade')
        assert sql('SELECT max(version) FROM _sqlx_migrations;') == '7'
        admin = requests.Session()
        ordinary = requests.Session()
        password = os.urandom(24).hex()
        replacement = os.urandom(24).hex()
        request(admin, 'POST', '/api/web/register', json={'username': 'upgrade-admin', 'password': password})
        account = request(ordinary, 'POST', '/api/web/register', json={'username': 'upgrade-user', 'password': password}).json()
        subprocess.run([str(baseline), 'web-admin', 'upgrade-admin', '--config', str(config_path)], check=True, capture_output=True)
        tenant = str(uuid.uuid4())
        device = str(uuid.uuid4())
        sql(f"""INSERT INTO tenants(id,kind) VALUES('{tenant}','unclaimed_device');
          INSERT INTO devices(id,code,tenant_id,name) VALUES('{device}',876543210,'{tenant}','upgrade fixture');
          INSERT INTO endpoints(endpoint_key,tenant_id,owner_kind,device_id) VALUES(decode(repeat('ab',32),'hex'),'{tenant}','device','{device}');""")
        before = sql(identity_sql)
        (output / 'before-upgrade.dump').write_bytes(subprocess.check_output([
            'docker', 'exec', env['container'], 'pg_dump', '-U', 'postgres', '-d', database, '-Fc',
        ]))
        stop(old)
        initialized = subprocess.run([str(current), 'init', '--config', str(config_path)], capture_output=True)
        (output / 'new-init.log').write_bytes(initialized.stdout + initialized.stderr)
        initialized.check_returncode()
        assert sql('SELECT max(version) FROM _sqlx_migrations;') == '11'
        assert before == sql(identity_sql)
        report['checks']['schema_7_to_11_identity_and_session_fingerprint'] = digest(before)
        new = launch(current, 'new-after-upgrade')
        assert request(ordinary, 'GET', '/api/web/session').json()['id'] == account['id']
        assert request(admin, 'GET', '/api/web/devices?scope=all').json()['total'] == 1
        new_user = requests.Session()
        post = request(new_user, 'POST', '/api/web/register', json={'username': 'after-upgrade-user', 'password': password}).json()
        revision = request(admin, 'GET', f"/api/web/accounts/{account['id']}").json()['revision']
        request(admin, 'POST', f"/api/web/accounts/{account['id']}/password-reset", json={
            'operation_id': str(uuid.uuid4()), 'revision': revision,
            'new_password': replacement, 'revoke_sessions': True,
        })
        request(ordinary, 'GET', '/api/web/session', 401)
        survivor = requests.Session()
        request(survivor, 'POST', '/api/web/session', json={'username': 'upgrade-user', 'password': replacement})
        revoked = requests.Session()
        request(revoked, 'POST', '/api/web/session', json={'username': 'after-upgrade-user', 'password': password})
        # Selective revocation must remain selective after rolling back.
        public_sessions = request(admin, 'GET', f"/api/web/accounts/{post['id']}/sessions").json()['items']
        newest = max(public_sessions, key=lambda item: item['created_at'])
        request(admin, 'POST', f"/api/web/accounts/{post['id']}/sessions/revoke", json={
            'operation_id': str(uuid.uuid4()), 'scope': 'session', 'session_id': newest['id'],
        })
        request(revoked, 'GET', '/api/web/session', 401)
        request(new_user, 'GET', '/api/web/session')
        policy = request(admin, 'GET', '/api/web/service').json()['registration']
        request(admin, 'PATCH', '/api/web/service', json={'revision': policy['revision'], 'registration_override': False})
        post_upgrade = sql(identity_sql)
        audit_before_rollback = sql("SELECT count(*) FROM web_admin_events;")
        stop(new)
        # An old application ignores the runtime override. Preserve the effective
        # closed policy in the rollback TOML, without changing the database row.
        settings['web']['registration_enabled'] = False
        write_config(config_path, settings)
        original_check = subprocess.run([str(baseline), 'init', '--config', str(config_path)], capture_output=True)
        (output / 'unmodified-old-startup.log').write_bytes(original_check.stdout + original_check.stderr)
        report['unmodified_old_binary'] = {
            'exit_code': original_check.returncode,
            'status': 'refuses_newer_migration_history' if original_check.returncode else 'starts',
        }
        rollback = baseline
        if original_check.returncode:
            error = (original_check.stdout + original_check.stderr).decode(errors='replace')
            assert 'migration' in error.lower(), 'unexpected old startup failure'
            # Backport only forward-compatible migration files; never erase the
            # SQLx history or restore a pre-upgrade database to force a rollback.
            copied = {}
            for migration in sorted((ROOT / 'crates/server/migrations').glob('*.sql'))[7:]:
                destination = old_source / 'crates/server/migrations' / migration.name
                if destination.exists():
                    assert destination.read_bytes() == migration.read_bytes()
                else:
                    shutil.copy2(migration, destination)
                copied[migration.name] = hashlib.sha256(migration.read_bytes()).hexdigest()
            report['rollback_compatibility_migrations'] = copied
            # The rollback startup adapter also accepts exact historical LF/
            # CRLF encodings. All other old business code remains unchanged.
            migration_adapter = ROOT / 'crates/server/src/postgres/migration.rs'
            shutil.copy2(migration_adapter, old_source / 'crates/server/src/postgres/migration.rs')
            shutil.copy2(ROOT / 'crates/server/build.rs', old_source / 'crates/server/build.rs')
            old_module = old_source / 'crates/server/src/postgres/mod.rs'
            old_text = old_module.read_text(encoding='utf-8')
            if 'mod migration;' not in old_text:
                old_text = old_text.replace('mod endpoints;', 'mod endpoints;\nmod migration;')
                before_method = 'sqlx::migrate!("./migrations").run(&self.pool).await?;\n        Ok(())'
                assert before_method in old_text
                old_text = old_text.replace(before_method, 'migration::run(&self.pool).await')
                old_module.write_text(old_text, encoding='utf-8', newline='\n')
            report['rollback_startup_adapter_sha256'] = hashlib.sha256(migration_adapter.read_bytes()).hexdigest()
            build_env = build_environment(settings['database']['url'])
            build_env['CARGO_TARGET_DIR'] = str(old_target)
            # Historical Cargo has no directory-change build script. Updating
            # only the mtime forces re-expansion without changing old Rust code.
            os.utime(old_source / 'crates/server/src/postgres/mod.rs', None)
            with (output / 'rollback-compat-build.log').open('wb') as log:
                command = ['cargo', 'build', '--locked', '-p', 'pab-server', '--bin', 'pab-server', '--jobs', '2']
                if args.rollback_profile == 'release':
                    command.append('--release')
                subprocess.run(command, cwd=old_source, env=build_env, stdout=log, stderr=subprocess.STDOUT, check=True)
            rollback = output / 'old-compatible.exe'
            shutil.copy2(old_target / args.rollback_profile / 'pab-server.exe', rollback)
            report['rollback_profile'] = args.rollback_profile
        original_assets = settings['web']['assets']
        old_assets = old_source / 'apps/web/dist'
        assert (old_assets / 'index.html').is_file(), 'Build the baseline Web assets before the rollback drill'
        settings['web']['assets'] = str(old_assets)
        write_config(config_path, settings)
        rolled_back = launch(rollback, 'old-after-rollback')
        assert request(admin, 'GET', '/settings').content == (old_assets / 'index.html').read_bytes()
        report['checks']['old_frontend_and_backend_together'] = True
        assert post_upgrade == sql(identity_sql)
        report['checks']['post_upgrade_data_preserved_without_restore'] = digest(post_upgrade)
        request(admin, 'GET', '/api/web/session')
        assert request(survivor, 'GET', '/api/web/session').json()['id'] == account['id']
        assert request(new_user, 'GET', '/api/web/session').json()['id'] == post['id']
        request(ordinary, 'GET', '/api/web/session', 401)
        request(revoked, 'GET', '/api/web/session', 401)
        fresh = requests.Session()
        request(fresh, 'POST', '/api/web/session', 401, json={'username': 'upgrade-user', 'password': password})
        request(fresh, 'POST', '/api/web/session', json={'username': 'upgrade-user', 'password': replacement})
        assert request(fresh, 'GET', '/api/web/config').json()['registration_enabled'] is False
        for path in ['/api/web/register', '/api/account/register']:
            request(fresh, 'POST', path, 403, json={'username': 'must-stay-closed', 'password': password})
        assert sql('SELECT count(*) FROM web_admin_events;') == audit_before_rollback
        report['checks'].update(old_session_retained=True, new_account_retained=True,
            reset_password_retained=True, revoked_sessions_stay_revoked=True,
            registration_stays_closed=True, audit_retained=True)
        stop(rolled_back)
        # A separately initialized current database may have different embedded
        # line endings from the Git-archive baseline. Validate the rollback
        # startup adapter there too; neither database's history is rewritten.
        variant_database = 'admin_browser_' + uuid.uuid4().hex
        sql(f'CREATE DATABASE "{variant_database}";')
        variant_settings = copy.deepcopy(settings)
        variant_settings['database']['url'] = settings['database']['url'].rsplit('/', 1)[0] + '/' + variant_database
        variant_config = write_config(output / 'current-history-variant.toml', variant_settings)
        subprocess.run([str(current), 'init', '--config', str(variant_config)], check=True, capture_output=True)
        checksum_query = "SELECT jsonb_agg(jsonb_build_array(version,encode(checksum,'hex')) ORDER BY version) FROM _sqlx_migrations;"
        checksums = sql(checksum_query, variant_database)
        subprocess.run([str(rollback), 'init', '--config', str(variant_config)], check=True, capture_output=True)
        assert checksums == sql(checksum_query, variant_database)
        report['checks']['rollback_accepts_current_embedded_newline_history'] = digest(checksums)
        # Return to the new application without rewriting the runtime override.
        settings['web'].pop('registration_enabled')
        settings['web']['assets'] = original_assets
        write_config(config_path, settings)
        recovered = launch(current, 'new-after-rollback-drill')
        assert request(admin, 'GET', '/api/web/service').json()['registration']['override_value'] is False
        request(survivor, 'GET', '/api/web/session')
        request(revoked, 'GET', '/api/web/session', 401)
        report['checks']['forward_again'] = True
        report.update(status='pass', binary_sha256={
            path.name: hashlib.sha256(path.read_bytes()).hexdigest()
            for path in [baseline, current, rollback]
        })
    except Exception as error:
        report.update(status='fail', error_class=type(error).__name__)
        raise
    finally:
        for process in processes:
            stop(process)
        for log in logs:
            log.close()
        report['owned_processes_stopped'] = all(process.poll() is not None for process in processes)
        (output / 'result.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
    print(json.dumps(report))


if __name__ == '__main__':
    main()
