"""Do not turn coalesced display observations or incomplete logs into a pass."""
import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest

from smoke.remote_desktop_stability import summarize, performance_failures


class StabilityEvidence(unittest.TestCase):
    def analyze(self, events, encoding="utf-8"):
        with TemporaryDirectory() as directory:
            path = Path(directory) / "frames.jsonl"
            path.write_text("\n".join(json.dumps(event) for event in events), encoding=encoding)
            return summarize(path, warmup_seconds=0)

    def events(self):
        events = [
            {"event": "opened", "seconds": 30},
            {"event": "presented", "elapsed_us": 0, "frame_id": 1},
            {"event": "presented", "elapsed_us": 1_000_000, "frame_id": 30},
            {"event": "presented", "elapsed_us": 2_000_000, "frame_id": 60},
            {"event": "closed", "native_summary": {"displayed": 60}},
        ]
        return events + [{"event": "telemetry", "written_events": len(events), "dropped_events": 0}]

    def test_frame_id_jumps_are_not_counted_as_presentations(self):
        result = self.analyze(self.events())
        self.assertTrue(result["completed"])
        self.assertTrue(result["measurement_complete"])
        self.assertEqual(result["presentation_notification_fps"], 1.0)
        self.assertEqual(result["presentation_notifications"], 3)
        self.assertEqual(result["native_summary"]["displayed"], 60)

    def test_utf16_powershell_output_preserves_the_measurement(self):
        events = self.events()
        expected = self.analyze(events)
        result = self.analyze(events, "utf-16")
        for key in ("measurement_complete", "presentation_notifications", "presentation_gap_ms"):
            self.assertEqual(result[key], expected[key])

    def test_missing_close_is_not_a_completed_connection(self):
        events = self.events()
        events.pop(-2)
        self.assertFalse(self.analyze(events)["completed"])

    def test_missing_tail_or_dropped_or_missing_events_are_incomplete_evidence(self):
        events = self.events()
        self.assertFalse(self.analyze(events[:-1])["measurement_complete"])
        events[-1]["dropped_events"] = 1
        self.assertFalse(self.analyze(events)["measurement_complete"])
        events[-1]["dropped_events"] = 0
        events.pop(2)
        self.assertFalse(self.analyze(events)["measurement_complete"])

    def test_stream_failure_cannot_pass_even_with_a_final_summary(self):
        events = self.events()
        events.insert(-1, {"event": "error", "message": "video stream ended early"})
        events[-1]["written_events"] += 1
        result = self.analyze(events)
        self.assertTrue(result["measurement_complete"])
        self.assertFalse(result["completed"])

    def test_sparse_native_observations_cannot_substantiate_visible_fps(self):
        events = self.events()
        events[-2]["native_summary"]["displayed"] = 1
        result = self.analyze(events)
        self.assertTrue(result["measurement_complete"])
        self.assertFalse(result["display_evidence_sufficient"])
        self.assertIn("insufficient actual presentation observations", performance_failures(result))

    def test_coalesced_notifications_may_have_more_native_observations(self):
        result = self.analyze(self.events())
        self.assertTrue(result["display_evidence_sufficient"])
        self.assertNotIn("insufficient actual presentation observations", performance_failures(result))

    def test_missing_native_summary_is_not_visible_performance_evidence(self):
        events = self.events()
        events[-2]["native_summary"] = None
        result = self.analyze(events)
        self.assertIsNone(result["native_observation_ratio"])
        self.assertFalse(result["display_evidence_sufficient"])


if __name__ == "__main__":
    unittest.main()
