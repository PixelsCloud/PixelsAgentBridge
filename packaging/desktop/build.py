"""Package already-built desktop binaries for one build profile."""

from argparse import ArgumentParser
from hashlib import sha256
from io import BytesIO
from pathlib import Path
import json
import platform as host_platform
import tarfile
import zipfile
import sys


root = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(root / 'scripts'))
scripts = Path(__file__).resolve().parent
parser = ArgumentParser(description=__doc__)
parser.add_argument("--platform", choices=("all", "windows", "linux", "macos"), default="all")
parser.add_argument("--profile", choices=("debug", "release"), default="release")
parser.add_argument("--windows-bin-dir", type=Path)
parser.add_argument(
    "--windows-desktop-bin",
    type=Path,
)
parser.add_argument("--linux-bin-dir", type=Path)
parser.add_argument("--macos-bin-dir", type=Path)
parser.add_argument("--macos-app", type=Path)
parser.add_argument("--macos-arch", choices=("aarch64", "x86_64"),
                    default="aarch64" if host_platform.machine() == "arm64" else "x86_64")
parser.add_argument("--output-dir", type=Path, default=root / ".build" / "packages")
args = parser.parse_args()
args.windows_bin_dir = args.windows_bin_dir or root / "target" / args.profile
args.windows_desktop_bin = args.windows_desktop_bin or (
    root / "apps" / "desktop" / "src-tauri" / "target" / args.profile / "pab-desktop.exe"
)
args.linux_bin_dir = args.linux_bin_dir or root / ".build" / f"guest-desktop-linux-{args.profile}"
args.macos_app = args.macos_app or (
    root / "apps/desktop/src-tauri/target" / args.profile / "bundle/macos/Pixels Agent Bridge.app"
)
args.output_dir.mkdir(parents=True, exist_ok=True)
versions = {}


def package_windows():
    from build_version import verify_artifacts

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
        *(binaries / name for name in (
            ("pab-mcp", "pab-executor") if platform == "macos"
            else ("pab-mcp", "pab-executor", "pab-desktop")
        )),
        *(scripts / ("macos" if platform == "macos" else "unix") / name for name in (
            "install.sh", "run-app.sh", "run-mcp.sh", "run-executor.sh", "uninstall.sh"
        )),
    ]
    if platform == "macos":
        files.extend(scripts / "macos" / name for name in (
            "com.pixelsagentbridge.executor.plist", "com.pixelsagentbridge.session-helper.plist",
        ))
    for file in files:
        if not file.is_file():
            raise FileNotFoundError(file)
    if platform == "macos" and not (args.macos_app / "Contents/MacOS/pab-desktop").is_file():
        raise FileNotFoundError(args.macos_app)
    if platform == "macos":
        from build_version import verify_artifacts, macos_artifacts, macos_app_version

        version = verify_artifacts(root, f'macos-{architecture}', args.profile, macos_artifacts(binaries, args.macos_app))
        if macos_app_version(args.macos_app) != version:
            raise ValueError('macOS app version does not match its build record')
        versions[archive_path.name] = version
    else:
        from build_version import verify_artifacts

        versions[archive_path.name] = verify_artifacts(root, platform, args.profile, {file.name: file for file in files[:3]})
    with tarfile.open(archive_path, "w:gz") as archive:
        if platform == "macos":
            archive.add(args.macos_app, arcname="Pixels Agent Bridge.app")
        for file in files:
            info = archive.gettarinfo(str(file), arcname=file.name)
            info.mode = 0o644 if file.suffix == ".plist" else 0o755
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
if args.platform == "macos" or (args.platform == "all" and args.macos_bin_dir is not None):
    package_unix("macos", args.macos_arch, args.macos_bin_dir or root / "target" / args.profile)

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
