from pathlib import Path
from tempfile import TemporaryDirectory
from unittest import TestCase
from unittest.mock import patch
import json
import subprocess

from smoke.executor import execute


class NativeCoverageTests(TestCase):
    def result(self, text, code=0):
        return subprocess.CompletedProcess([], code, text, "")

    def test_native_opt_in_executes_every_discovered_ignored_test(self):
        results = [self.result("native::first: test\nnative::second: test\n"),
                   self.result("test result: ok. 4 passed; 0 failed; 2 ignored"),
                   self.result("test result: ok. 1 passed; 0 failed; 0 ignored"),
                   self.result("test result: ok. 1 passed; 0 failed; 0 ignored")]
        with TemporaryDirectory() as temp, patch("smoke.executor.subprocess.run", side_effect=results) as call:
            self.assertEqual(execute(Path("fixture"), Path(temp), True), 0)
            self.assertEqual(json.loads((Path(temp) / "counts.json").read_text()),
                             {"passed": 6, "failed": 0, "skipped": 0})
            self.assertEqual(call.call_args_list[2].args[0][1:4], ["native::first", "--exact", "--ignored"])
            self.assertEqual(call.call_args_list[3].args[0][1], "native::second")

    def test_no_opt_in_keeps_native_tests_blocked(self):
        results = [self.result("native::first: test\n"),
                   self.result("test result: ok. 4 passed; 0 failed; 1 ignored")]
        with TemporaryDirectory() as temp, patch("smoke.executor.subprocess.run", side_effect=results) as call:
            execute(Path("fixture"), Path(temp), False)
            self.assertEqual(call.call_count, 2)
            self.assertEqual(json.loads((Path(temp) / "counts.json").read_text())["skipped"], 1)

    def test_desktop_unit_tests_use_the_selected_user_but_native_tests_keep_service_identity(self):
        results = [self.result("native::first: test\n"), self.result("test result: ok. 1 passed; 0 failed; 0 ignored")]
        unit = self.result("test result: ok. 4 passed; 0 failed; 1 ignored")
        with TemporaryDirectory() as temp, patch("smoke.executor.subprocess.run", side_effect=results) as native, patch("smoke.windows_test_user.run", return_value=unit) as user:
            self.assertEqual(execute(Path("fixture"), Path(temp), True, unit_user=2), 0)
            self.assertEqual(user.call_args.args[2], 2)
            self.assertEqual(native.call_count, 2)
            self.assertIn("--ignored", native.call_args.args[0])
            self.assertEqual(json.loads((Path(temp) / "counts.json").read_text()), {"passed": 5, "failed": 0, "skipped": 0})

    def test_zero_or_missing_native_result_cannot_pass(self):
        results = [self.result("native::first: test\n"),
                   self.result("test result: ok. 4 passed; 0 failed; 1 ignored"),
                   self.result("test result: ok. 0 passed; 0 failed; 0 ignored")]
        with TemporaryDirectory() as temp, patch("smoke.executor.subprocess.run", side_effect=results):
            self.assertEqual(execute(Path("fixture"), Path(temp), True), 1)
            self.assertGreater(json.loads((Path(temp) / "counts.json").read_text())["failed"], 0)
