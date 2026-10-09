"""Wrap a checksum-verified macOS archive in a double-clickable Installer product.

Python 3.12+ and Apple's pkgbuild/productbuild tools are required. This builder
does not install anything. A scripts-only component reuses the macOS installer;
its receipt is not a file inventory and uninstall.sh remains the uninstall entry.
"""

import argparse
from hashlib import sha256
import html
import json
from pathlib import Path, PurePosixPath
import plistlib
import shlex
import shutil
import subprocess
import sys
import tarfile
import tempfile
from urllib.parse import urlsplit
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
from build_version import parse_version
SCRIPTS = ROOT / "packaging/desktop/macos"
PACKAGES = ROOT / ".build/packages"
APP = "Pixels Agent Bridge.app"
IDENTIFIER = "vip.rgaa.pab.desktop.installer"
CONTROL_URL = "wss://pab.rgaa.vip/control"
RELAY_URL = "https://pab-relay.rgaa.vip"
REQUIRED = {
    "pab-executor", "pab-mcp", "install.sh", "uninstall.sh", "run-app.sh", "lifecycle.sh",
    "run-mcp.sh", "run-executor.sh", "com.pixelsagentbridge.executor.plist",
    "com.pixelsagentbridge.session-helper.plist",
    "com.pixelsagentbridge.login-helper.plist",
}


def digest(path):
    value = sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def checked_url(value, scheme):
    parsed = urlsplit(value)
    if (parsed.scheme != scheme or not parsed.hostname or parsed.username is not None
            or parsed.password is not None or parsed.fragment
            or any(ord(c) < 33 or ord(c) == 127 for c in value)):
        raise ValueError(f"expected an absolute {scheme} URL without credentials or whitespace")
    _ = parsed.port  # Also reject malformed/out-of-range ports.
    return value


def extract_verified(archive, manifest_path, destination):
    entry = json.loads(manifest_path.read_text())[archive.name]
    if digest(archive) != entry["sha256"] or archive.stat().st_size != entry["bytes"]:
        raise ValueError("macOS archive does not match its checksum manifest")
    with tarfile.open(archive) as package:
        members = package.getmembers()
        names = set()
        total = 0
        for member in members:
            path = PurePosixPath(member.name)
            if (path.is_absolute() or ".." in path.parts or not path.parts
                    or member.name in names or not (member.isfile() or member.isdir())
                    or path.parts[0] not in REQUIRED | {APP}
                    or (path.parts[0] != APP and len(path.parts) != 1)):
                raise ValueError(f"unexpected or unsafe archive member: {member.name}")
            names.add(member.name)
            total += member.size
        if total > 2 * 1024**3 or len(members) > 10000:
            raise ValueError("archive exceeds packaging budget")
        if not REQUIRED.issubset(names) or f"{APP}/Contents/MacOS/pab-desktop" not in names:
            raise ValueError("incomplete macOS package")
        package.extractall(destination, filter="data")
    for name in REQUIRED:
        source = SCRIPTS / name
        if source.is_file() and (destination / name).read_bytes() != source.read_bytes().replace(b"\r\n", b"\n"):
            raise ValueError(f"archive has an outdated macOS installation file: {name}")


def verify_binaries(payload, architecture):
    with (payload / APP / "Contents/Info.plist").open("rb") as source:
        if plistlib.load(source).get("CFBundleIdentifier") != "vip.rgaa.pab.desktop":
            raise ValueError("unexpected app bundle identifier")
    for relative in ("pab-executor", "pab-mcp", f"{APP}/Contents/MacOS/pab-desktop"):
        path = payload / relative
        actual = subprocess.check_output(["/usr/bin/lipo", "-archs", str(path)], text=True).split()
        if actual != [architecture]:
            raise ValueError(f"{relative}: expected {architecture}, got {actual}")
        if not path.stat().st_mode & 0o111:
            raise ValueError(f"non-executable binary: {relative}")
    subprocess.run(["/usr/bin/codesign", "--verify", "--deep", "--strict", str(payload / APP)], check=True)
    sys.path.insert(0, str(ROOT / 'scripts'))
    from macos_signing import load_identity, verify, IDENTIFIERS
    fingerprint = load_identity()
    verify(payload / APP, IDENTIFIERS['desktop'], fingerprint)
    for name in ('executor', 'mcp'):
        verify(payload / f'pab-{name}', IDENTIFIERS[name], fingerprint)


