#!/usr/bin/env python3
"""Sequential Relay upgrades with durable operation IDs and real forwarding probes.

Python 3.11+. SSH targets additionally require Paramiko. Default is read-only;
--apply stages artifacts and resumes the journal. Never manages Executor services.
"""
import argparse
import base64
import contextlib
import hashlib
import json
import os
from pathlib import Path, PurePosixPath, PureWindowsPath
import re
import shlex
import ssl
import stat
import subprocess
import tarfile
import tempfile
import time
import tomllib
import urllib.error
import urllib.parse
import urllib.request
import uuid
import zipfile

MAX_OUTPUT = 2 * 1024 * 1024
MAX_ARCHIVE_BYTES = 2 * 1024 * 1024 * 1024


def digest(path):
    with open(path, 'rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def private_write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + '.tmp')
    with open(temporary, 'w', encoding='utf-8', opener=lambda p, f: os.open(p, f, 0o600)) as out:
        out.write(value)
        out.flush()
        os.fsync(out.fileno())
    os.replace(temporary, path)


def archive_members(archive):
    """Validate every entry, including entries that will not be installed."""
    seen = set()
    size = 0
    entries = []
    is_zip = zipfile.is_zipfile(archive)
    source = zipfile.ZipFile(archive) if is_zip else tarfile.open(archive, 'r:gz')
    try:
        for entry in source.infolist() if is_zip else source:
            # ZipInfo.filename normalizes backslashes on Windows; inspect the raw
            # archive name so the exact same package is accepted on every OS.
            name = entry.orig_filename if is_zip else entry.name
            path = PurePosixPath(name)
            mode = entry.external_attr >> 16 if is_zip else entry.mode
            directory = entry.is_dir() if is_zip else entry.isdir()
            if (not name or '\\' in name or ':' in name or path.is_absolute()
                    or '..' in path.parts or len(path.parts) > 12
                    or any(ord(c) < 32 for c in name)
                    or any(part.endswith(('.', ' ')) for part in path.parts)
                    or stat.S_ISLNK(mode) or (not is_zip and not (entry.isfile() or directory))):
                raise ValueError('Unsafe archive entry')
            canonical = str(path).casefold()
            if canonical in seen or len(seen) >= 10000:
                raise ValueError('Duplicate archive entry or too many entries')
            seen.add(canonical)
            size += entry.file_size if is_zip else entry.size
            if size > MAX_ARCHIVE_BYTES:
                raise ValueError('Uncompressed archive exceeds 2 GiB')
            entries.append((str(path), entry, directory))
        yield source, entries, is_zip
    finally:
        source.close()


def unpack_relay(archive, platform, destination):
    suffix = '.exe' if platform == 'windows' else ''
    required = {'pab-relay-server' + suffix,
                'windows/install.ps1' if platform == 'windows' else 'linux/install.py'}
    for source, entries, is_zip in archive_members(archive):
        found = set()
        for name, entry, directory in entries:
            if name not in required or directory:
                continue
            path = destination / name
            path.parent.mkdir(parents=True, exist_ok=True)
            stream = source.open(entry) if is_zip else source.extractfile(entry)
            with stream, path.open('wb') as out:
                while data := stream.read(1024 * 1024):
                    out.write(data)
            if name.endswith(suffix) and platform == 'linux':
                path.chmod(0o755)
            found.add(name)
        if found != required:
            raise ValueError('Package is missing Relay binary or native installer')
    return digest(destination / ('pab-relay-server' + suffix))


class Api:
    def __init__(self, config, base):
        parsed = urllib.parse.urlsplit(config['url'])
        if (parsed.scheme != 'https' or not parsed.hostname or parsed.username or parsed.password
                or parsed.query or parsed.fragment or parsed.path not in ['', '/']):
            raise ValueError('control.url must be an HTTPS origin')
        self.origin = config['url'].rstrip('/')
        self.config, self.base, self.token = config, base, None
        context = ssl.create_default_context(cafile=str(base / config['ca_cert']) if config.get('ca_cert') else None)
        class NoRedirect(urllib.request.HTTPRedirectHandler):
            def redirect_request(self, *args, **kwargs):
                return None
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}),
            urllib.request.HTTPSHandler(context=context), NoRedirect())
        self.owned = not config.get('token_file')

    def request(self, method, path, body=None):
        headers = {'Content-Type': 'application/json'}
        if self.token:
            headers['Authorization'] = 'Bearer ' + self.token
        request = urllib.request.Request(self.origin + path,
            data=json.dumps(body).encode() if body is not None else None, headers=headers, method=method)
        # GET and state changes with a persisted operation ID are safe to retry.
        attempts = 3 if method == 'GET' or (body and 'operation_id' in body) else 1
        for attempt in range(attempts):
            try:
                with self.opener.open(request, timeout=10) as response:
                    raw = response.read(MAX_OUTPUT + 1)
                    if len(raw) > MAX_OUTPUT:
                        raise ValueError('Management response is too large')
                    return json.loads(raw) if raw else None
            except urllib.error.HTTPError as error:
                raise RuntimeError(f'Management request failed: HTTP {error.code} ({path})') from None
            except (OSError, TimeoutError):
                if attempt + 1 == attempts:
                    raise RuntimeError('Management request could not be confirmed; resume with the same journal') from None
                time.sleep(1)

    def login(self):
        if self.owned:
            self.token = self.request('POST', '/api/account/session', {
                'username': self.config['username'], 'password': self.config['password']})['access_token']
        else:
            path = self.base / self.config['token_file']
            if path.stat().st_size > 16384:
                raise ValueError('Token file is too large')
            self.token = path.read_text().strip()

    def close(self):
        if self.owned and self.token:
            with contextlib.suppress(Exception):
                self.request('POST', '/api/account/logout')

    def nodes(self):
        # Cluster is bounded to 64 nodes; request a single complete snapshot.
        return {row['id']: row for row in self.request('GET', '/api/admin/relays?page_size=100')['items']}


