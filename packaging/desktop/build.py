"""Package already-built desktop binaries for one build profile."""

from argparse import ArgumentParser
from hashlib import sha256
from io import BytesIO
from pathlib import Path
import json
import tarfile
import zipfile
import sys


root = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(root / 'scripts'))
from build_version import verify_artifacts
scripts = Path(__file__).resolve().parent
parser = ArgumentParser(description=__doc__)
parser.add_argument("--platform", choices=("all", "windows", "linux"), default="all")
parser.add_argument("--profile", choices=("debug", "release"), default="release")
parser.add_argument("--windows-bin-dir", type=Path)
parser.add_argument(
    "--windows-desktop-bin",
    type=Path,
)
parser.add_argument("--linux-bin-dir", type=Path)
parser.add_argument("--macos-bin-dir", type=Path)
parser.add_argument("--output-dir", type=Path, default=root / ".build" / "packages")
args = parser.parse_args()
args.windows_bin_dir = args.windows_bin_dir or root / "target" / args.profile
args.windows_desktop_bin = args.windows_desktop_bin or (
    root / "apps" / "desktop" / "src-tauri" / "target" / args.profile / "pab-desktop.exe"
)
args.linux_bin_dir = args.linux_bin_dir or root / ".build" / f"guest-desktop-linux-{args.profile}"
args.output_dir.mkdir(parents=True, exist_ok=True)
versions = {}


def package_windows():
    archive_path = args.output_dir / f"pixels-agent-bridge-windows-x86_64-{args.profile}.zip"
    files = [
        args.windows_desktop_bin,
        *(args.windows_bin_dir / name for name in (
            "pab-mcp.exe", "pab-executor.exe"
        )),
        *(scripts / "windows" / name for name in (
            "install.ps1", "run-app.ps1", "launch-app.ps1",
            "run-session-supervisor.ps1", "uninstall.ps1", "INSTALL-WINDOWS.txt"
        )),
    ]
    for file in files:
        if not file.is_file():
            raise FileNotFoundError(file)
    versions[archive_path.name] = verify_artifacts(root, 'desktop', args.profile, {file.name: file for file in files[:3]})
    with zipfile.ZipFile(archive_path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for file in files:
            archive.write(file, file.name)
    return archive_path


def package_unix(platform, architecture, binaries):
    archive_path = args.output_dir / f"pixels-agent-bridge-{platform}-{architecture}-{args.profile}.tar.gz"
    files = [
        *(binaries / name for name in ("pab-mcp", "pab-executor", "pab-desktop")),
        *(scripts / "unix" / name for name in (
            "install.sh", "run-app.sh", "run-mcp.sh", "run-executor.sh", "uninstall.sh"
        )),
    ]
    for file in files:
        if not file.is_file():
            raise FileNotFoundError(file)
    versions[archive_path.name] = verify_artifacts(root, platform, args.profile, {file.name: file for file in files[:3]})
    with tarfile.open(archive_path, "w:gz") as archive:
        for file in files:
            info = archive.gettarinfo(str(file), arcname=file.name)
            info.mode = 0o755
            if file.suffix == ".sh":
                content = file.read_bytes().replace(b"\r\n", b"\n")
                info.size = len(content)
                archive.addfile(info, BytesIO(content))
            else:
                with file.open("rb") as content:
                    archive.addfile(info, content)
    return archive_path


if args.platform in ("all", "windows"):
    package_windows()
if args.platform in ("all", "linux"):
    package_unix("linux", "x86_64", args.linux_bin_dir)
if args.platform == "all" and args.macos_bin_dir is not None:
    package_unix("macos", "aarch64", args.macos_bin_dir)

checksum_name = "SHA256.json" if args.profile == "release" else "SHA256-debug.json"
previous_path = args.output_dir / checksum_name
previous = json.loads(previous_path.read_text(encoding='utf-8')) if previous_path.exists() else {}
manifest = {}
for archive in sorted(args.output_dir.iterdir()):
    if not archive.name.startswith("pixels-agent-bridge-"):
        continue
    if not archive.name.endswith((f"-{args.profile}.zip", f"-{args.profile}.tar.gz")):
        continue
    if not archive.is_file():
        continue
    digest = sha256()
    with archive.open("rb") as content:
        while chunk := content.read(1024 * 1024):
            digest.update(chunk)
    manifest[archive.name] = {
        "bytes": archive.stat().st_size,
        "sha256": digest.hexdigest(),
    }
    if archive.name in versions:
        manifest[archive.name]['version'] = versions[archive.name]
    elif previous.get(archive.name, {}).get('sha256') == digest.hexdigest() and 'version' in previous[archive.name]:
        manifest[archive.name]['version'] = previous[archive.name]['version']
(args.output_dir / checksum_name).write_text(json.dumps(manifest, indent=2) + "\n")
print(json.dumps(manifest, indent=2))
