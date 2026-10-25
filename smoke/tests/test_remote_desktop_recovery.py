import unittest
from smoke.remote_desktop_recovery import audit


class RecoveryEvidenceTests(unittest.TestCase):
    def fixture(self):
        # A measured blackout followed by a full 20 seconds at 30 FPS.
        events = [dict(event='presented', elapsed_us=9_900_000),
                  dict(event='presented', elapsed_us=10_500_000)]
        events += [dict(event='presented', elapsed_us=11_000_000+i*1_000_000/30) for i in range(602)]
        events += [dict(event='frame_trace', elapsed_us=at, trace=dict(network=dict(path_id='0', remote_addr='ip:test', lost_packets=lost))) for at, lost in [(9_000_000, 0), (11_500_000, 2)]]
        injection = dict(before_receiver_elapsed_s=10, after_receiver_elapsed_s=11,
                         requested_ms=500, result=dict(applied=True, rules_removed=True))
        return events, [injection]

    def test_proven_interruption_and_full_recovery_can_pass(self):
        events, injections = self.fixture()
        result = audit(events, injections, expected_faults=1)
        self.assertEqual(result['status'], 'passed', result)
        self.assertEqual(result['faults'][0]['sender_loss_delta'], 2)

    def test_rule_creation_without_transport_impact_cannot_pass(self):
        events, injections = self.fixture()
        events = [e for e in events if e['event'] != 'frame_trace']
        self.assertEqual(audit(events, injections, 1)['status'], 'failed')

    def test_short_or_still_frozen_observation_cannot_pass(self):
        events, injections = self.fixture()
        events = [e for e in events if e['elapsed_us'] < 15_000_000]
        self.assertEqual(audit(events, injections, 1)['status'], 'failed')

    def test_missing_fault_or_cleanup_is_not_a_pass(self):
        events, injections = self.fixture()
        self.assertEqual(audit(events, injections, 3)['status'], 'failed')
        injections[0]['result']['rules_removed'] = False
        self.assertEqual(audit(events, injections, 1)['status'], 'failed')


if __name__ == '__main__':
    unittest.main()
