#!/usr/bin/env python3
"""
Headless test of the Textual dashboard against a stand-in daemon client.
Needs no daemon, camera or display:  python3 tests/general/test_tui.py
"""

import logging
import os
import sys
import unittest

# unittest runs the event loop in debug mode, which logs every slow redraw.
logging.getLogger("asyncio").setLevel(logging.ERROR)

sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..')))

from textual.widgets import DataTable, Input, Static

import sentinel_py.tui.app as tui

CONFIG = """[camera]
source = "/dev/video0"
width = 640
height = 480
fps = 30

[detection]
scrfd_input_size = 320
score_threshold = 0.5
nms_threshold = 0.3
min_face_size_px = 80

[security]
golden_threshold = 0.28
standard_threshold = 0.42
two_factor_threshold = 0.5
spoof_threshold = 0.8
spoof_threshold_standard = 0.7
max_retries = 3
global_session_timeout = 7.0
gallery_max_size = 20

[adaptive_policy]
adaptation_limit_per_day = 1

[hardware]
onnx_num_threads = 2
"""


class FakeClient:
    def __init__(self):
        self.saved = []
        self.save_result = (True, "Configuration updated successfully")
        self.dismissed = []

    def get_status(self):
        return {"daemon_uptime_secs": 12, "camera_source": "/dev/video0", "enrolled_users_count": 1,
                "last_auth_result": "GRANTED (tier=1)",
                "models_loaded": {"scrfd_500m_kps": True, "mobile_facenet": True, "minifasnetv2": True}}

    def get_recent_auth_log(self, lines=10):
        return ["2026-07-21T14:32:10.104Z|alice|GRANTED|0.1820|1|PASSIVE|0.984|380",
                "2026-07-21T14:42:15.510Z|unknown|SPOOF|0.2100|1|PASSIVE|0.020|620"]

    def list_users(self):
        return ["alice"]

    def get_user_info(self, username):
        return {"username": username, "core_vector_count": 15, "adaptive_vector_count": 2,
                "last_adaptation_date": "2026-07-20", "enrolled_at": "2026-07-01 10:00"}

    def get_intrusion_list(self):
        return ["intrusion_20260701_101500.jpg", "intrusion_20260720_183000.jpg"]

    def dismiss_intrusion(self, filename):
        self.dismissed.append(filename)

    def get_config(self):
        return CONFIG

    def set_config(self, toml_string):
        self.saved.append(toml_string)
        return self.save_result


def text_of(widget: Static) -> str:
    return str(widget.render())


class TestDashboard(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.fake = FakeClient()
        tui.SentinelDBusClient = lambda: self.fake

    async def test_screens_show_daemon_data(self):
        async with tui.SentinelApp().run_test(size=(140, 50)) as pilot:
            await pilot.pause()
            log = pilot.app.screen.query_one("#log_table", DataTable)
            self.assertEqual(log.row_count, 2)
            newest = [str(c) for c in log.get_row_at(0)]
            self.assertIn("SPOOF", newest[2])          # newest entry first
            self.assertEqual(newest[3:], ["0.2100", "1", "0.020", "620"])
            self.assertNotIn("T14:42", newest[0])      # shown as local time, not the raw UTC stamp
            self.assertIn("GRANTED", text_of(pilot.app.screen.query_one("#status_panel", Static)))

            await pilot.press("u")
            await pilot.pause()
            users = pilot.app.screen.query_one("#users_table", DataTable)
            self.assertEqual([str(c) for c in users.get_row_at(0)], ["alice", "15", "2", "2026-07-20", "2026-07-01 10:00"])

            await pilot.press("i")
            await pilot.pause()
            photos = pilot.app.screen.query_one("#intrusions_table", DataTable)
            self.assertEqual(str(photos.get_row_at(0)[0]), "intrusion_20260720_183000.jpg")  # newest first
            self.assertEqual(str(photos.get_row_at(0)[1]), "2026-07-20 18:30:00")

            # The dashboard's 5 s refresh timer must not fail while another screen is open.
            await pilot.pause(5.5)
            await pilot.press("d")
            await pilot.pause()
            self.assertEqual(pilot.app.screen.query_one("#log_table", DataTable).row_count, 2)

    async def test_settings_load_and_save(self):
        async with tui.SentinelApp().run_test(size=(140, 60)) as pilot:
            await pilot.press("s")
            await pilot.pause()
            screen = pilot.app.screen
            value = lambda i: screen.query_one(f"#{i}", Input).value
            self.assertEqual(value("in_golden"), "0.28")
            self.assertEqual(value("in_spoof_std"), "0.7")
            self.assertEqual(value("in_timeout"), "7")
            self.assertEqual(value("in_camera"), "/dev/video0")

            screen.query_one("#in_golden", Input).value = "0.25"
            screen.query_one("#in_timeout", Input).value = "5"
            screen.query_one("#btn_save").press()
            await pilot.pause()
            self.assertEqual(len(self.fake.saved), 1)
            saved = self.fake.saved[0]
            self.assertIn("golden_threshold = 0.25\n", saved)
            self.assertIn("global_session_timeout = 5.0\n", saved)
            self.assertIn('source = "/dev/video0"\n', saved)
            # Everything that was not edited is written back exactly as it was.
            untouched = lambda text: [l for l in text.splitlines() if not l.startswith(("golden_threshold", "global_session_timeout"))]
            self.assertEqual(untouched(saved), untouched(CONFIG))
            self.assertIn("saved", text_of(screen.query_one("#save_status", Static)))

    async def test_settings_report_bad_input_and_daemon_refusal(self):
        async with tui.SentinelApp().run_test(size=(140, 60)) as pilot:
            await pilot.press("s")
            await pilot.pause()
            screen = pilot.app.screen
            status = lambda: text_of(screen.query_one("#save_status", Static))

            screen.query_one("#in_golden", Input).value = "abc"
            screen.query_one("#btn_save").press()
            await pilot.pause()
            self.assertEqual(self.fake.saved, [])      # nothing sent to the daemon
            self.assertIn("golden_threshold", status())

            screen.query_one("#in_golden", Input).value = "0.9"
            self.fake.save_result = (False, "security.golden_threshold = 0.9 is outside the allowed range")
            screen.query_one("#btn_save").press()
            await pilot.pause()
            self.assertIn("outside the allowed range", status())


if __name__ == "__main__":
    unittest.main(verbosity=2)
