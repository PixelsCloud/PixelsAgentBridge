"""Reporting must not turn ignored, missing or unconfirmed tests into passes."""
import json
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest import TestCase, main
from acceptance import save, test_results


class ReportTests(TestCase):
    def test_only_result_lines_are_collected(self):
        output = 'test a::ok ... ok\ntest b ... ignored, native only\ntest c ... FAILED\nsecret fixture\ntest result: ok. 1 passed\n'
        self.assertEqual(test_results(output), [
            {'case': 'a::ok', 'status': 'pass'}, {'case': 'b', 'status': 'skip'}, {'case': 'c', 'status': 'fail'}])
        self.assertEqual(test_results('Finished compilation, no tests executed'), [])

    def test_unconfirmed_and_cleanup_evidence_survive_report_write(self):
        report = {'run_id': 'fixture', 'suite': 'isolated', 'commit': 'abc', 'version': '1.2.15',
                  'cases': [{'case': 'interrupted', 'status': 'unconfirmed', 'evidence': 'inspect original ID'}],
                  'uncleaned_resources': ['fixture process 123']}
        with TemporaryDirectory() as path:
            save(Path(path), report)
            self.assertEqual(json.loads((Path(path) / 'report.json').read_text()), report)
            self.assertIn('unconfirmed', (Path(path) / 'report.md').read_text())
            self.assertFalse((Path(path) / 'report.json.tmp').exists())


if __name__ == '__main__':
    main()
