from unittest import TestCase

from smoke.soak_audit import audit


class SoakJournalTests(TestCase):
    def fixture(self):
        report = {'required_seconds': 28800, 'period_seconds': 60, 'devices': [{'code': '123456789'}],
            'status': 'passed', 'elapsed_seconds': 28801, 'samples': 481,
            'cleanup': {'123456789': True, 'owned_mcp_exited': True}}
        rows = [{'device': '123456789', 'cycle': n + 1, 'elapsed_seconds': n * 60,
                 'executor_pid': 42, 'passed': True} for n in range(481)]
        return report, rows

    def test_full_duration_continuous_same_process_can_pass(self):
        report, rows = self.fixture()
        self.assertEqual(audit(report, rows, finished=True)['status'], 'passed')

    def test_process_restart_cannot_hide_a_leak_or_crash_with_fresh_counters(self):
        report, rows = self.fixture()
        rows[-1]['executor_pid'] = 99
        result = audit(report, rows, finished=True)
        self.assertEqual(result['status'], 'failed')
        self.assertIn('123456789:executor_restarted_or_identity_missing', result['failures'])

    def test_partial_time_and_unconfirmed_cleanup_cannot_pass(self):
        report, rows = self.fixture()
        self.assertEqual(audit(report, rows[:20], finished=False)['status'], 'running')
        self.assertEqual(audit(report, rows[:20], finished=True)['status'], 'failed')
        report['cleanup']['owned_mcp_exited'] = False
        self.assertIn('owned_session_cleanup_unconfirmed', audit(report, rows, finished=True)['failures'])

    def test_missing_middle_time_is_not_replaced_by_a_late_success(self):
        report, rows = self.fixture()
        del rows[10:20]
        result = audit(report, rows, finished=True)
        self.assertIn('123456789:observation_gap', result['failures'])
