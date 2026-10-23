"""Publishing cannot contact upload/finalize endpoints before the smoke gate succeeds."""
from contextlib import ExitStack
import importlib.util
import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("release_smoke_test", ROOT / "scripts/release.py")
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class ReleaseHook(unittest.TestCase):
    def test_rejected_report_prevents_upload_and_finalize_even_with_allow_dirty(self):
        self.exercise(prepare=False)

    def test_prepare_only_never_publishes(self):
        self.exercise(prepare=True)

    def exercise(self, prepare):
        with TemporaryDirectory() as directory, ExitStack() as stack:
            root = Path(directory)
            (root / ".build").mkdir()
            state = {"platform": "windows", "arch": "x86_64", "commit": "fixture-commit",
                     "build_id": "fixture-build", "phase": "built", "version": "1.0.0"}
            (root / ".build/release-windows-x86_64.json").write_text(json.dumps(state))
            # Ephemeral test key only, never a repository or deployed signing key.
            key = root / "test-key"
            key.write_bytes(bytes(range(32)))
            artifact = root / "fixture.exe"
            artifact.write_bytes(b"candidate")
            args = ["release.py", "windows", "--resume", "--allow-dirty", "--site", "https://smoke.invalid",
                    "--notes-zh", "fixture", "--notes-en", "fixture", "--smoke-report", str(root / "report.json")]
            if prepare:
                args.append("--prepare-only")
            stack.enter_context(patch.object(release, "ROOT", root))
            stack.enter_context(patch.object(release.sys, "argv", args))
            stack.enter_context(patch.dict(release.os.environ, {"PAB_RELEASE_API_TOKEN": "synthetic-test-token", "PAB_RELEASE_PRIVATE_KEY_FILE": str(key)}))
            stack.enter_context(patch.object(release.subprocess, "check_output", return_value="fixture-commit"))
            stack.enter_context(patch.object(release, "package_info", return_value=(artifact, 9, "synthetic-hash")))
            gate = stack.enter_context(patch("smoke.run.validate_release_report", side_effect=ValueError("incomplete smoke")))
            api = stack.enter_context(patch.object(release, "request_json"))
            upload = stack.enter_context(patch.object(release, "upload"))
            if prepare:
                release.main()
                gate.assert_not_called()
            else:
                with self.assertRaisesRegex(ValueError, "incomplete smoke"):
                    release.main()
                gate.assert_called_once()
            api.assert_not_called()
            upload.assert_not_called()
            self.assertEqual(json.loads((root / ".build/release-windows-x86_64.json").read_text()), state)


if __name__ == "__main__":
    unittest.main()
