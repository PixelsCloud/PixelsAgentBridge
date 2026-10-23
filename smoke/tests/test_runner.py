from datetime import datetime, timedelta, timezone
import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

from smoke import run
from smoke.adapters import counts_for, result_status, command_for


class ResultTests(unittest.TestCase):
    def test_success_requires_nonempty_results_and_no_skips(self):
        for counts, expected in [({"passed": 1, "failed": 0, "skipped": 0}, "pass"),
                                  ({"passed": 0, "failed": 0, "skipped": 0}, "blocked"),
                                  ({"passed": 8, "failed": 0, "skipped": 1}, "blocked"),
                                  ({"passed": 8, "failed": 1, "skipped": 0}, "fail"),
                                  ({"passed": -1, "failed": 0, "skipped": 0}, "unconfirmed")]:
            self.assertEqual(result_status(0, counts), expected)
        self.assertEqual(result_status(1, {"passed": 8, "failed": 0, "skipped": 0}), "fail")

    def test_playwright_flaky_and_global_errors_fail(self):
        with TemporaryDirectory() as temp:
            path = Path(temp)
            (path / "playwright.json").write_text(json.dumps({"stats": {
                "expected": 4, "unexpected": 0, "flaky": 1, "skipped": 0}, "errors": [{}]}))
            self.assertEqual(counts_for("playwright", path, ""), {"passed": 4, "failed": 2, "skipped": 0})

    def test_libtest_counts_multiple_binaries_without_counting_filtered(self):
        output = "test result: ok. 4 passed; 0 failed; 1 ignored; 5 filtered out\n"
        output += "test result: ok. 2 passed; 0 failed; 0 ignored; 0 filtered out\n"
        self.assertEqual(counts_for("libtest", Path("."), output), {"passed": 6, "failed": 0, "skipped": 1})

    def test_catalog_is_complete_and_has_platform_scope(self):
        cases = run.catalog()
        self.assertGreaterEqual(len(cases), 50)
        self.assertTrue(all(c["prerequisite"] and c["cleanup"] for c in cases))
        for target in run.TARGETS:
            self.assertTrue(run.selected(target, "quick"))
            self.assertGreater(len(run.selected(target, "release")), len(run.selected(target, "quick")))

    def test_tool_manifest_points_to_distributed_sources_and_real_cases(self):
        definitions = json.loads((run.HOME / "tool-coverage.json").read_text(encoding="utf-8"))["tools"]
        cases = {c["id"] for c in run.catalog()}
        for name, definition in definitions.items():
            self.assertIn(definition["live_suite"], cases, name)
            self.assertTrue(definition["existing_test_sources"], name)
            for source in definition["existing_test_sources"]:
                self.assertTrue((run.ROOT / source).is_file(), source)
            self.assertNotIn("live_status", definition)

    def test_production_url_rejected_before_browser_starts(self):
        with patch("smoke.adapters.shutil.which", return_value="node"):
            with self.assertRaisesRegex(ValueError, "loopback"):
                command_for("web-live", Path("."), {"web": {"isolated": True, "url": "https://production.example"}})

    def test_excess_build_jobs_are_rejected(self):
        with self.assertRaisesRegex(ValueError, "jobs"):
            command_for("python", Path("."), {"execution": {"jobs": 11}})

    def test_redaction(self):
        value = run.redact('password="super-secret" Authorization: Bearer abc123\nhttps://user:pass@example.com')
        self.assertNotIn("super-secret", value)
        self.assertNotIn("abc123", value)
        self.assertNotIn("user:pass", value)


