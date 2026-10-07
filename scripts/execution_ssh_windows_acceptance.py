"""Windows user-worker Git/SSH acceptance using Git for Windows and isolated sshd.

Run from the interactive user's elevated shell, passing its WTS session number.
Requires Docker's pab-linux-execution-ssh:bookworm image and prebuilt test/worker
binaries. Changes only generated home directories and a generated SYSTEM task.
No Windows SSH service, real repository, login agent or ~/.ssh is modified.
"""
import argparse
import json
import os
from pathlib import Path
import shlex
import shutil
import socket
import subprocess
import tempfile
import time
import uuid


def ps_literal(value):
    return "'" + str(value).replace("'", "''") + "'"


def msys(path):
    path = Path(path).resolve()
    return "/" + path.drive[0].lower() + path.as_posix()[2:]


def run(args):
    if os.name != "nt":
        raise RuntimeError("Windows fixture only")
    home = Path.home().resolve()
    binary, worker = Path(args.test_binary).resolve(strict=True), Path(args.worker).resolve(strict=True)
    git = Path(args.git_bin).resolve(strict=True)
    for exe in ("ssh", "ssh-agent", "ssh-add", "ssh-keygen"):
        if not (git / (exe + ".exe")).is_file():
            raise RuntimeError("Git for Windows OpenSSH tools required")
    root = Path(tempfile.mkdtemp(prefix="pab-ssh-acceptance-", dir=home)).resolve()
    workspace = Path(tempfile.mkdtemp(prefix="pab-git-acceptance-", dir=home)).resolve()
    suffix = uuid.uuid4().hex[:12]
    container, task = "pab-windows-ssh-" + suffix, "PAB-GitSsh-" + suffix
    agent = None
    container_started = task_created = False
    hidden = {"creationflags": subprocess.CREATE_NO_WINDOW}

    def command(argv, **kw):
        return subprocess.run(argv, check=True, capture_output=True, text=True,
                              timeout=kw.pop("timeout", 30), **hidden, **kw)

    def powershell(source):
        return command(["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", source])

    try:
        key, sock = root / "identity", root / "agent.sock"
        command([str(git / "ssh-keygen.exe"), "-q", "-t", "ed25519", "-N", "", "-f", str(key)])
        public = Path(str(key) + ".pub")
        agent = subprocess.Popen([str(git / "ssh-agent.exe"), "-D", "-a", msys(sock)],
                                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, **hidden)
        until = time.monotonic() + 10
        while not sock.exists():
            if agent.poll() is not None or time.monotonic() >= until:
                raise RuntimeError("isolated agent did not become ready")
            time.sleep(0.05)
        command([str(git / "ssh-add.exe"), msys(key)], env=dict(os.environ, SSH_AUTH_SOCK=msys(sock)))
        key.unlink()  # Real authentication must use the agent, not a disk private key.
        command(["docker", "run", "-d", "--name", container, "--publish", "127.0.0.1::22",
                 "--mount", f"type=bind,source={workspace},target=/fixture",
                 args.image, "/bin/sleep", "infinity"])
        container_started = True
        port = int(command(["docker", "port", container, "22/tcp"]).stdout.strip().rsplit(":", 1)[1])
        setup = """import pathlib,subprocess
subprocess.run(['useradd','-m','-s','/bin/sh','pabssh'],check=True)
subprocess.run(['usermod','-p','x','pabssh'],check=True)
pathlib.Path('/run/sshd').mkdir(exist_ok=True)
for name in ('host','wrong'):
 subprocess.run(['ssh-keygen','-q','-t','ed25519','-N','','-f','/etc/ssh/pab_'+name],check=True)
pathlib.Path('/etc/ssh/pab_authorized_keys').write_text(PUBLIC)
pathlib.Path('/etc/ssh/pab_authorized_keys').chmod(0o644)
pathlib.Path('/etc/ssh/pab_sshd_config').write_text('''Port 22
ListenAddress 0.0.0.0
HostKey /etc/ssh/pab_host
PidFile /run/pab_sshd.pid
AuthorizedKeysFile /etc/ssh/pab_authorized_keys
AllowUsers pabssh
PubkeyAuthentication yes
PasswordAuthentication no
KbdInteractiveAuthentication no
UsePAM no
PermitRootLogin no
StrictModes yes
AllowAgentForwarding no
AllowTcpForwarding no
X11Forwarding no
PermitTTY no
LogLevel VERBOSE
''')
# Docker Desktop presents the dedicated bind as root-owned. Grant access only
# to the isolated fixture; do not bind a home directory or alter the host ACL.
subprocess.run(['su','-s','/bin/sh','pabssh','-c','git config --global safe.directory /fixture/remote.git'],check=True)
subprocess.run(['/usr/sbin/sshd','-t','-f','/etc/ssh/pab_sshd_config'],check=True)
subprocess.run(['/usr/sbin/sshd','-f','/etc/ssh/pab_sshd_config','-E','/var/log/pab_sshd.log'],check=True)
""".replace("PUBLIC", repr(public.read_text(encoding="utf-8")))
        command(["docker", "exec", "-i", container, "python3", "-"], input=setup)
        known, wrong = root / "known_hosts", root / "wrong_hosts"
        for destination, name in ((known, "host"), (wrong, "wrong")):
            host = command(["docker", "exec", container, "cat", "/etc/ssh/pab_" + name + ".pub"]).stdout
            destination.write_text(f"[127.0.0.1]:{port} " + host, encoding="utf-8")

        def ssh_args(hosts, socket_path):
            return [str(git / "ssh.exe").replace("\\", "/"), "-F", "/dev/null", "-p", str(port),
                    "-i", msys(public), "-o", "IdentitiesOnly=yes", "-o", "BatchMode=yes",
                    "-o", "ConnectTimeout=5", "-o", "StrictHostKeyChecking=yes",
                    "-o", "GlobalKnownHostsFile=/dev/null", "-o", "UserKnownHostsFile=" + msys(hosts),
                    "-o", "IdentityAgent=" + msys(socket_path)]

        probe = command(ssh_args(known, sock) + ["pabssh@127.0.0.1", "id -un"], timeout=15)
        if probe.stdout.strip() != "pabssh":
            raise RuntimeError("isolated SSH authentication did not return expected account")
        fixture = {"workspace": str(workspace), "remote_url": f"ssh://pabssh@127.0.0.1:{port}/fixture/remote.git",
                   "command": shlex.join(ssh_args(known, sock)),
                   "wrong_host_command": shlex.join(ssh_args(wrong, sock)),
                   "no_agent_command": shlex.join(ssh_args(known, root / "absent.sock"))}
        (root / "fixture.json").write_text(json.dumps(fixture), encoding="utf-8")
        runner, result = root / "run.ps1", root / "result.json"
        stdout, stderr = root / "stdout.log", root / "stderr.log"
        runner.write_text("\n".join([
            "$ErrorActionPreference = 'Stop'",
            "$env:PAB_EXECUTION_TEST_USER = " + ps_literal(args.session),
            "$env:PAB_EXECUTION_TEST_WORKER = " + ps_literal(worker),
            "$env:PAB_EXECUTION_TEST_GIT_SSH = Get-Content -Raw -LiteralPath " + ps_literal(root / "fixture.json"),
            "$start = New-Object System.Diagnostics.ProcessStartInfo",
            "$start.FileName = " + ps_literal(binary),
            "$start.Arguments = 'native_user_git_acceptance --ignored --nocapture --test-threads=1'",
            "$start.UseShellExecute = $false; $start.CreateNoWindow = $true",
            "$start.RedirectStandardOutput = $true; $start.RedirectStandardError = $true",
            "$p = New-Object System.Diagnostics.Process; $p.StartInfo = $start; $null = $p.Start()",
            "$out = $p.StandardOutput.ReadToEndAsync(); $err = $p.StandardError.ReadToEndAsync()",
            "$done = $p.WaitForExit(180000)",
            "if (-not $done) { $p.Kill(); $p.WaitForExit(); $code = 124 } else { $code = $p.ExitCode }",
            "[IO.File]::WriteAllText(" + ps_literal(stdout) + ", $out.GetAwaiter().GetResult())",
            "[IO.File]::WriteAllText(" + ps_literal(stderr) + ", $err.GetAwaiter().GetResult())",
            "$p.Dispose()",
            "@{finished=$true;exit_code=$code} | ConvertTo-Json | Set-Content -Encoding UTF8 -LiteralPath " + ps_literal(result),
        ]), encoding="utf-8-sig")
        powershell("$a = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument "
                   + ps_literal('-NoProfile -ExecutionPolicy Bypass -File "' + str(runner) + '"')
                   + "; $p = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest; "
                   + "Register-ScheduledTask -TaskName " + ps_literal(task) + " -Action $a -Principal $p | Out-Null")
        task_created = True
        powershell("Start-ScheduledTask -TaskName " + ps_literal(task))
        until = time.monotonic() + 210
        while not result.exists():
            if time.monotonic() >= until:
                raise TimeoutError("SYSTEM acceptance task did not produce its result")
            time.sleep(0.2)
        outcome = json.loads(result.read_text(encoding="utf-8-sig"))
        out, err = stdout.read_text(errors="replace"), stderr.read_text(errors="replace")
        if outcome["exit_code"] != 0 or "SSH_ACCEPTANCE" not in out:
            print(json.dumps({"process_result": outcome}), flush=True)
            print(out[-14000:], flush=True)
            print(err[-5000:], flush=True)
            raise RuntimeError("native Windows Git SSH test failed")
        print(next(line for line in out.splitlines() if "SSH_ACCEPTANCE" in line), flush=True)
        print(json.dumps({"status": "pass", "session": args.session, "private_key_on_disk_during_test": False,
                          "client": "Git for Windows OpenSSH", "server": "isolated Linux OpenSSH"}), flush=True)
    except subprocess.CalledProcessError as error:
        # OpenSSH diagnostics never contain the generated private key contents.
        print((error.stdout or "")[-4000:], flush=True)
        print((error.stderr or "")[-4000:], flush=True)
        raise
    finally:
        if task_created:
            powershell("Stop-ScheduledTask -TaskName " + ps_literal(task)
                       + "; Unregister-ScheduledTask -TaskName " + ps_literal(task) + " -Confirm:$false")
        if container_started:
            command(["docker", "rm", "-f", container])
        if agent is not None and agent.poll() is None:
            agent.terminate()
            agent.wait(timeout=10)
        for path, prefix in ((root, "pab-ssh-acceptance-"), (workspace, "pab-git-acceptance-")):
            if path.exists():
                assert path.resolve().parent == home and path.name.startswith(prefix) and not path.is_symlink()
                shutil.rmtree(path)
        print(json.dumps({"fixture_removed": not root.exists() and not workspace.exists(),
                          "agent_stopped": agent is None or agent.poll() is not None}), flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--session", type=int, required=True)
    parser.add_argument("--worker", required=True)
    parser.add_argument("--test-binary", required=True)
    parser.add_argument("--git-bin", default=r"C:\Program Files\Git\usr\bin")
    parser.add_argument("--image", default="pab-linux-execution-ssh:bookworm")
    run(parser.parse_args())