class Target:
    def __init__(self, node, base):
        self.node, self.base, self.ssh = node, base, None
        self.windows = node['platform'] == 'windows'
        self.path = PureWindowsPath if self.windows else PurePosixPath
        self.root = self.path(node['install_directory'])
        if not self.root.is_absolute() or self.root.parent == self.root:
            raise ValueError('Each target needs a dedicated absolute installation directory')
        if not re.fullmatch(r'[A-Za-z0-9_.-]{1,80}', node['service_name']) or node['service_name'].lower() in {
                'pixelsagentbridgeexecutor', 'pixels-agent-bridge-executor', 'pixels-agent-bridge-executor.service'}:
            raise ValueError('Invalid or reserved service name')
        for key in ['install_directory', 'config_file', 'service_name']:
            if any(ord(c) < 32 or c == '"' for c in node[key]):
                raise ValueError('Invalid service path')
        if node.get('transport', 'ssh') == 'local':
            if self.windows != (os.name == 'nt'):
                raise ValueError('Local transport platform differs from this host')
        elif node.get('transport', 'ssh') != 'ssh':
            raise ValueError('transport must be local or ssh')

    def connect(self):
        if self.node.get('transport', 'ssh') == 'local':
            return
        if self.ssh and self.ssh.get_transport() and self.ssh.get_transport().is_active():
            return
        import paramiko
        self.ssh = paramiko.SSHClient()
        self.ssh.load_system_host_keys()
        if self.node.get('known_hosts'):
            self.ssh.load_host_keys(str(self.base / self.node['known_hosts']))
        self.ssh.set_missing_host_key_policy(paramiko.RejectPolicy())
        self.ssh.connect(self.node['ssh_host'], port=int(self.node.get('ssh_port', 22)),
            username=self.node['ssh_user'],
            key_filename=str(self.base / self.node['identity_file']) if self.node.get('identity_file') else None,
            password=self.node.get('ssh_password'), timeout=10, auth_timeout=10, banner_timeout=10)

    def close(self):
        if self.ssh:
            self.ssh.close()

    def run(self, arguments, timeout=90):
        self.connect()
        if self.windows:
            quote = lambda v: "'" + str(v).replace("'", "''") + "'"
            script = "$ErrorActionPreference='Stop'; & " + ' '.join(map(quote, arguments))
            script += '; if ($LASTEXITCODE) {exit $LASTEXITCODE}'
            arguments = ['powershell.exe', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass',
                '-EncodedCommand', base64.b64encode(script.encode('utf-16le')).decode()]
        if not self.ssh:
            # Installer output is bounded on disk, not accumulated indefinitely in RAM.
            with tempfile.TemporaryFile() as output:
                result = subprocess.run(list(map(str, arguments)), stdout=output, stderr=output,
                    timeout=timeout, creationflags=subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0)
                if output.tell() > MAX_OUTPUT:
                    raise RuntimeError('Target output exceeded its limit')
                output.seek(0)
                text = output.read().decode('utf-8-sig', errors='replace')
        else:
            command = ' '.join(map(shlex.quote, map(str, arguments))) if not self.windows else subprocess.list2cmdline(arguments)
            _, out, _ = self.ssh.exec_command(command, timeout=timeout)
            channel = out.channel
            channel.set_combine_stderr(True)
            deadline, chunks, size = time.monotonic() + timeout, [], 0
            try:
                while True:
                    while channel.recv_ready():
                        chunk = channel.recv(65536)
                        chunks.append(chunk)
                        size += len(chunk)
                        if size > MAX_OUTPUT:
                            raise RuntimeError('Target output exceeded its limit')
                    if channel.exit_status_ready() and not channel.recv_ready():
                        break
                    if time.monotonic() >= deadline:
                        raise TimeoutError('Deployment outcome unknown; query persisted target status before resuming')
                    time.sleep(.05)
                result = type('Result', (), {'returncode': channel.recv_exit_status()})()
                text = b''.join(chunks).decode('utf-8-sig', errors='replace')
            finally:
                channel.close()
        if result.returncode:
            # Do not echo remote logs, which may include private configuration details.
            raise RuntimeError(f'Target command exited with code {result.returncode}; inspect service/preflight logs')
        return text.strip()

    def copy_source(self, source, run_id):
        stage = self.root / 'incoming' / run_id
        self.connect()
        if self.ssh:
            with self.ssh.open_sftp() as sftp:
                def mkdir(path):
                    try:
                        existing = sftp.lstat(str(path).replace('\\', '/'))
                        if not stat.S_ISDIR(existing.st_mode):
                            raise ValueError('Staging path is not an ordinary directory')
                    except FileNotFoundError:
                        if path.parent != path:
                            mkdir(path.parent)
                        sftp.mkdir(str(path).replace('\\', '/'))
                mkdir(stage)
                for local in source.rglob('*'):
                    remote = stage / local.relative_to(source).as_posix()
                    if local.is_dir():
                        mkdir(remote)
                    else:
                        mkdir(remote.parent)
                        # Reuse an already installed immutable Linux executable
                        # when resuming/redeploying the same build. The normal
                        # post-copy SHA check still runs before maintenance.
                        if not self.windows and local.name=='pab-relay-server':
                            cached=self.root/'releases'/digest(local)/local.name
                            try:
                                old=sftp.lstat(str(cached))
                                if stat.S_ISREG(old.st_mode) and old.st_size==local.stat().st_size:
                                    self.run(['cp','--',str(cached),str(remote)])
                                    continue
                            except FileNotFoundError:
                                pass
                        sftp.put(str(local), str(remote).replace('\\', '/'))
                        if not self.windows:
                            sftp.chmod(str(remote), 0o755 if local.name == 'pab-relay-server' else 0o600)
        else:
            import shutil
            shutil.copytree(source, Path(str(stage)), dirs_exist_ok=True)
        return stage

    def stage(self, source, run_id):
        stage=self.copy_source(source,run_id)
        binary = stage / ('pab-relay-server.exe' if self.windows else 'pab-relay-server')
        if self.windows:
            # Native .NET SHA implementation avoids module autoload under service accounts.
            text = "$s=[IO.File]::OpenRead('" + str(binary).replace("'", "''") + "');try{[BitConverter]::ToString([Security.Cryptography.SHA256]::Create().ComputeHash($s)).Replace('-','').ToLowerInvariant()}finally{$s.Dispose()}"
            sha = self.run(['powershell.exe', '-NoProfile', '-NonInteractive', '-EncodedCommand', base64.b64encode(text.encode('utf-16le')).decode()])
        else:
            sha = self.run(['sha256sum', str(binary)]).split()[0]
        if sha != digest(source / binary.name):
            raise ValueError('Staged binary checksum mismatch')
        self.run([str(binary), 'check', '--config', self.node['config_file']], timeout=30)
        return str(stage)

    def install(self, stage, action, expected_sha256=None):
        if self.windows:
            args = ['powershell.exe', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File',
                str(self.path(stage) / 'windows/install.ps1'), '-Component', 'Relay', '-Action', action.title(),
                '-InstallDirectory', str(self.root), '-ServiceName', self.node['service_name']]
            if action == 'update':
                args += ['-SourceDirectory', stage, '-ConfigFile', self.node['config_file']]
            if expected_sha256:
                args += ['-ExpectedSha256', expected_sha256]
        else:
            args = ['python3', str(self.path(stage) / 'linux/install.py'), '--component', 'relay', '--action', action,
                '--root', str(self.root), '--service-name', self.node['service_name']]
            if action == 'update':
                args += ['--source', stage, '--config', self.node['config_file']]
            if expected_sha256:
                args += ['--expected-sha256', expected_sha256]
            if self.node.get('sudo'):
                args = ['sudo', '-n', *args]
        text = self.run(args, timeout=90 if action != 'status' else 15)
        return json.loads(text) if action == 'status' else None


