import hashlib
import json
from pathlib import Path
import subprocess
from tempfile import TemporaryDirectory
from unittest import TestCase
from unittest.mock import patch

from smoke import android_network


class NetworkStateTests(TestCase):
    def test_rejected_pairing_redacts_password_and_restores_original_state(self):
        with TemporaryDirectory() as temp:
            root = Path(temp)
            apk, config, mcp = [root / name for name in ("test.apk", "operator.toml", "mcp")]
            for path in (apk, config, mcp):
                path.write_bytes(b"fixture")
            output = root / "smoke/runs/network"
            state = {"paused": False, "connection_requested": False, "accessibility_ready": True}
            argv = ["test", "--serial", "TEST", "--operator-config", str(config), "--apk", str(apk),
                    "--mcp", str(mcp), "--output", str(output)]
            with patch.object(android_network, "ROOT", root), patch("sys.argv", argv), \
                    patch.object(android_network, "installed_apk_sha256", return_value=hashlib.sha256(b"fixture").hexdigest()), \
                    patch.object(android_network, "normal_status", return_value=state), \
                    patch.object(android_network, "Home") as home, \
                    patch.object(android_network.subprocess, "run", return_value=subprocess.CompletedProcess([], 1, "", "rejected test-secret")):
                home.return_value.revealed_credentials.return_value = ("123456789", "test-secret")
                with self.assertRaisesRegex(RuntimeError, "pairing failed"):
                    android_network.main()
                observed = json.loads((output / "result.json").read_text())
                self.assertTrue(observed["cleanup"])
                self.assertNotEqual(observed["status"], "pass")
                self.assertNotIn("test-secret", observed["pairing_error"])
                home.return_value.click.assert_any_call("disconnect")
