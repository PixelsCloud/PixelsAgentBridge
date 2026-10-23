"""A UI tap is not proof that Android stopped accepting remote control."""
import unittest
from unittest.mock import patch

from smoke.android import Home
import android_home


class StopConnectionTests(unittest.TestCase):
    def home(self):
        home = Home.__new__(Home)
        home.serial = "synthetic-device"
        return home

    def test_waits_for_runtime_and_persisted_stop(self):
        home = self.home()
        states = [
            {"connection_requested": False, "remote_ready": True},
            {"connection_requested": False, "remote_ready": False},
            {"connection_requested": False, "remote_ready": False},
        ]
        with patch.object(home, "click"), patch.object(android_home.time, "sleep"), \
                patch.object(android_home, "normal_status", side_effect=states), \
                patch.object(android_home, "adb", side_effect=["", "connection_requested = true", "connection_requested = false"]) as adb:
            home.stop()
        self.assertEqual(adb.call_count, 3)

    def test_ui_stop_without_persisted_intent_cannot_pass(self):
        home = self.home()
        with patch.object(home, "click"), patch.object(android_home.time, "sleep"), \
                patch.object(android_home.time, "monotonic", side_effect=[0, 1, 16]), \
                patch.object(android_home, "normal_status", return_value={"connection_requested": False, "remote_ready": False}), \
                patch.object(android_home, "adb", return_value="connection_requested = true"):
            with self.assertRaisesRegex(RuntimeError, "persisted connection"):
                home.stop()


if __name__ == "__main__":
    unittest.main()