class DockerTarget(Target):
    def arguments(self, stage, action):
        args=['python3',str(self.path(stage)/'docker/install.py'),'--action',action,'--root',stage,
            '--compose-file',self.node['config_file'],'--project-directory',self.node['project_directory'],
            '--project-name',self.node['project_name'],'--service-name',self.node['service_name']]
        return ['sudo','-n',*args] if self.node.get('sudo') else args

    def stage(self, source, run_id):
        stage=str(self.copy_source(source,run_id))
        result=json.loads(self.run(self.arguments(stage,'stage')+['--image',self.node['image']],timeout=330))
        self.prepared_sha256=result['sha256']
        return stage

    def install(self, stage, action, expected_sha256=None):
        args=self.arguments(stage,action)
        if expected_sha256:args+=['--expected-sha256',expected_sha256]
        text=self.run(args,timeout=100 if action!='status' else 15)
        return json.loads(text) if action=='status' else None


def make_target(node,base):
    return DockerTarget(node,base) if node.get('deployment')=='docker' else Target(node,base)


def prepare(node,base,source,entry):
    if node.get('deployment')!='docker':return unpack_relay(base/node['artifact'],node['platform'],source)
    import shutil
    helper=Path(__file__).resolve().parent/'docker/install.py'
    if not helper.exists():helper=Path(__file__).resolve().parents[1]/'packaging/server/docker/install.py'
    (source/'docker').mkdir()
    shutil.copy2(helper,source/'docker/install.py')
    return entry.get('target_sha256')


