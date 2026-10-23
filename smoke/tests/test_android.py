"""The smoke harness must restore a phone even when a remote check fails."""
import hashlib
import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

from smoke import android
from smoke.adapters import command_for


class AndroidSafety(unittest.TestCase):
    def test_device_requires_explicit_opt_in(self):
        with self.assertRaisesRegex(ValueError, "Explicitly"):
            command_for("android-relay", Path("."), {"android": {"serial": "test"}})

    def test_output_cannot_escape_private_run_directory(self):
        with self.assertRaisesRegex(ValueError, "smoke/runs"):
            android.run("test", "missing", "missing", "missing", android.ROOT)

    def test_artifact_mismatch_never_changes_phone(self):
        with TemporaryDirectory() as temp:
            root = Path(temp)
            apk = root / "apk"
            apk.write_bytes(b"candidate")
            with patch.object(android, "ROOT", root), patch.object(android, "installed_apk_sha256", return_value="other"), patch.object(android, "Home") as home:
                with self.assertRaisesRegex(ValueError, "differs"):
                    android.run("test", apk, apk, apk, root / "smoke/runs/test")
                home.assert_not_called()

    def test_failure_restores_connection_and_cannot_report_pass(self):
        with TemporaryDirectory() as temp:
            root = Path(temp)
            apk = root / "apk"
            apk.write_bytes(b"candidate")
            output = root / "smoke/runs/test"
            before = {"connection_requested": True, "remote_ready": True, "paused": False, "accessibility_ready": True}
            off = dict(before, connection_requested=False, remote_ready=False)
            with patch.object(android, "ROOT", root), patch.object(android, "installed_apk_sha256", return_value=hashlib.sha256(b"candidate").hexdigest()), patch.object(android, "normal_status", side_effect=[before, off, off, before]), patch.object(android, "Home") as home, patch.object(android, "adb"), patch.object(android.subprocess, "run", side_effect=RuntimeError("fixture failed")):
                with self.assertRaisesRegex(RuntimeError, "fixture failed"):
                    android.run("test", apk, apk, apk, output)
                home.return_value.start.assert_called_once()
            result = json.loads((output / "android-result.json").read_text())
            self.assertTrue(result["cleanup"])
            self.assertEqual(result["status"], "fail")
            self.assertEqual(json.loads((output / "counts.json").read_text())["failed"], 1)


if __name__ == "__main__":
    unittest.main()
