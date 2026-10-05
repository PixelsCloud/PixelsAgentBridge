"""No installation occurs: tests validate inputs and optionally expand a test pkg."""
import ast
import io
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
import xml.etree.ElementTree as ET

import build_macos_pkg as pkg


class Installer(unittest.TestCase):
    def test_defaults_match_windows_builder(self):
        tree = ast.parse((pkg.ROOT / "packaging/desktop/build_nsis.py").read_text())
        defaults = {}
        for node in ast.walk(tree):
            if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute) and node.func.attr == "add_argument":
                if node.args and isinstance(node.args[0], ast.Constant):
                    for keyword in node.keywords:
                        if keyword.arg == "default":
                            defaults[node.args[0].value] = ast.literal_eval(keyword.value)
        self.assertEqual(pkg.CONTROL_URL, defaults["--control-url"])
        self.assertEqual(pkg.RELAY_URL, defaults["--relay-url"])

    def test_url_validation(self):
        self.assertEqual(pkg.checked_url(pkg.CONTROL_URL, "wss"), pkg.CONTROL_URL)
        for value in ("http://example.test", "wss://", "wss://a\n", "wss://a b", "wss://a:99999", "wss://u:p@a", "wss://a/#fragment"):
            with self.subTest(value=value), self.assertRaises(ValueError):
                pkg.checked_url(value, "wss")

    def test_generated_configuration_is_shell_quoted(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary)
            value = "wss://example.test/control?a='x'&b=$(false)"
            pkg.prepare_scripts(path, "arm64", value, pkg.RELAY_URL)
            self.assertNotIn("DEPLOYMENT", (path / "package.env").read_text())
            result = subprocess.check_output(["/bin/bash", "-c", 'source "$1"; printf "%s" "$PAB_CONTROL_URL"', "test", str(path / "package.env")], text=True)
            self.assertEqual(result, value)

    def test_unsafe_incomplete_and_tampered_archives_are_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / "fixture.tar.gz"
            manifest = root / "SHA256.json"
            for name in ("../escape", "/absolute", "unknown", "pab-mcp"):
                with self.subTest(name=name):
                    with tarfile.open(archive, "w:gz") as output:
                        member = tarfile.TarInfo(name)
                        member.size = 1
                        output.addfile(member, io.BytesIO(b"x"))
                    manifest.write_text(json.dumps({archive.name: {"sha256": pkg.digest(archive), "bytes": archive.stat().st_size}}))
                    with self.assertRaises(ValueError):
                        pkg.extract_verified(archive, manifest, root / "unpacked")
                    self.assertFalse((root / "unpacked").exists())
            manifest.write_text(json.dumps({archive.name: {"sha256": "wrong", "bytes": archive.stat().st_size}}))
            with self.assertRaises(ValueError):
                pkg.extract_verified(archive, manifest, root / "unpacked")

    @unittest.skipUnless(os.environ.get("PAB_PKG_TEST_ARCHIVE"), "set PAB_PKG_TEST_ARCHIVE to build/expand a temporary fixture installer")
    def test_real_product_build_and_expand_without_installing(self):
        archive = Path(os.environ["PAB_PKG_TEST_ARCHIVE"]).resolve()
        arch = "aarch64" if "aarch64" in archive.name else "x86_64"
        profile = "debug" if "debug" in archive.name else "release"
        # Build/expand only; installation scripts are never executed.
        with tempfile.TemporaryDirectory(prefix="pab-pkg-test-", dir=pkg.ROOT / ".build") as temporary:
            root = Path(temporary)
            shutil.copyfile(archive, root / archive.name)
            manifest_name = "SHA256-debug.json" if profile == "debug" else "SHA256.json"
            source_manifest = json.loads((archive.parent / manifest_name).read_text())
            (root / manifest_name).write_text(json.dumps({archive.name: source_manifest[archive.name]}))
            subprocess.run([sys.executable, str(pkg.ROOT / "packaging/desktop/build_macos_pkg.py"),
                            "--arch", arch, "--profile", profile,
                            "--packages-dir", str(root)], check=True, capture_output=True)
            installers = list(root.glob("*-setup.pkg"))
            self.assertEqual(len(installers), 1)
            expanded = root / "expanded"
            subprocess.run(["/usr/sbin/pkgutil", "--expand-full", str(installers[0]), str(expanded)], check=True)
            distribution = ET.parse(expanded / "Distribution").getroot()
            self.assertEqual(distribution.find("domains").attrib["enable_anywhere"], "false")
            self.assertEqual(distribution.find("options").attrib["require-scripts"], "true")
            configs = list(expanded.rglob("package.env"))
            self.assertEqual(len(configs), 1)
            self.assertNotIn("DEPLOYMENT", configs[0].read_text())
            self.assertIn(pkg.CONTROL_URL, configs[0].read_text())
            self.assertTrue((configs[0].parent / "payload" / pkg.APP / "Contents/MacOS/pab-desktop").is_file())
            pkg.verify_binaries(configs[0].parent / "payload", "arm64" if arch == "aarch64" else "x86_64")
            app_payload = configs[0].parent / "payload" / pkg.APP
            self.assertEqual((app_payload / "Contents/MacOS/pab-desktop").stat().st_mode & 0o777, 0o755)
            self.assertEqual((app_payload / "Contents/Info.plist").stat().st_mode & 0o777, 0o644)
            with (configs[0].parent / "payload" / pkg.APP / "Contents/Info.plist").open("rb") as source:
                built_version = plistlib.load(source)["CFBundleShortVersionString"]
            package_info = ET.parse(next(expanded.rglob("PackageInfo"))).getroot()
            self.assertEqual(package_info.attrib["version"], built_version)
            metadata = json.loads(next(root.glob("SHA256-macos-*-setup-*.json")).read_text())
            self.assertEqual(pkg.digest(installers[0]), metadata["sha256"])
            self.assertEqual(metadata['version'], built_version)
            self.assertFalse(metadata["installer_signed"])
            self.assertNotIn("deployment_id", metadata)


if __name__ == "__main__":
    unittest.main()
