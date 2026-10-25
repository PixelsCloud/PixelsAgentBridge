import unittest
from smoke.remote_desktop_soak import windows


class SoakWindowsTests(unittest.TestCase):
    def test_partial_final_window_and_no_frames_are_retained(self):
        rows = windows([dict(event='opened', seconds=130),
                        dict(event='presented', elapsed_us=5_000_000),
                        dict(event='presented', elapsed_us=6_000_000)])
        self.assertEqual([(r['start_s'], r['end_s']) for r in rows], [(5, 65), (65, 125), (125, 130)])
        self.assertEqual(rows[0]['fps'], 2 / 60)
        self.assertEqual(rows[1]['fps'], 0)
        self.assertEqual(rows[2]['fps'], 0)

    def test_boundary_stall_is_not_lost(self):
        rows = windows([dict(event='opened', seconds=130),
                        dict(event='presented', elapsed_us=64_000_000),
                        dict(event='presented', elapsed_us=66_000_000)])
        self.assertEqual(rows[1]['max_gap_ms'], 2000)
        self.assertEqual(rows[1]['gaps_over_1s'], 1)

    def test_udp_timeout_and_success_stay_distinct(self):
        rows = windows([dict(event='opened', seconds=70),
                        dict(event='network_probe', elapsed_us=6_000_000, rtt_us=300),
                        dict(event='network_probe', elapsed_us=7_000_000, rtt_us=None)])
        self.assertEqual(rows[0]['udp_samples'], 2)
        self.assertEqual(rows[0]['udp_completed'], 1)
        self.assertEqual(rows[0]['udp_rtt_p95_ms'], .3)

    def test_end_boundary_does_not_inflate_last_window(self):
        rows = windows([dict(event='opened', seconds=65),
                        dict(event='presented', elapsed_us=65_000_000)])
        self.assertEqual(rows[0]['frames'], 0)


if __name__ == '__main__':
    unittest.main()