def bounded_wait(predicate, seconds, label):
    deadline = time.monotonic() + seconds
    while True:
        value = predicate()
        if value:
            return value
        if time.monotonic() >= deadline:
            raise TimeoutError(label)
        time.sleep(.5)


def wait_activation(api, operation_id, timeout):
    def confirmed():
        operation = api.request('GET', '/api/admin/relay-operations/' + operation_id)
        if operation.get('phase') == 'failed' or operation.get('error_code') == 'superseded':
            raise RuntimeError('Relay activation was not confirmed: ' + str(operation.get('error_code')))
        return operation.get('phase') == 'completed'
    bounded_wait(confirmed, timeout, 'Relay activation acknowledgement expired')


def run_rollout(config, base, api, journal, save):
    health_timeout = int(config.get('rollout', {}).get('health_timeout_seconds', 60))
    observe = int(config.get('rollout', {}).get('observation_seconds', 5))
    allow_outage = config.get('rollout', {}).get('allow_outage', False)
    if not 10 <= health_timeout <= 300 or not 1 <= observe <= 60:
        raise ValueError('Invalid bounded health/observation interval')
    with tempfile.TemporaryDirectory(prefix='pixels-relay-rollout-') as work:
        work = Path(work)
        for node in config['nodes']:
            node_id = node['node_id']
            entry = journal['nodes'].setdefault(node_id, {'phase': 'new', 'stage_id': str(uuid.uuid4())})
            if entry['phase'] == 'done':
                continue
            if entry['phase'] == 'rolled_back':
                raise RuntimeError('A node was rolled back; investigate before starting a new rollout journal')
            target = make_target(node, base)
            try:
                source = work / node_id
                source.mkdir()
                expected = prepare(node,base,source,entry)
                if entry['phase'] == 'new':
                    entry['stage'] = target.stage(source, entry['stage_id'])
                    expected=getattr(target,'prepared_sha256',expected)
                    before = target.install(entry['stage'], 'status')
                    if before['installer_active'] or not before.get('running_sha256'):
                        raise RuntimeError('Target service is busy or not running; maintenance was not requested')
                    entry.update(phase='staged', previous_sha256=before['running_sha256'], target_sha256=expected)
                    save()
                if entry['phase'] == 'staged':
                    row = api.nodes().get(node_id)
                    if not row or row['admin_state'] != 'active' or row['health'] != 'healthy':
                        raise RuntimeError('Target must be healthy and active before maintenance')
                    entry.update(phase='maintenance_requested', operation_id=str(uuid.uuid4()))
                    save()  # Persist the operation ID before any mutation can leave this host.
                if entry['phase'] == 'maintenance_requested':
                    api.request('PATCH', f'/api/admin/relays/{node_id}/state', {'state': 'maintenance',
                        'operation_id': entry['operation_id'], 'allow_outage': allow_outage})
                    def ready():
                        operation = api.request('GET', '/api/admin/relay-operations/' + entry['operation_id'])
                        if operation['phase'] not in ['waiting', 'ready']:
                            raise RuntimeError('Maintenance operation is no longer available; node remains isolated')
                        return operation['phase'] == 'ready'
                    bounded_wait(ready, 310, 'Maintenance did not become ready within the configured bound')
                    entry['phase'] = 'ready'
                    save()
                try:
                    if entry['phase'] == 'ready':
                        entry['phase'] = 'deploying'
                        save()
                        target.install(entry['stage'], 'update')
                    if entry['phase'] == 'deploying':
                        # An interrupted/unknown command is never repeated blindly.
                        status = bounded_wait(lambda: (s if not s['installer_active'] else None)
                            if (s := target.install(entry['stage'], 'status')) else None, 100, 'Installer still active')
                        if status.get('running_sha256') != expected:
                            raise RuntimeError('Target did not start the requested binary')
                        entry['phase'] = 'verifying'
                        save()
                    if entry['phase'] == 'verifying':
                        bounded_wait(lambda: (row if row and row.get('health') == 'healthy' else None)
                            if (row := api.nodes().get(node_id)) else None, health_timeout, 'New Relay did not become healthy')
                        entry['phase'] = 'activating'
                        entry['activate_id'] = str(uuid.uuid4())
                        save()
                    if entry['phase'] == 'activating':
                        api.request('PATCH', f'/api/admin/relays/{node_id}/state', {'state': 'active', 'operation_id': entry['activate_id']})
                        wait_activation(api, entry['activate_id'], health_timeout)
                        entry['phase'] = 'probing'
                        save()
                    if entry['phase'] == 'probing':
                        # Maintenance intentionally rejects new Relay connections.
                        probe(config, base, api, node_id)
                        entry['phase'] = 'observing'
                        save()
                    if entry['phase'] == 'observing':
                        deadline = time.monotonic() + observe
                        while time.monotonic() < deadline:
                            row = api.nodes().get(node_id)
                            if not row or row['health'] != 'healthy' or row['admin_state'] != 'active':
                                raise RuntimeError('Relay failed during observation')
                            time.sleep(.5)
                        entry['phase'] = 'done'
                        save()
                        print(json.dumps({'node_id': node_id, 'phase': 'done'}), flush=True)
                except Exception:
                    # A live installer might still finish after SSH disconnects. Do not race it.
                    entry['error_phase'] = entry['phase']
                    entry['phase'] = 'restoring'
                    save()
                    raise
                if entry['phase'] == 'restoring':
                    raise RuntimeError('Rollback recovery required')
            finally:
                target.close()


