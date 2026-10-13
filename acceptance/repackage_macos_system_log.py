"""Repackage the already-built macOS app after an Executor-only log fix.

Run in the isolated macOS source snapshot with CARGO_TARGET_DIR set to the
shared build target. The previous app, MCP, worker and runtimes must exist.
"""

from pathlib import Path
import os
import subprocess
import sys

root = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(root / "scripts"))
from build_version import macos_artifacts, record_artifacts
from macos_signing import IDENTIFIERS, sign

target = Path(os.environ["CARGO_TARGET_DIR"]) / "aarch64-apple-darwin" / "release"
app = target / "bundle" / "macos" / "Pixels Agent Bridge.app"
assert app.is_dir(), app
sign(target / "pab-executor", IDENTIFIERS["executor"])
sign(target / "pab-mcp", IDENTIFIERS["mcp"])
record_artifacts(root, "macos-aarch64", "release", "1.2.94", macos_artifacts(target, app))
subprocess.run([
    sys.executable, str(root / "packaging/desktop/build.py"),
    "--platform", "macos", "--profile", "release", "--macos-arch", "aarch64",
    "--macos-bin-dir", str(target), "--macos-app", str(app),
], cwd=root, check=True)
subprocess.run([
    sys.executable, str(root / "packaging/desktop/build_macos_pkg.py"),
    "--arch", "aarch64", "--profile", "release",
], cwd=root, check=True)