def prepare_scripts(destination, arch, control_url, relay_url):
    for name in ("preinstall", "postinstall", "failure-dialog.sh"):
        (destination / name).write_bytes((SCRIPTS / "pkg" / name).read_bytes().replace(b"\r\n", b"\n"))
        (destination / name).chmod(0o755)
        subprocess.run(["/bin/bash", "-n", str(destination / name)], check=True)
    shutil.copyfile(SCRIPTS / "pkg/failure-dialog.applescript", destination / "failure-dialog.applescript")
    values = {"PAB_PKG_ARCH": arch,
              "PAB_CONTROL_URL": control_url, "PAB_RELAY_URL": relay_url}
    (destination / "package.env").write_text("".join(f"{k}={shlex.quote(v)}\n" for k, v in values.items()))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--arch", required=True, choices=("aarch64", "x86_64"))
    parser.add_argument("--control-url", default=CONTROL_URL)
    parser.add_argument("--relay-url", default=RELAY_URL)
    parser.add_argument("--profile", choices=("release", "debug"), default="release")
    parser.add_argument("--packages-dir", type=Path, default=PACKAGES)
    parser.add_argument("--sign", help="Developer ID Installer identity; omit for an unsigned test package")
    args = parser.parse_args()
    if sys.platform != "darwin" or sys.version_info < (3, 12):
        parser.error("run on macOS using Python 3.12 or newer")
    control = checked_url(args.control_url, "wss")
    relay = checked_url(args.relay_url, "https")
    arch = "arm64" if args.arch == "aarch64" else "x86_64"
    packages = args.packages_dir.resolve()
    stem = f"pixels-agent-bridge-macos-{args.arch}-{args.profile}"
    archive = packages / f"{stem}.tar.gz"
    manifest = packages / ("SHA256.json" if args.profile == "release" else "SHA256-debug.json")
    with tempfile.TemporaryDirectory(prefix="pab-pkg-", dir=ROOT / ".build") as temporary:
        stage = Path(temporary)
        scripts = stage / "scripts"
        payload = scripts / "payload"
        payload.mkdir(parents=True)
        extract_verified(archive, manifest, payload)
        verify_binaries(payload, arch)
        with (payload / APP / "Contents/Info.plist").open("rb") as source:
            parse_version(plistlib.load(source)["CFBundleShortVersionString"])
        version = json.loads(manifest.read_text())[archive.name].get('version')
        parse_version(version)
        output = packages / f"{stem}-{version}-setup.pkg"
        prepare_scripts(scripts, arch, control, relay)
        component = stage / "component.pkg"
        subprocess.run(["/usr/bin/pkgbuild", "--nopayload", "--scripts", str(scripts),
                        "--identifier", IDENTIFIER, "--version", version, str(component)], check=True)
        requirements = stage / "requirements.plist"
        requirements.write_bytes(plistlib.dumps({"arch": [arch], "os": ["12.0"]}))
        distribution = stage / "Distribution.xml"
        subprocess.run(["/usr/bin/productbuild", "--synthesize", "--product", str(requirements),
                        "--package", str(component), str(distribution)], check=True)
        tree = ET.parse(distribution)
        root = tree.getroot()
        ET.SubElement(root, "title").text = "Pixels Agent Bridge"
        options = root.find("options")
        if options is None:
            options = ET.SubElement(root, "options")
        options.set("customize", "never")
        options.set("require-scripts", "true")
        ET.SubElement(root, "domains", enable_anywhere="false", enable_currentUserHome="false", enable_localSystem="true")
        resources = stage / "resources"
        resources.mkdir()
        (resources / "Welcome.html").write_text(
            "<html><meta charset='utf-8'><body><h2>Pixels Agent Bridge</h2>"
            f"<p>{arch} · macOS 12+</p><p>升级将覆盖原来的 Pixels Agent Bridge，自动关闭旧版主程序、MCP 和后台服务。"
            "进行中的远程连接和任务会中断，请先完成工作。Codex 等客户端本身不会被关闭；升级后可能需要重新连接 Pixels MCP。"
            "安装器将安装应用、机器后台服务和当前登录用户的桌面辅助进程；需要管理员密码。</p>"
            "<p>Upgrades replace the existing app and automatically stop old PAB processes. "
            "Active connections and tasks will be interrupted. Finish your work first. "
            "Agent hosts stay open; reconnect Pixels MCP after upgrading if needed. "
            "Administrator authorization is required.</p>"
            f"<p>Control: {html.escape(control)}<br>Relay: {html.escape(relay)}</p>"
            "<p>既有数据保留。不自动授予屏幕录制或辅助功能权限。"
            "文件替换失败时会尝试恢复原程序，并显示具体错误。升级前请备份重要数据。</p></body></html>")
        (resources / "Conclusion.html").write_text(
            "<html><meta charset='utf-8'><body><h2>安装完成 / Installed</h2>"
            "<p>从 Applications 打开 Pixels Agent Bridge，在设置中授予屏幕录制和辅助功能权限。"
            "授权后重新登录以重启后台辅助进程。</p>"
            "<p>Open the app from Applications. Grant Screen Recording and Accessibility in System Settings; "
            "log out and back in after changing capture permission.</p>"
            "<p>MCP: /Library/Application Support/PixelsAgentBridge/run-mcp.sh</p></body></html>")
        ET.SubElement(root, "welcome", file="Welcome.html", **{"mime-type": "text/html"})
        ET.SubElement(root, "conclusion", file="Conclusion.html", **{"mime-type": "text/html"})
        tree.write(distribution, encoding="utf-8", xml_declaration=True)
        command = ["/usr/bin/productbuild", "--distribution", str(distribution),
                   "--package-path", str(stage), "--resources", str(resources)]
        if args.sign:
            command.extend(["--sign", args.sign])
        command.append(str(stage / "installer.pkg"))
        subprocess.run(command, check=True)
        shutil.copyfile(stage / "installer.pkg", output)
    metadata = {"file": output.name, "version": version, "bytes": output.stat().st_size, "sha256": digest(output),
                "architecture": arch, "control_url": control,
                "relay_url": relay, "installer_signed": bool(args.sign), "notarized": False}
    (packages / f"SHA256-macos-{args.arch}-setup-{args.profile}.json").write_text(json.dumps(metadata, indent=2) + "\n")
    print(json.dumps(metadata, indent=2))


if __name__ == "__main__":
    main()