def recover(config, base, api, journal, save):
    """Finish rollback only after proving which executable is actually running."""
    for node in config['nodes']:
        entry = journal['nodes'].get(node['node_id'], {})
        if entry.get('phase') != 'restoring':
            continue
        target = make_target(node, base)
        try:
            # Isolate again if a failure happened after activation.
            row = api.nodes().get(node['node_id'])
            if not row:
                raise RuntimeError('Node was removed during rollout; manual recovery required')
            if row['admin_state'] == 'maintenance':
                if row.get('operation_id') not in [entry['operation_id'], entry.get('restore_maintenance_id')]:
                    raise RuntimeError('Another operator controls this node; refusing to override maintenance')
                operation = row['operation_id']
            else:
                operation = entry.setdefault('restore_maintenance_id', str(uuid.uuid4()))
            save()
            if row['admin_state'] != 'maintenance':
                api.request('PATCH', f"/api/admin/relays/{node['node_id']}/state", {'state': 'maintenance',
                    'operation_id': operation, 'allow_outage': config.get('rollout', {}).get('allow_outage', False)})
            bounded_wait(lambda: api.request('GET', '/api/admin/relay-operations/' + operation)['phase'] == 'ready',
                310, 'Rollback maintenance wait expired')
            status = bounded_wait(lambda: (s if not s['installer_active'] else None)
                if (s := target.install(entry['stage'], 'status')) else None, 100, 'Installer remains active')
            if status.get('running_sha256') != entry['previous_sha256']:
                if status.get('running_sha256') not in [None, entry['target_sha256']]:
                    raise RuntimeError('Running binary is unknown; manual recovery required, node remains in maintenance')
                # Persist before invoking rollback; status is checked on every resume.
                entry['rollback_requested'] = True
                save()
                target.install(entry['stage'], 'restore', expected_sha256=entry['previous_sha256'])
            status = target.install(entry['stage'], 'status')
            if status.get('running_sha256') != entry['previous_sha256']:
                raise RuntimeError('Rollback binary could not be verified; node remains in maintenance')
            bounded_wait(lambda: (api.nodes().get(node['node_id']) or {}).get('health') == 'healthy',
                int(config.get('rollout', {}).get('health_timeout_seconds', 60)), 'Rolled back Relay did not become healthy')
            # A maintained node rejects the probe until activation is confirmed.
            activate = entry.setdefault('restore_activate_id', str(uuid.uuid4()))
            save()
            api.request('PATCH', f"/api/admin/relays/{node['node_id']}/state", {'state': 'active', 'operation_id': activate})
            wait_activation(api, activate, int(config.get('rollout', {}).get('health_timeout_seconds', 60)))
            probe(config, base, api, node['node_id'])
            entry['phase'] = 'rolled_back'
            save()
            raise RuntimeError('Previous version restored and verified; rollout stopped before the next node')
        finally:
            target.close()


