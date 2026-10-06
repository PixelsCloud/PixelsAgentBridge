"""Run the native user Git test against a temporary loopback OpenSSH server.

Unix root fixture only. Requires openssh-server/client and prebuilt Executor test
and worker binaries. No installed service, ~/.ssh, login agent, or real repository
is changed. Reports contain assertions/status only, never private key material.
"""
import argparse
import json
import os
from pathlib import Path
import pwd
import shlex
import shutil
import signal
import socket
import subprocess
import tempfile
import time


def wait_ready(process, probe, timeout=10):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError("fixture process exited before readiness")
        try:
            if probe():
                return
        except OSError:
            pass
        time.sleep(0.05)
    raise TimeoutError("fixture readiness deadline exceeded")


def listening(port):
    with socket.create_connection(("127.0.0.1", port), timeout=0.2):
        return True


def stop(process):
    if process is None or process.poll() is not None:
        return
    # Only groups created by this harness; never a service PID from a file.
    assert os.getpgid(process.pid) == process.pid
    os.killpg(process.pid, signal.SIGTERM)
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait(timeout=5)


def run(args):
    if os.geteuid() != 0:
        raise RuntimeError("run the fixture as root; it launches the selected UID")
    account = pwd.getpwuid(args.uid)
    if args.uid == 0:
        raise ValueError("select an ordinary user UID")
    sshd = shutil.which("sshd") or "/usr/sbin/sshd"
    ssh = shutil.which("ssh") or "/usr/bin/ssh"
    agent = shutil.which("ssh-agent") or "/usr/bin/ssh-agent"
    keygen = shutil.which("ssh-keygen") or "/usr/bin/ssh-keygen"
    add = shutil.which("ssh-add") or "/usr/bin/ssh-add"
    worker, binary = Path(args.worker).resolve(strict=True), Path(args.test_binary).resolve(strict=True)
    # OpenSSH StrictModes rejects writable ancestors such as /private/tmp.
    # Use an isolated child of this user's home, retaining strict verification.
    fixture_parent = Path(account.pw_dir).resolve(strict=True)
    root = Path(tempfile.mkdtemp(prefix="pab-ssh-acceptance-", dir=fixture_parent)).resolve()
    root.chmod(0o755)
    user_dir = root / "user"
    user_dir.mkdir(mode=0o700)
    os.chown(user_dir, account.pw_uid, account.pw_gid)
    user_options = {"user": account.pw_uid, "group": account.pw_gid,
                    "extra_groups": os.getgrouplist(account.pw_name, account.pw_gid),
                    "env": {"HOME": account.pw_dir, "USER": account.pw_name,
                            "LOGNAME": account.pw_name, "PATH": "/usr/bin:/bin:/usr/sbin:/sbin"}}
    daemon = ssh_agent = None
    daemon_log = None
    try:
        for path in [root / "host", root / "wrong-host"]:
            subprocess.run([keygen, "-q", "-t", "ed25519", "-N", "", "-f", str(path)], check=True)
        identity = user_dir / "fixture-key"
        subprocess.run([keygen, "-q", "-t", "ed25519", "-N", "", "-f", str(identity)], check=True, **user_options)
        public = identity.with_suffix(".pub")
        (root / "authorized_keys").write_bytes(public.read_bytes())
        (root / "authorized_keys").chmod(0o644)
        agent_socket = user_dir / "agent.sock"
        ssh_agent = subprocess.Popen([agent, "-D", "-a", str(agent_socket)], start_new_session=True,
                                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, **user_options)
        wait_ready(ssh_agent, agent_socket.exists)
        with_agent = dict(user_options)
        with_agent["env"] = dict(user_options["env"], SSH_AUTH_SOCK=str(agent_socket))
        subprocess.run([add, str(identity)], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, **with_agent)
        # Only the agent retains the private key; the client receives a public
        # identity file and cannot silently fall back to reading the private key.
        identity.unlink()
        with socket.socket() as reserve:
            reserve.bind(("127.0.0.1", 0))
            port = reserve.getsockname()[1]
        known_hosts, wrong_hosts = user_dir / "known_hosts", user_dir / "wrong_hosts"
        for destination, source in [(known_hosts, root / "host.pub"), (wrong_hosts, root / "wrong-host.pub")]:
            destination.write_text(f"[127.0.0.1]:{port} " + source.read_text(), encoding="utf-8")
            os.chown(destination, account.pw_uid, account.pw_gid)
        config = root / "sshd_config"
        config.write_text("\n".join([
            f"Port {port}", "ListenAddress 127.0.0.1", f'HostKey "{root / "host"}"',
            f'PidFile "{root / "sshd.pid"}"', f'AuthorizedKeysFile "{root / "authorized_keys"}"',
            f"AllowUsers {account.pw_name}", "PubkeyAuthentication yes", "PasswordAuthentication no",
            "KbdInteractiveAuthentication no", "UsePAM no", "PermitRootLogin no", "StrictModes yes",
            "AllowAgentForwarding no", "AllowTcpForwarding no", "X11Forwarding no", "PermitTTY no",
            "LogLevel VERBOSE", "PrintMotd no", "LoginGraceTime 10", "MaxStartups 10",
        ]) + "\n", encoding="utf-8")
        subprocess.run([sshd, "-t", "-f", str(config)], check=True)
        daemon_log = (root / "sshd.log").open("wb")
        daemon = subprocess.Popen([sshd, "-D", "-e", "-f", str(config)], start_new_session=True,
                                  stdout=daemon_log, stderr=daemon_log)
        wait_ready(daemon, lambda: listening(port))

        def command(hosts, sock):
            return shlex.join([ssh, "-F", "/dev/null", "-p", str(port), "-i", str(public),
                               "-o", "IdentitiesOnly=yes", "-o", "BatchMode=yes", "-o", "ConnectTimeout=5",
                               "-o", "StrictHostKeyChecking=yes", "-o", "GlobalKnownHostsFile=/dev/null",
                               "-o", "UserKnownHostsFile=" + str(hosts), "-o", "IdentityAgent=" + str(sock)])
        fixture = {"prefix": f"ssh://{account.pw_name}@127.0.0.1:{port}",
                   "command": command(known_hosts, agent_socket),
                   "wrong_host_command": command(wrong_hosts, agent_socket),
                   "no_agent_command": command(known_hosts, user_dir / "absent-agent.sock")}
        probe = subprocess.run(shlex.split(fixture["command"]) + ["-v", account.pw_name + "@127.0.0.1", "/usr/bin/id -u"],
                               capture_output=True, text=True, timeout=15, **user_options)
        if probe.returncode or probe.stdout.strip() != str(args.uid):
            print(probe.stderr[-8000:], flush=True)
            daemon_log.flush()
            print((root / "sshd.log").read_text(errors="replace")[-4000:], flush=True)
            raise RuntimeError("isolated SSH fixture authentication failed before product test")
        env = dict(os.environ, PAB_EXECUTION_TEST_USER=str(args.uid), PAB_EXECUTION_TEST_WORKER=str(worker),
                   PAB_EXECUTION_TEST_GIT_SSH=json.dumps(fixture))
        result = subprocess.run([str(binary), "native_user_git_acceptance", "--ignored", "--nocapture", "--test-threads=1"],
                                env=env, timeout=180, capture_output=True, text=True)
        # Only expose compact acceptance evidence; retain useful failure details
        # in the invoking task output, which never contains generated private keys.
        if result.returncode:
            print(result.stdout[-12000:], flush=True)
            print(result.stderr[-3000:], flush=True)
            daemon_log.flush()
            print((root / "sshd.log").read_text(errors="replace")[-3000:], flush=True)
            raise RuntimeError(f"native Git SSH acceptance failed: {result.returncode}")
        marker = next((line for line in result.stdout.splitlines() if "SSH_ACCEPTANCE" in line), None)
        if marker is None:
            raise RuntimeError("test binary did not exercise SSH acceptance")
        print(marker, flush=True)
        print(json.dumps({"status": "pass", "uid": args.uid, "private_key_on_disk_during_test": False,
                          "transport": "OpenSSH loopback", "test": "native_user_git_acceptance"}), flush=True)
    finally:
        stop(daemon)
        stop(ssh_agent)
        if daemon_log:
            daemon_log.close()
        if root.parent == fixture_parent and root.name.startswith("pab-ssh-acceptance-") and not root.is_symlink():
            shutil.rmtree(root)
        print(json.dumps({"fixture_removed": not root.exists(), "agent_stopped": ssh_agent is None or ssh_agent.poll() is not None,
                          "sshd_stopped": daemon is None or daemon.poll() is not None}), flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--uid", type=int, required=True)
    parser.add_argument("--worker", required=True)
    parser.add_argument("--test-binary", required=True)
    run(parser.parse_args())
