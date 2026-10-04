"""Wrap a verified Windows desktop ZIP in an NSIS installer."""

from argparse import ArgumentParser
from hashlib import sha256
from pathlib import Path
from tempfile import TemporaryDirectory
from urllib.parse import urlsplit
import json
import subprocess
import zipfile


ROOT = Path(__file__).resolve().parents[2]
PACKAGES = ROOT / ".build" / "packages"
BINARIES = {
    "pab-desktop.exe": ROOT / "apps" / "desktop" / "src-tauri" / "target",
    "pab-executor.exe": ROOT / "target",
    "pab-mcp.exe": ROOT / "target",
}
CONTENTS = set(BINARIES) | {
    "install.ps1",
    "run-app.ps1",
    "launch-app.ps1",
    "run-session-supervisor.ps1",
    "uninstall.ps1",
    "INSTALL-WINDOWS.txt",
}


def parse_url(value: str, scheme: str) -> str:
    parsed = urlsplit(value)
    if parsed.scheme != scheme or not parsed.hostname or any(char in value for char in '"$\r\n'):
        raise ValueError(f"expected a safe {scheme} URL")
    return value


def digest_file(path: Path) -> str:
    digest = sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main() -> None:
    parser = ArgumentParser(description=__doc__)
    parser.add_argument("--control-url", default="wss://pab.rgaa.vip/control")
    parser.add_argument("--relay-url", default="https://pab-relay.rgaa.vip")
    parser.add_argument("--profile", choices=("debug", "release"), default="debug")
    args = parser.parse_args()

    control_url = parse_url(args.control_url, "wss")
    relay_url = parse_url(args.relay_url, "https")
    profile = args.profile
    archive = PACKAGES / f"pixels-agent-bridge-windows-x86_64-{profile}.zip"
    checksum_file = PACKAGES / ("SHA256-debug.json" if profile == "debug" else "SHA256.json")
    expected_hash = json.loads(checksum_file.read_text(encoding="utf-8"))[archive.name]["sha256"]
    if digest_file(archive) != expected_hash:
        raise ValueError(f"Windows archive checksum does not match {checksum_file}")

    compiler = ROOT / "tools" / "nsis" / "makensis.exe"
    icon = ROOT / "apps" / "desktop" / "src-tauri" / "icons" / "icon.ico"
    script = ROOT / "packaging" / "desktop" / "windows" / "setup.nsi"
    if not compiler.is_file():
        raise FileNotFoundError(f"Copy NSIS into {compiler.parent} first")

    output = PACKAGES / f"pixels-agent-bridge-windows-x86_64-{profile}-setup.exe"
    with zipfile.ZipFile(archive) as package, TemporaryDirectory(
        prefix="pab-nsis-", dir=ROOT / ".build"
    ) as temporary:
        if set(package.namelist()) != CONTENTS or package.testzip() is not None:
            raise ValueError("Windows archive contents or CRC are invalid")
        for name, base in BINARIES.items():
            source = base / profile / name
            if sha256(package.read(name)).digest() != bytes.fromhex(digest_file(source)):
                raise ValueError(f"Windows archive contains an outdated {name}")
        payload = Path(temporary)
        for name in sorted(CONTENTS):
            (payload / name).write_bytes(package.read(name))
        command = [
            str(compiler),
            "/INPUTCHARSET",
            "UTF8",
            f"/DPAYLOAD_DIR={payload}",
            f"/DOUTPUT_FILE={output}",
            f"/DCONTROL_URL={control_url}",
            f"/DRELAY_URL={relay_url}",
            f"/DAPP_ICON={icon}",
            f"/DBUILD_PROFILE={profile}",
            str(script),
        ]
        subprocess.run(command, cwd=ROOT, check=True)

    metadata = {"file": output.name, "bytes": output.stat().st_size, "sha256": digest_file(output)}
    (PACKAGES / f"SHA256-windows-setup-{profile}.json").write_text(
        json.dumps(metadata, indent=2) + "\n", encoding="utf-8"
    )
    print(json.dumps(metadata, indent=2))


if __name__ == "__main__":
    main()