def probe(config, base, api, node_id):
    with tempfile.TemporaryDirectory(prefix='pixels-relay-probe-') as work:
        work = Path(work)
        private_write(work / 'token', api.token)
        values = {'control_url': api.origin, 'node_id': node_id}
        for key in ['ca_cert', 'relay_ca_cert']:
            if config['control'].get(key):
                values[key] = str((base / config['control'][key]).resolve())
        private_write(work / 'probe.toml', '\n'.join(f'{k} = {json.dumps(v)}' for k, v in values.items())
            + '\n[auth]\naccess_token_file = ' + json.dumps(str(work / 'token')) + '\n')
        result = subprocess.run([str((base / config['control']['probe_binary']).resolve()), '--config', str(work / 'probe.toml')],
            timeout=25, capture_output=True, creationflags=subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0)
        if result.returncode or len(result.stdout) > MAX_OUTPUT or not json.loads(result.stdout).get('ok'):
            raise RuntimeError('Real bidirectional forwarding probe failed')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', type=Path, required=True)
    parser.add_argument('--apply', action='store_true', help='Stage artifacts and apply/resume this exact rollout')
    args = parser.parse_args()
    path = args.config.resolve()
    if path.stat().st_size > 1024 * 1024:
        raise ValueError('Rollout configuration exceeds 1 MiB')
    config, base = tomllib.loads(path.read_text(encoding='utf-8-sig')), path.parent
    nodes = config.get('nodes', [])
    if not 1 <= len(nodes) <= 64 or len({n['node_id'] for n in nodes}) != len(nodes):
        raise ValueError('Provide 1..64 distinct nodes')
    for node in nodes:
        if not re.fullmatch(r'[A-Za-z0-9_.-]{1,64}', node['node_id']) or node['platform'] not in ['windows', 'linux']:
            raise ValueError('Invalid node identity or platform')
        if node.get('deployment')=='docker':
            if node['platform']!='linux' or not re.fullmatch(r'[^\s@]+@sha256:[a-f0-9]{64}',node['image']):
                raise ValueError('Docker requires Linux and an image pinned by SHA-256 digest')
            if not PurePosixPath(node['project_directory']).is_absolute():raise ValueError('Compose project directory must be absolute')
        else:
            if not re.fullmatch(r'[a-f0-9]{64}', node['sha256']) or digest(base / node['artifact']) != node['sha256']:
                raise ValueError('Artifact checksum mismatch')
        make_target(node, base)
    identity = hashlib.sha256(json.dumps({'url': config['control']['url'], 'allow_outage':config.get('rollout',{}).get('allow_outage',False), 'nodes': [
        {key: node.get(key) for key in ['node_id', 'sha256', 'platform', 'transport', 'install_directory', 'service_name',
            'config_file', 'ssh_host', 'ssh_port', 'ssh_user','sudo','deployment','image','project_name','project_directory']} for node in nodes]}, sort_keys=True).encode()).hexdigest()
    state_path = (base / config['rollout']['state_file']).resolve()
    api = Api(config['control'], base)
    api.login()
    try:
        inventory = api.nodes()
        for node in nodes:
            row = inventory.get(node['node_id'])
            if not row:
                raise ValueError('Rollout node is not in the central catalog')
        if not args.apply:
            print(json.dumps({'read_only': True, 'nodes': [{'node_id': n['node_id'], 'health': inventory[n['node_id']]['health'],
                'state': inventory[n['node_id']]['admin_state']} for n in nodes]}))
            return
        # File lock prevents two local operators from racing the same journal.
        state_path.parent.mkdir(parents=True, exist_ok=True)
        with open(str(state_path) + '.lock', 'a+b') as lock:
            if os.name == 'nt':
                import msvcrt
                lock.write(b'0');lock.flush();lock.seek(0)
                msvcrt.locking(lock.fileno(), msvcrt.LK_NBLCK, 1)
            else:
                import fcntl
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            journal = json.loads(state_path.read_text()) if state_path.exists() else {'identity': identity, 'nodes': {}}
            if journal['identity'] != identity:
                raise ValueError('Journal belongs to another rollout; use a separate state_file')
            save = lambda: private_write(state_path, json.dumps(journal, indent=2) + '\n')
            save()
            recover(config, base, api, journal, save)
            try:
                run_rollout(config, base, api, journal, save)
            except Exception:
                # Try recovery now; if the network is unavailable the journal retains the exact phase.
                recover(config, base, api, journal, save)
                raise
    finally:
        api.close()


if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        # Exception types and our fixed messages are safe; third-party SSH errors may contain identities.
        safe = str(error) if type(error) in [ValueError, RuntimeError, TimeoutError] else type(error).__name__
        raise SystemExit(safe) from None