class GateTests(unittest.TestCase):
    def setUp(self):
        self.temp = TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name)
        self.path = self.home / "runs/id/report.json"
        self.path.parent.mkdir(parents=True)
        (self.home / "cases.toml").write_text("fixture")
        self.artifact = self.home / "candidate.bin"
        self.artifact.write_bytes(b"candidate")
        self.case = {"id": "MAN-01", "steps": ["observe"], "expected": ["changed"]}
        self.source = {"commit": "abc", "sha256": "source", "dirty": False}
        self.observation = {"run_id": "id", "case": "MAN-01", "artifact_sha256": run.digest(self.artifact),
            "operator": "tester", "environment": "owned VM", "status": "pass", "cleanup_verified": True,
            "steps": [{"step": 1, "passed": True, "observed": "Fixture received the expected operation once"}]}
        evidence = self.path.parent / "observation.json"
        evidence.write_text(json.dumps(self.observation))
        self.report = {"schema_version": 1, "run_id": "id", "profile": "release", "target": "windows", "arch": "x86_64",
            "version": "1.0.0", "source": self.source, "catalog_sha256": run.digest(self.home / "cases.toml"),
            "artifact": {"sha256": run.digest(self.artifact), "bytes": self.artifact.stat().st_size},
            "unresolved_resources": [], "cases": [{"id": "MAN-01", "status": "pass", "attempts": [{
                "kind": "manual", "status": "pass", "finished_at": run.now(), "evidence": "observation.json",
                "evidence_sha256": run.digest(evidence)}]}]}
        for target, value in [("HOME", self.home), ("source_state", lambda: dict(self.source)),
                              ("selected", lambda *args: [self.case])]:
            patcher = patch.object(run, target, value)
            patcher.start(); self.addCleanup(patcher.stop)

    def validate(self):
        self.path.write_text(json.dumps(self.report))
        return run.validate_release_report(self.path, platform="windows", arch="x86_64", artifact=self.artifact,
                                           version="1.0.0", commit="abc")

    def test_complete_matching_report_passes(self):
        self.validate()

    def test_missing_and_duplicate_cases_fail(self):
        original = self.report["cases"]
        for cases in ([], original * 2):
            self.report["cases"] = cases
            with self.assertRaisesRegex(ValueError, "Missing"):
                self.validate()

    def test_skipped_blocked_not_run_or_unconfirmed_cannot_pass(self):
        for status in ("skip", "blocked", "not_run", "unconfirmed", "fail"):
            self.report["cases"][0]["status"] = status
            with self.assertRaisesRegex(ValueError, "not passed"):
                self.validate()

    def test_modified_artifact_fails(self):
        self.artifact.write_bytes(b"other candidate")
        with self.assertRaisesRegex(ValueError, "artifact"):
            self.validate()

    def test_modified_or_missing_evidence_fails(self):
        evidence = self.path.parent / "observation.json"
        evidence.write_text("edited")
        with self.assertRaisesRegex(ValueError, "Evidence"):
            self.validate()
        evidence.unlink()
        with self.assertRaisesRegex(ValueError, "Evidence"):
            self.validate()

    def test_stale_and_future_evidence_fail(self):
        for delta in (timedelta(days=-4), timedelta(days=1)):
            self.report["cases"][0]["attempts"][0]["finished_at"] = (datetime.now(timezone.utc) + delta).isoformat()
            with self.assertRaisesRegex(ValueError, "older"):
                self.validate()

    def test_manual_cannot_satisfy_automated_case(self):
        self.case["adapter"] = "python"
        with self.assertRaisesRegex(ValueError, "Automated"):
            self.validate()

    def test_incomplete_observation_rejected(self):
        self.observation["steps"] = []
        with self.assertRaisesRegex(ValueError, "Every manual"):
            run.validate_observation(self.observation, self.case, self.report)

    def test_wrong_scope_and_quick_reports_rejected(self):
        for field, value in (("target", "macos"), ("version", "2.0.0"), ("arch", "aarch64"), ("profile", "quick")):
            before = self.report[field]; self.report[field] = value
            with self.assertRaises(ValueError):
                self.validate()
            self.report[field] = before

    def test_source_changes_dirty_tree_and_cleanup_block(self):
        for change in ("source", "dirty", "cleanup"):
            if change == "source":
                self.report["source"] = {**self.source, "sha256": "old"}
            elif change == "dirty":
                self.source["dirty"] = True; self.report["source"] = self.source
            else:
                self.source["dirty"] = False; self.report["unresolved_resources"] = [{"pid": 42}]
            with self.assertRaises(ValueError):
                self.validate()
