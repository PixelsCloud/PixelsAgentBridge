import sys
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "scripts"))
import build_android
from build_profile import apply_release_preset


class ReleaseProfileTests(unittest.TestCase):
    def test_daily_and_publication_have_distinct_effective_parameters(self):
        env = apply_release_preset({})
        self.assertEqual(env["CARGO_PROFILE_RELEASE_OPT_LEVEL"], "2")
        self.assertEqual(env["CARGO_PROFILE_RELEASE_LTO"], "false")
        self.assertEqual(env["CARGO_INCREMENTAL"], "1")
        apply_release_preset(env, publication=True)
        self.assertEqual(env["CARGO_PROFILE_RELEASE_OPT_LEVEL"], "3")
        self.assertEqual(env["CARGO_PROFILE_RELEASE_LTO"], "thin")
        self.assertEqual(env["CARGO_PROFILE_RELEASE_CODEGEN_UNITS"], "1")
        self.assertEqual(env["CARGO_PROFILE_RELEASE_INCREMENTAL"], "false")
        self.assertEqual(env["CARGO_INCREMENTAL"], "0")
        # A nested build must retain its parent's selected publication preset.
        self.assertEqual(apply_release_preset(env)["PAB_BUILD_PRESET"], "optimized")

    def test_fast_cannot_override_publication(self):
        with self.assertRaises(ValueError):
            apply_release_preset({}, "fast", publication=True)

    def test_android_native_defaults_to_release_and_rejects_debug(self):
        with patch.object(build_android, "run") as run, patch.object(build_android.shutil, "copy2") as copy:
            build_android.native("arm64-v8a", {}, False)
            self.assertIn("--release", run.call_args.args[0])
            self.assertEqual(copy.call_args.args[0].parent.name, "release")
            run.reset_mock()
            with self.assertRaises(ValueError):
                build_android.native("arm64-v8a", {}, False, "debug")
            run.assert_not_called()

    def test_android_check_uses_the_same_release_configuration(self):
        with patch.object(build_android, "run") as run:
            build_android.native("arm64-v8a", {}, True)
            self.assertIn("--release", run.call_args.args[0])


if __name__ == "__main__":
    unittest.main()
