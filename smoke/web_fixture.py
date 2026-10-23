"""Create/restart/stop an owned loopback Web lab using already built candidate assets.

No compilation, installation, shared database or production service mutation.
Docker images must already be available locally. All generated files stay in smoke/local.
"""
from __future__ import annotations
import argparse
from datetime import datetime, timedelta, timezone
import hashlib
import ipaddress
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import time
import tomllib
from urllib.parse import urlsplit
import uuid

ROOT = Path(__file__).resolve().parents[1]
LOCAL = ROOT / "smoke/local"
sys.path.insert(0, str(ROOT / "scripts"))


def docker(*args):
    return subprocess.check_output(["docker", *args], text=True, stderr=subprocess.PIPE).strip()


def metadata(config):
    config = Path(config).resolve()
    if (not config.is_relative_to(LOCAL.resolve()) or config.name != "pab-server.toml"
            or not config.parent.name.startswith("web-")):
        raise ValueError("Select smoke/local/web-<run-id>/pab-server.toml")
    value = json.loads((config.parent / "owner.json").read_text(encoding="utf-8"))
    if config.parent.name != "web-" + str(uuid.UUID(value["run_id"])):
        raise ValueError("Fixture ownership mismatch")
    return config, value


def fixture_info(config):
    config, owner = metadata(config)
    settings = tomllib.loads(config.read_text(encoding="utf-8"))
    database = urlsplit(settings["database"]["url"])
    info = json.loads(docker("inspect", owner["container"]))[0]
    bindings = info["NetworkSettings"]["Ports"].get("5432/tcp") or []
    if (info["Config"]["Labels"].get("pixels.smoke") != owner["run_id"]
            or info["Id"] != owner["container_id"] or not info["State"]["Running"]
            or database.hostname != "127.0.0.1" or database.path != "/" + owner["database"]
            or not any(p["HostIp"] == "127.0.0.1" and int(p["HostPort"]) == database.port for p in bindings)
            or settings["listen"] != "127.0.0.1:38623"
            or settings["web"]["origin"] != "https://127.0.0.1:38623"):
        raise ValueError("Isolated fixture ownership/network contract failed")
    executable = config.parent / ("pab-server.exe" if os.name == "nt" else "pab-server")
    if hashlib.sha256(executable.read_bytes()).hexdigest() != owner["server_sha256"]:
        raise ValueError("Fixture executable changed")
    return {"config": str(config), "container": owner["container"], "database": owner["database"],
            "origin": settings["web"]["origin"], "cert": settings["tls"]["cert"], "executable": str(executable)}


def stop_owned_server(config):
    import psutil
    config, owner = metadata(config)
    executable = config.parent / ("pab-server.exe" if os.name == "nt" else "pab-server")
    if not executable.is_file() or hashlib.sha256(executable.read_bytes()).hexdigest() != owner["server_sha256"]:
        raise ValueError("Cannot stop an unverified fixture executable")
    owned = []
    for process in psutil.process_iter(["exe", "cmdline"]):
        try:
            if (process.info["exe"] and Path(process.info["exe"]).resolve() == executable
                    and str(Path(config).resolve()) in (process.info["cmdline"] or [])):
                process.terminate(); owned.append(process)
        except psutil.NoSuchProcess:
            pass
    for process in owned:
        try:
            process.wait(timeout=20)
        except psutil.NoSuchProcess:
            pass
    return {"status": "stopped"}


