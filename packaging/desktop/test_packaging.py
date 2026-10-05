"""Archive regression checks; fixture binaries are never executed."""
import hashlib
import json
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
import zipfile


SCRIPTS = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPTS.parents[1] / "scripts"))
from build_version import record_artifacts, macos_artifacts


class Packages(unittest.TestCase):
    def test_platform_archives_keep_their_own_scripts_and_components(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            # Keep synthetic provenance out of the checkout's real build records.
            package_scripts = root / "packaging/desktop"
            package_scripts.mkdir(parents=True)
            shutil.copyfile(SCRIPTS / "build.py", package_scripts / "build.py")
            for name in ("windows", "unix", "macos"):
                shutil.copytree(SCRIPTS / name, package_scripts / name)
            (root / "scripts").mkdir()
            shutil.copyfile(SCRIPTS.parents[1] / "scripts/build_version.py", root / "scripts/build_version.py")
            binaries = root / "bin"
            binaries.mkdir()
            for name in ("pab-mcp", "pab-executor", "pab-desktop"):
                (binaries / name).write_bytes(b"fixture")
                (binaries / (name + ".exe")).write_bytes(b"fixture")
            app = root / "Pixels Agent Bridge.app"
            executable = app / "Contents/MacOS/pab-desktop"
            executable.parent.mkdir(parents=True)
            executable.write_bytes(b"fixture")
            executable.chmod(0o700)
            (app / "Contents/Info.plist").write_bytes(plistlib.dumps({"CFBundleShortVersionString": "1.2.0"}))
            (app / "Contents/Info.plist").chmod(0o600)
            executable.parent.chmod(0o700)
            for platform in ("windows", "linux", "macos"):
                with self.subTest(platform=platform):
                    out = root / platform
                    if platform != "macos":
                        suffix = ".exe" if platform == "windows" else ""
                        files = {name + suffix: binaries / (name + suffix) for name in ("pab-executor", "pab-mcp", "pab-desktop")}
                        record_artifacts(root, "desktop" if platform == "windows" else platform, "debug", "1.2.0", files)
                    else:
                        record_artifacts(root, 'macos-aarch64', 'debug', '1.2.0', macos_artifacts(binaries, app))
                    subprocess.run([
                        sys.executable, str(package_scripts / "build.py"), "--platform", platform,
                        "--profile", "debug", "--output-dir", str(out),
                        "--windows-bin-dir", str(binaries),
                        "--windows-desktop-bin", str(binaries / "pab-desktop.exe"),
                        "--linux-bin-dir", str(binaries), "--macos-bin-dir", str(binaries),
                        "--macos-app", str(app), "--macos-arch", "aarch64",
                    ], check=True, capture_output=True)
                    manifest = json.loads((out / "SHA256-debug.json").read_text())
                    self.assertEqual(len(manifest), 1)
                    name, info = next(iter(manifest.items()))
                    self.assertEqual(info["version"], "1.2.0")
                    archive = out / name
                    self.assertEqual(hashlib.sha256(archive.read_bytes()).hexdigest(), info["sha256"])
                    self.assertEqual(archive.stat().st_size, info["bytes"])
                    if platform == "windows":
                        with zipfile.ZipFile(archive) as package:
                            self.assertIn("pab-desktop.exe", package.namelist())
                            self.assertEqual(package.read("install.ps1"), (SCRIPTS / "windows/install.ps1").read_bytes())
                            self.assertNotIn("install.sh", package.namelist())
                    else:
                        with tarfile.open(archive) as package:
                            script_dir = "macos" if platform == "macos" else "unix"
                            self.assertEqual(package.extractfile("install.sh").read(), (SCRIPTS / script_dir / "install.sh").read_bytes().replace(b"\r\n", b"\n"))
                            self.assertEqual(package.getmember("install.sh").mode, 0o755)
                            if platform == "macos":
                                self.assertIn("Pixels Agent Bridge.app/Contents/MacOS/pab-desktop", package.getnames())
                                self.assertEqual(package.getmember("Pixels Agent Bridge.app/Contents/MacOS/pab-desktop").mode, 0o755)
                                self.assertEqual(package.getmember("Pixels Agent Bridge.app/Contents/MacOS").mode, 0o755)
                                self.assertEqual(package.getmember("Pixels Agent Bridge.app/Contents/Info.plist").mode, 0o644)
                                self.assertNotIn("pab-desktop", package.getnames())
                                self.assertEqual(package.getmember("com.pixelsagentbridge.executor.plist").mode, 0o644)
                            else:
                                self.assertIn("pab-desktop", package.getnames())
                                self.assertNotIn("com.pixelsagentbridge.executor.plist", package.getnames())


if __name__ == "__main__":
    unittest.main()
