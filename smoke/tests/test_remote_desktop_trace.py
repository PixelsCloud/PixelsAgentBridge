import json
from pathlib import Path
import tempfile
import unittest
from smoke.remote_desktop_trace import analyze


class TraceTests(unittest.TestCase):
    def events(self):
        frame = lambda i, at: dict(frame_id=i, elapsed_us=at, capture_time_us=at,
            credit_us=0, budget_us=0, source_us=33000, capture=None, send_us=20, ack_us=10000)
        return [dict(event='opened', debug_assertions=False),
            dict(event='received', frame=1, elapsed_us=5_000_000, read_us=33000),
            dict(event='submitted', frame=1, elapsed_us=5_001_000, submit_us=1000),
            dict(event='presented', frame=1, elapsed_us=5_010_000),
            dict(event='received', frame=2, elapsed_us=5_500_000, read_us=499000),
            dict(event='submitted', frame=2, elapsed_us=5_501_000, submit_us=1000),
            dict(event='presented', frame=2, elapsed_us=5_510_000),
            dict(event='frame_trace', elapsed_us=5_000_000, trace=dict(frames=[frame(1, 1_000_000)],
                slow_frames=[], network=dict(path_id='same', lost_packets=0))),
            dict(event='frame_trace', elapsed_us=5_550_000, trace=dict(frames=[frame(2, 1_033_000)],
                slow_frames=[], network=dict(path_id='same', lost_packets=3))),
            dict(event='closed')]

    def analyze(self, events):
        events.append(dict(event='telemetry', dropped_events=0, written_events=len(events)))
        with tempfile.TemporaryDirectory() as d:
            p = Path(d)/'frames.jsonl'
            p.write_text('\n'.join(json.dumps(e) for e in events))
            return analyze(p)

    def test_compares_intervals_without_subtracting_different_clock_origins(self):
        result = self.analyze(self.events())
        self.assertTrue(result['trace_complete'])
        row = result['gaps'][0]
        self.assertEqual(row['segment'], 'transport_or_receiver_read_wait')
        self.assertEqual(row['producer_interval_ms'], 33)
        self.assertEqual(row['receive_interval_ms'], 500)
        self.assertEqual(row['sender_loss_detected_near_gap'], 3)

    def test_receiver_submit_wait_is_not_misattributed_to_transport(self):
        events = self.events()
        events[2]['submit_us'] = 480000
        self.assertEqual(self.analyze(events)['gaps'][0]['segment'], 'receiver_submit_or_decoder_backpressure')

    def test_source_wait_is_attributed_to_recorded_helper_stage(self):
        events = self.events()
        f = events[8]['trace']['frames'][0]
        f.update(elapsed_us=1_500_000, source_us=500000,
            capture=dict(wait_us=1000, encode_us=19000, detail=dict(native_total_us=20000,
                worker_roundtrip_us=30000, helper_roundtrip_us=499000)))
        self.assertEqual(self.analyze(events)['gaps'][0]['segment'], 'helper_ipc')

    def test_codec_budget_reconfiguration_and_keyframe_are_recorded(self):
        events = self.events()
        events[4]['keyframe'] = True
        events[8]['trace']['frames'][0].update(elapsed_us=1_500_000, budget_us=460000)
        row = self.analyze(events)['gaps'][0]
        self.assertEqual(row['segment'], 'encoder_reconfiguration')
        self.assertTrue(row['next_frame_keyframe'])

    def test_missing_trace_or_close_cannot_be_counted_complete(self):
        events = self.events()
        events[8]['trace']['frames'] = []
        self.assertFalse(self.analyze(events)['trace_complete'])
        events = self.events()[:-1]
        self.assertFalse(self.analyze(events)['trace_complete'])

    def test_control_queue_is_separated_from_transport_and_reply_roundtrip(self):
        events = self.events()
        events[8]['trace']['frames'][0].update(elapsed_us=1_500_000, credit_us=450000)
        events[8]['receiver_control'] = [dict(generation=1, frame_id=1, queue_us=440000,
            write_us=100, reply_us=15000, pending_before=8)]
        events[8]['receiver_network'] = dict(remote_addr='10.23.0.10:1234', local_addr='test')
        result = self.analyze(events)
        row = result['gaps'][0]
        self.assertEqual(row['segment'], 'receiver_control_queue')
        self.assertEqual(row['ack_queue_before_ms'], 440)
        self.assertEqual(row['control_reply_before_ms'], 15)
        self.assertEqual(result['selected_endpoints'][0]['remote_addr'], '10.23.0.10:1234')

    def test_udp_and_scheduler_delays_use_overlapping_receiver_intervals(self):
        events = self.events()
        events += [dict(event='network_probe', send_us=5_000_000, elapsed_us=5_180_000,
                        status='ok', rtt_us=180000, server_work_us=7),
                   dict(event='network_probe', send_us=5_400_000, elapsed_us=5_600_000,
                        status='timeout', rtt_us=None, server_work_us=None),
                   dict(event='network_probe', send_us=6_000_000, elapsed_us=6_001_000,
                        status='ok', rtt_us=1000, server_work_us=2),
                   dict(event='schedule_probe', scheduled_us=5_450_000, elapsed_us=5_550_000, late_us=100000),
                   dict(event='runtime_probe', elapsed_us=5_450_000, late_us=6000)]
        row = self.analyze(events)['gaps'][0]['concurrent_probes']
        self.assertEqual(row['samples'], 2)
        self.assertEqual(row['failures'], 1)
        self.assertEqual(row['max_rtt_ms'], 180)
        self.assertEqual(row['max_thread_late_ms'], 100)
        self.assertEqual(row['max_runtime_late_ms'], 6)

    def test_responder_errors_join_by_sequence_despite_different_clock_origin(self):
        events = self.events()+[dict(event='network_probe', sequence=42,
            send_us=5_100_000, elapsed_us=5_300_000, status='timeout')]
        events.append(dict(event='telemetry', dropped_events=0, written_events=len(events)))
        with tempfile.TemporaryDirectory() as d:
            p, errors = Path(d)/'frames.jsonl', Path(d)/'server-errors.jsonl'
            p.write_text('\n'.join(json.dumps(e) for e in events))
            errors.write_text(json.dumps(dict(operation='send', sequence=42,
                elapsed_us=999_000_000, winerror=10055, errno=10055)))
            row = analyze(p, network_errors_path=errors)['gaps'][0]
        self.assertEqual(row['concurrent_probes']['responder_send_error_codes'], [10055])