def restart(config, stop=False):
    if stop:
        return stop_owned_server(config)
    info = fixture_info(config)
    executable = Path(info["executable"]).resolve()
    stop_owned_server(config)
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 38623))
    with (executable.parent / "server.log").open("ab") as log:
        child = subprocess.Popen([str(executable), "serve", "--config", str(Path(config).resolve())],
            cwd=ROOT, stdin=subprocess.DEVNULL, stdout=log, stderr=log,
            creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
    for _ in range(100):
        if child.poll() is not None:
            raise ValueError("Fixture exited; inspect its local server.log")
        try:
            with socket.create_connection(("127.0.0.1", 38623), timeout=.2):
                return {"status": "started", "pid": child.pid}
        except OSError:
            time.sleep(.1)
    child.terminate(); child.wait(timeout=20)
    raise ValueError("Fixture startup timed out")


def create(binary, assets, image):
    from cryptography import x509
    from cryptography.hazmat.primitives import hashes, serialization
    from cryptography.hazmat.primitives.asymmetric import rsa
    from cryptography.x509.oid import NameOID
    from service_config import write_config
    binary, assets = binary.resolve(), assets.resolve()
    if not binary.is_file() or not (assets / "index.html").is_file():
        raise ValueError("Supply a built Server executable and Web assets directory")
    # Do not fetch mutable image tags implicitly during a release test.
    docker("image", "inspect", image)
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 38623))
    run_id = str(uuid.uuid4())
    folder = LOCAL / ("web-" + run_id)
    folder.mkdir(parents=True)
    container = "pab-smoke-" + run_id
    database = "admin_browser_" + run_id.replace("-", "")
    owner = {"run_id": run_id, "container": container, "database": database, "phase": "creating"}
    owner_path = folder / "owner.json"
    owner_path.write_text(json.dumps(owner, indent=2), encoding="utf-8")
    # No mounted host data; this trust-authenticated database is loopback-only and ephemeral.
    owner["container_id"] = docker("run", "-d", "--pull=never", "--name", container,
        "--label", "pixels.smoke=" + run_id, "-p", "127.0.0.1::5432",
        "-e", "POSTGRES_HOST_AUTH_METHOD=trust", "-e", "POSTGRES_DB=" + database, image)
    owner_path.write_text(json.dumps(owner, indent=2), encoding="utf-8")
    for _ in range(90):
        result = subprocess.run(["docker", "exec", container, "pg_isready", "-U", "postgres", "-d", database],
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        if result.returncode == 0:
            break
        time.sleep(1)
    else:
        raise ValueError(f"Database startup failed; owned resources recorded in {owner_path}")
    port = json.loads(docker("inspect", container))[0]["NetworkSettings"]["Ports"]["5432/tcp"][0]["HostPort"]
    key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "localhost")])
    now = datetime.now(timezone.utc)
    cert = (x509.CertificateBuilder().subject_name(name).issuer_name(name).public_key(key.public_key())
        .serial_number(x509.random_serial_number()).not_valid_before(now-timedelta(minutes=1))
        .not_valid_after(now+timedelta(days=3)).add_extension(x509.SubjectAlternativeName([
            x509.DNSName("localhost"), x509.IPAddress(ipaddress.ip_address("127.0.0.1"))]), False).sign(key, hashes.SHA256()))
    (folder / "cert.pem").write_bytes(cert.public_bytes(serialization.Encoding.PEM))
    (folder / "key.pem").write_bytes(key.private_bytes(serialization.Encoding.PEM,
        serialization.PrivateFormat.PKCS8, serialization.NoEncryption()))
    executable = folder / ("pab-server.exe" if os.name == "nt" else "pab-server")
    shutil.copy2(binary, executable)
    owner["server_sha256"] = hashlib.sha256(executable.read_bytes()).hexdigest()
    # Copy deployable resources, never a source checkout. Bind them to this fixture.
    shutil.copytree(assets, folder / "web")
    config = write_config(folder / "pab-server.toml", {"listen": "127.0.0.1:38623",
        "database": {"url": f"postgres://postgres@127.0.0.1:{port}/{database}"},
        "tls": {"cert": folder / "cert.pem", "key": folder / "key.pem"},
        "web": {"origin": "https://127.0.0.1:38623", "assets": folder / "web"},
        "relay": {"control_secret": uuid.uuid4().hex + uuid.uuid4().hex},
        "log": {"directory": folder / "logs"}})
    owner["phase"] = "prepared"
    owner_path.write_text(json.dumps(owner, indent=2), encoding="utf-8")
    with (folder / "init.log").open("wb") as log:
        subprocess.run([str(executable), "init", "--config", str(config)], check=True, stdout=log, stderr=log)
    restart(config)
    return {"config": str(config), "origin": "https://127.0.0.1:38623", "container": container}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="action", required=True)
    start = sub.add_parser("create")
    start.add_argument("--binary", type=Path, required=True)
    start.add_argument("--assets", type=Path, required=True)
    start.add_argument("--postgres-image", required=True, help="Preloaded PostgreSQL image tag or digest")
    for action in ("info", "restart", "stop", "destroy"):
        sub.add_parser(action).add_argument("--config", type=Path, required=True)
    args = parser.parse_args()
    if args.action == "create":
        result = create(args.binary, args.assets, args.postgres_image)
    elif args.action == "info":
        result = fixture_info(args.config)
    elif args.action == "destroy":
        config, owner = metadata(args.config)
        info = json.loads(docker("inspect", owner["container"]))[0]
        if info["Id"] != owner["container_id"] or info["Config"]["Labels"].get("pixels.smoke") != owner["run_id"]:
            raise ValueError("Cannot destroy a container not owned by this fixture")
        if config.is_file():
            stop_owned_server(config)
        docker("rm", "-f", "-v", owner["container_id"])
        result = {"status": "destroyed owned database; local evidence retained"}
    else:
        result = restart(args.config, stop=args.action == "stop")
    print(json.dumps(result))


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        # Exception commands can contain runtime credentials; keep console errors generic.
        print(f"Fixture failed ({type(error).__name__}); inspect smoke/local ownership and logs", file=sys.stderr)
        raise SystemExit(2)
