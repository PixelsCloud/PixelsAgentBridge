"""Bounded load/export acceptance against a fresh, task-owned loopback fixture."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import csv
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import statistics
import subprocess
import sys
import threading
import time
import tomllib
from urllib.parse import urlsplit

import psutil
import requests

from test_admin_console import ROOT
from admin_console_resources import OwnedProcessMemoryLimit
from service_config import write_config


def percentile(values, fraction):
    ordered = sorted(values)
    return round(ordered[max(0, int(len(ordered) * fraction + .999999) - 1)], 2)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--environment', type=Path, required=True)
    parser.add_argument('--binary-dir', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    environment = args.environment.resolve()
    environment.relative_to(ROOT / '.build/admin-console')
    output = args.output.resolve()
    output.relative_to(environment.parent)
    output.mkdir(exist_ok=False)
    shutil.copy2(environment, output / 'environment.toml')
    subprocess.run([
        sys.executable, str(ROOT / 'scripts/serve_admin_console_test.py'),
        '--config', str(output / 'environment.toml'), '--port', '38626',
        '--binary-dir', str(args.binary_dir.resolve()), '--prepare-only',
    ], check=True, cwd=ROOT)
    config_path = output / 'https/pab-server.toml'
    config = tomllib.loads(config_path.read_text(encoding='utf-8'))
    environment_data = tomllib.loads(environment.read_text(encoding='utf-8'))
    database = urlsplit(config['database']['url']).path[1:]
    binary = output / 'https/pab-server.exe'
    report = {'status': 'running', 'concurrent_reads': 20, 'read_threshold_p95_ms': 500,
              'cache_scope': 'first application wave after restarting owned PostgreSQL shared buffers, then warmed waves; host filesystem cache not flushed; earlier endpoints may warm shared tables',
              'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
              'environment': {'server_cpu_limit': 2, 'server_memory_limit': 'not capped',
                              'reference_2vcpu_4gb_equivalence': False}}
    processes = []
    memory_limits = []
    logs = []
    session = requests.Session()
    origin = 'https://127.0.0.1:38626'

    def request(method, path, expected=200, **kwargs):
        response = session.request(method, origin + path, verify=config['tls']['cert'],
            headers={'Origin': origin}, timeout=30, **kwargs)
        assert response.status_code == expected, (method, path, response.status_code)
        return response

    def sql(statement):
        return subprocess.check_output(['docker', 'exec', '-i', environment_data['container'],
            'psql', '-X', '-At', '-v', 'ON_ERROR_STOP=1', '-U', 'postgres', '-d', database],
            input=statement.encode()).decode().strip()

    def start(label):
        log = (output / f'{label}.log').open('ab')
        logs.append(log)
        parent = psutil.Process()
        affinity = parent.cpu_affinity() if hasattr(parent, 'cpu_affinity') else None
        try:
            if affinity:
                # Constrain CPU execution; this does not set Tokio worker count.
                parent.cpu_affinity(affinity[:2])
            process = subprocess.Popen([str(binary), 'serve', '--config', str(config_path)],
                cwd=ROOT, stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT,
                creationflags=subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0)
        finally:
            if affinity:
                parent.cpu_affinity(affinity)
        processes.append(process)
        if os.name == 'nt':
            memory_limits.append(OwnedProcessMemoryLimit(process, 4 * 1024 ** 3))
            report['environment']['server_memory_limit'] = 'Windows job private commit, 4 GiB; host file cache excluded'
            report['environment']['server_memory_limit_bytes'] = 4 * 1024 ** 3
        monitor = psutil.Process(process.pid)
        if hasattr(monitor, 'cpu_affinity'):
            monitor.cpu_affinity(monitor.cpu_affinity()[:2])
        deadline = time.monotonic() + 30
        while process.poll() is None:
            try:
                if request('GET', '/health', 204).status_code == 204:
                    report['environment']['server_threads_at_ready'] = monitor.num_threads()
                    report['environment']['server_cpu_affinity'] = monitor.cpu_affinity() if affinity else None
                    return process, monitor
            except requests.RequestException:
                pass
            assert time.monotonic() < deadline
            time.sleep(.1)
        raise AssertionError('load fixture failed to start')

    def timed_get(path):
        with requests.Session() as reader:
            reader.cookies.update(session.cookies)
            started = time.perf_counter()
            response = reader.get(origin + path, verify=config['tls']['cert'], timeout=30)
            elapsed = (time.perf_counter() - started) * 1000
            assert response.status_code == 200, (path, response.status_code)
            return elapsed

    try:
        server, monitor = start('seed-server')
        password = os.urandom(24).hex()
        admin = request('POST', '/api/web/register', json={'username': 'global-load-admin', 'password': password}).json()
        subprocess.run([str(binary), 'web-admin', 'global-load-admin', '--config', str(config_path)], check=True, capture_output=True)
        hour = int(time.time()) // 3600 * 3600000
        audit_seed_rows = 100000 - int(sql('SELECT count(*) FROM web_admin_events;'))
        assert audit_seed_rows > 0
        usage_rollups = []
        for table, bucket in [('usage_daily', '(h.hour_unix_ms/86400000)*86400000'), ('usage_lifetime', '0::bigint')]:
            usage_rollups.append(f"""INSERT INTO {table}(source_kind,subject,hour_unix_ms,counters)
              SELECT source_kind,subject,bucket,jsonb_object_agg(key,value) FROM (
                SELECT h.source_kind,h.subject,{bucket} bucket,part.key,
                  CASE WHEN part.key='incomplete' THEN to_jsonb(bool_or(part.value='true'))
                    ELSE to_jsonb(sum(CASE WHEN part.key='incomplete' THEN 0 ELSE part.value::numeric END)) END value
                FROM usage_hourly h CROSS JOIN LATERAL jsonb_each_text(h.counters) part
                GROUP BY h.source_kind,h.subject,bucket,part.key
              ) totals GROUP BY source_kind,subject,bucket;""")
        seed = f"""
        BEGIN;
        INSERT INTO users(id,username,username_key,password_hash)
          SELECT md5('global-user-'||n)::uuid,'global-user-'||n,'global-user-'||n,
          (SELECT password_hash FROM users WHERE id='{admin['id']}') FROM generate_series(1,999) n;
        INSERT INTO tenants(id,kind,created_by_user_id)
          SELECT md5('global-tenant-'||n)::uuid,'personal',md5('global-user-'||n)::uuid FROM generate_series(1,999) n;
        INSERT INTO personal_tenants(tenant_id,user_id)
          SELECT md5('global-tenant-'||n)::uuid,md5('global-user-'||n)::uuid FROM generate_series(1,999) n;
        INSERT INTO tenants(id,kind)
          SELECT md5('global-device-tenant-'||n)::uuid,'unclaimed_device' FROM generate_series(1,10000) n;
        INSERT INTO devices(id,code,tenant_id,name)
          SELECT md5('global-device-'||n)::uuid,100000000+n,md5('global-device-tenant-'||n)::uuid,'global-device-'||n FROM generate_series(1,10000) n;
        INSERT INTO endpoints(endpoint_key,tenant_id,owner_kind,device_id)
          SELECT decode(md5('global-endpoint-a-'||n)||md5('global-endpoint-b-'||n),'hex'),md5('global-device-tenant-'||n)::uuid,'device',md5('global-device-'||n)::uuid FROM generate_series(1,10000) n;
        INSERT INTO device_accounts(device_id,user_id)
          SELECT md5('global-device-'||n)::uuid,md5('global-user-'||((n-1)%999+1))::uuid FROM generate_series(1,10000) n;
        INSERT INTO device_runtime(tenant_id,device_id,execution_context,environment_revision,agent_version,observed_at_unix_ms)
          SELECT tenant_id,id,jsonb_build_object('os_family',CASE WHEN code%4=0 THEN 'android' WHEN code%4=1 THEN 'windows' WHEN code%4=2 THEN 'linux' ELSE 'macos' END,'architecture',CASE WHEN code%2=0 THEN 'aarch64' ELSE 'x86_64' END),
          'global-load-fixture','1.2.48',{hour} FROM devices;
        INSERT INTO web_sessions(token_hash,user_id,channel)
          SELECT md5('global-token-a-'||n)||md5('global-token-b-'||n),md5('global-user-'||((n-1)%999+1))::uuid,'unknown' FROM generate_series(1,19999) n;
        INSERT INTO web_admin_events(actor_id,action,resource_id,result,format_version,actor_ref,actor_name,resource_kind,resource_key,resource_name,before_values,after_values)
          SELECT '{admin['id']}','account.bandwidth_updated',md5('global-user-'||((n-1)%999+1))::uuid,'succeeded',1,'{admin['id']}','global-load-admin','account',
          md5('global-user-'||((n-1)%999+1))::uuid::text,'global-user-'||((n-1)%999+1),'{{"relay_limit_mbps":5}}','{{"relay_limit_mbps":10}}' FROM generate_series(1,{audit_seed_rows}) n;
        INSERT INTO usage_hourly(source_kind,subject,hour_unix_ms,counters)
          SELECT 'client',id::text,{hour}-bucket*3600000,'{{"relay_upload_bytes":0,"relay_download_bytes":0,"connections":1,"connection_ms":1000,"uploaded_files":1,"downloaded_files":0,"uploaded_bytes":4096,"downloaded_bytes":0,"failed_transfers":0,"cancelled_transfers":0,"incomplete":false}}' FROM users CROSS JOIN generate_series(0,11) bucket;
        {''.join(usage_rollups)}
        COMMIT;
        ANALYZE;
        """
        (output / 'seed.sql').write_text(seed, encoding='utf-8')
        sql(seed)
        counts = json.loads(sql("SELECT json_build_object('users',(SELECT count(*) FROM users),'devices',(SELECT count(*) FROM devices),'sessions',(SELECT count(*) FROM web_sessions),'audit',(SELECT count(*) FROM web_admin_events),'usage_rows',(SELECT count(*) FROM usage_hourly),'tenants',(SELECT count(*) FROM tenants),'endpoints',(SELECT count(*) FROM endpoints));"))
        assert counts == {'users': 1000, 'devices': 10000, 'sessions': 20000, 'audit': 100000, 'usage_rows': 12000, 'tenants': 11000, 'endpoints': 10000}, counts
        report['dataset'] = counts
        assert sql("SELECT bool_and((counters->>'connections')::numeric=12 AND (counters->>'uploaded_bytes')::numeric=49152) FROM usage_lifetime;") == 't'
        report['usage_rollups_consistent'] = True
        session_subject = sql("SELECT id FROM users WHERE username='global-user-1';")
        container_info = json.loads(subprocess.check_output(['docker', 'inspect', environment_data['container']], text=True))[0]
        report['environment']['database_cpu_quota_nanocpus'] = container_info['HostConfig']['NanoCpus']
        report['environment']['database_memory_limit_bytes'] = container_info['HostConfig']['Memory']
        server.terminate()
        server.wait(timeout=15)
        assert container_info['Config']['Labels'].get('pixels.fixture') == 'admin-console-load', 'Restart only the separate owned load database'
        assert container_info['HostConfig']['Memory'] == 4 * 1024 ** 3
        assert container_info['HostConfig']['NanoCpus'] == 2_000_000_000
        subprocess.run(['docker', 'restart', environment_data['container']], check=True, capture_output=True)
        deadline = time.monotonic() + 45
        while subprocess.run(['docker', 'exec', environment_data['container'], 'pg_isready', '-U', 'postgres'], capture_output=True).returncode:
            assert time.monotonic() < deadline
            time.sleep(.2)
        rebound = json.loads(subprocess.check_output(['docker', 'inspect', environment_data['container']], text=True))[0]
        binding = rebound['NetworkSettings']['Ports']['5432/tcp']
        assert len(binding) == 1 and binding[0]['HostIp'] == '127.0.0.1'
        parsed = urlsplit(config['database']['url'])
        config['database']['url'] = f"postgres://postgres@127.0.0.1:{binding[0]['HostPort']}{parsed.path}"
        write_config(config_path, config)
        report['environment']['database_shared_buffers_restarted'] = True
        server, monitor = start('measurement-server')
        report['environment']['host_cpu_percent_at_start'] = psutil.cpu_percent(interval=.25)
        report['environment']['host_memory_percent_at_start'] = psutil.virtual_memory().percent
        endpoints = ['/api/web/accounts?page_size=100', '/api/web/devices?scope=all&page_size=100',
            '/api/web/devices/diagnostics?scope=all', '/api/web/audit?page_size=100',
            '/api/web/overview?scope=all', f"/api/web/accounts/{session_subject}/sessions?page_size=100",
            '/api/web/usage?scope=all&interval=lifetime', '/api/web/usage/ranking?scope=all&interval=lifetime&page_size=100']
        measurements = []
        with ThreadPoolExecutor(max_workers=20) as executor:
            for wave, repeats in [('first', 20), ('warm', 60)]:
                for path in endpoints:
                    values = list(executor.map(timed_get, [path] * repeats))
                    measurements.append({'wave': wave, 'path': path, 'requests': len(values),
                        'p50_ms': percentile(values, .5), 'p95_ms': percentile(values, .95),
                        'max_ms': round(max(values), 2), 'passed_500ms': percentile(values, .95) <= 500})
                    (output / 'read-progress.json').write_text(json.dumps(measurements, indent=2))
        report['reads'] = measurements
        before_rss = monitor.memory_info().rss
        barrier = threading.Barrier(3)
        exports = []

        def export(path, label):
            with requests.Session() as reader:
                reader.cookies.update(session.cookies)
                barrier.wait(timeout=10)
                started = time.monotonic()
                response = reader.get(origin + path, verify=config['tls']['cert'], timeout=30, stream=True)
                assert response.status_code == 200, (path, response.status_code)
                destination = output / f'{label}.csv'
                with destination.open('wb') as sink:
                    chunks = 0
                    for chunk in response.iter_content(16384):
                        sink.write(chunk)
                        chunks += 1
                        time.sleep(.005)
                rows = list(csv.DictReader(io.StringIO(destination.read_text(encoding='utf-8-sig'))))
                data = [row for row in rows if row.get('record_type') == 'data']
                footer = rows[-1]
                assert len(data) == 10000, (label, len(data))
                assert footer['record_type'] == 'export_status' and footer['truncated'] == 'true', footer
                return {'kind': label, 'data_rows': len(data), 'truncated': True,
                    'reader_delay_per_chunk_seconds': .005, 'read_chunks': chunks,
                    'intentional_reader_delay_seconds': round(chunks * .005, 3),
                    'elapsed_seconds': round(time.monotonic() - started, 3),
                    'bytes': destination.stat().st_size, 'sha256': hashlib.sha256(destination.read_bytes()).hexdigest()}

        export_from = hour - 11 * 3600000
        with ThreadPoolExecutor(max_workers=2) as executor:
            futures = [executor.submit(export, '/api/web/audit/export', 'audit'),
                executor.submit(export, f'/api/web/usage/export?scope=all&interval=hour&from={export_from}&until={hour+3600000}', 'usage')]
            barrier.wait(timeout=10)
            health = []
            peak_rss = before_rss
            while not all(future.done() for future in futures):
                started = time.perf_counter()
                request('GET', '/health', 204)
                health.append((time.perf_counter() - started) * 1000)
                peak_rss = max(peak_rss, monitor.memory_info().rss)
                time.sleep(.025)
            exports = [future.result() for future in futures]
        report['parallel_exports'] = {'exports': exports, 'health_probes': len(health),
            'health_p95_ms': percentile(health, .95), 'peak_rss_growth_bytes': peak_rss - before_rss,
            'rss_growth_limit_bytes': 128 * 1024 * 1024}
        assert peak_rss - before_rss <= 128 * 1024 * 1024
        report['reads_meet_500ms'] = all(item['passed_500ms'] for item in measurements)
        report['status'] = 'pass' if report['reads_meet_500ms'] else 'performance_threshold_failed'
    except Exception as error:
        report.update(status='fail', error_class=type(error).__name__, error=str(error)[:500])
        raise
    finally:
        for process in processes:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=15)
        for log in logs:
            log.close()
        for limit in memory_limits:
            limit.close()
        report['owned_processes_stopped'] = all(process.poll() is not None for process in processes)
        (output / 'result.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
    print(json.dumps(report))
    if report['status'] != 'pass':
        raise SystemExit(1)


if __name__ == '__main__':
    main()
