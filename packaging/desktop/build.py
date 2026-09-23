"""Package already-built stripped Release desktop binaries."""

from argparse import ArgumentParser
from hashlib import sha256
from pathlib import Path
import json
import tarfile
import zipfile


root = Path(__file__).resolve().parents[2]
scripts = Path(__file__).resolve().parent
parser = ArgumentParser(description=__doc__)
parser.add_argument("--windows-bin-dir", type=Path, default=root / "target" / "release")
parser.add_argument(
    "--windows-desktop-bin",
    type=Path,
    default=root / "apps" / "desktop" / "src-tauri" / "target" / "release" / "pab-desktop.exe",
)
parser.add_argument("--linux-bin-dir", type=Path, default=root / ".build" / "guest-desktop-linux-release")
parser.add_argument("--macos-bin-dir", type=Path)
parser.add_argument("--output-dir", type=Path, default=root / ".build" / "packages")
args = parser.parse_args()
args.output_dir.mkdir(parents=True, exist_ok=True)


def package_windows():
    archive_path = args.output_dir / "pixels-agent-bridge-windows-x86_64-release.zip"
    files = [
        args.windows_desktop_bin,
        *(args.windows_bin_dir / name for name in (
            "pab-mcp.exe", "pab-bridge.exe", "pab-executor.exe"
        )),
        *(scripts / "windows" / name for name in (
            "install.ps1", "run-ui.ps1", "run-executor.ps1", "run-device-ui.ps1", "uninstall.ps1"
        )),
    ]
    for file in files:
        if not file.is_file():
            raise FileNotFoundError(file)
    with zipfile.ZipFile(archive_path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for file in files:
            archive.write(file, file.name)
    return archive_path


def package_unix(platform, architecture, binaries):
    archive_path = args.output_dir / f"pixels-agent-bridge-{platform}-{architecture}-release.tar.gz"
    files = [
        *(binaries / name for name in ("pab-mcp", "pab-bridge", "pab-executor")),
        *(scripts / "unix" / name for name in (
            "install.sh", "run-ui.sh", "run-executor.sh", "uninstall.sh"
        )),
    ]
    for file in files:
        if not file.is_file():
            raise FileNotFoundError(file)
    with tarfile.open(archive_path, "w:gz") as archive:
        for file in files:
            info = archive.gettarinfo(str(file), arcname=file.name)
            info.mode = 0o755
            with file.open("rb") as content:
                archive.addfile(info, content)
    return archive_path


archives = [
    package_windows(),
    package_unix("linux", "x86_64", args.linux_bin_dir),
]
if args.macos_bin_dir is not None:
    archives.append(package_unix("macos", "aarch64", args.macos_bin_dir))

manifest = {}
for archive in archives:
    digest = sha256()
    with archive.open("rb") as content:
        while chunk := content.read(1024 * 1024):
            digest.update(chunk)
    manifest[archive.name] = {
        "bytes": archive.stat().st_size,
        "sha256": digest.hexdigest(),
    }
(args.output_dir / "SHA256.json").write_text(json.dumps(manifest, indent=2) + "\n")
print(json.dumps(manifest, indent=2))
