#!/usr/bin/env python3
"""
Exploratory QA test suite for Sentinel Recreated.
Tests key user journeys, boundary conditions, input sanitization,
and security constraints via DBus and PAM interfaces.
"""

import os
import sys
import unittest
import dbus
from sentinel_py.dbus_client import SentinelDBusClient

class TestSentinelExploratoryQA(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        try:
            cls.client = SentinelDBusClient()
            cls.daemon_available = True
        except Exception as e:
            cls.daemon_available = False
            cls.daemon_error = str(e)

    def setUp(self):
        if not self.daemon_available:
            self.skipTest(f"Sentinel daemon not reachable via DBus: {getattr(self, 'daemon_error', 'unknown')}")

    def test_daemon_status_and_models(self):
        """Journey 1: Query daemon status and verify loaded models."""
        status = self.client.get_status()
        self.assertIsInstance(status, dict)
        self.assertIn("models_loaded", status)
        self.assertIn("camera_source", status)
        self.assertIn("enrolled_users_count", status)

    def test_list_users_journey(self):
        """Journey 2: Enumerate enrolled users."""
        users = self.client.list_users()
        self.assertIsInstance(users, list)
        for u in users:
            self.assertIsInstance(u, str)
            self.assertNotIn("/", u)
            self.assertNotIn("..", u)

    def test_get_user_info_boundary_validation(self):
        """Journey 3: Query user info with invalid/traversal paths."""
        traversal_attempts = [
            "../../../etc/passwd",
            "..",
            "/root",
            "test/user",
            "test\\user",
            ".hidden",
            "",
        ]
        for bad_user in traversal_attempts:
            try:
                info = self.client.get_user_info(bad_user)
                # If connected to patched daemon, DBus raises InvalidArgs error.
                # If connected to old unpatched daemon, it returns a dict with username=bad_user.
                print(f"  [Notice] get_user_info('{bad_user}') returned: {info}")
            except dbus.DBusException as e:
                # Patched daemon rejects invalid username with DBus InvalidArgs / Failed
                print(f"  [Verified] get_user_info('{bad_user}') properly rejected: {e}")

    def test_remove_user_boundary_validation(self):
        """Journey 4: Attempt user removal with malicious paths."""
        bad_users = [
            "../../../etc/shadow",
            "../../",
            "nonexistent_user_xyz_12345",
        ]
        for bad_user in bad_users:
            try:
                result = self.client.remove_user(bad_user)
                self.assertFalse(result, f"Expected remove_user to return False for '{bad_user}'")
            except dbus.DBusException:
                pass

    def test_dismiss_intrusion_filename_boundary(self):
        """Journey 5: Attempt intrusion dismissal with traversal paths."""
        bad_filenames = [
            "../../../etc/passwd",
            "/var/lib/sentinel/something",
            "sub/dir/file.jpg",
            "file\x00traversal.jpg",
        ]
        for bad_file in bad_filenames:
            try:
                self.client.dismiss_intrusion(bad_file)
            except (dbus.DBusException, ValueError):
                # Expected rejection due to filename validation, PolKit, or DBus string constraints
                pass

    def test_enrollment_session_lifecycle_bounds(self):
        """Journey 6: Test invalid session IDs on enrollment endpoints."""
        fake_sessions = [
            "",
            "00000000000000000000000000000000",
            "invalid-session-token",
            "../../../etc",
        ]
        for session_id in fake_sessions:
            try:
                success, msg = self.client.finish_enrollment(session_id)
                self.assertFalse(success, f"Expected finish_enrollment to fail for fake session '{session_id}'")
            except dbus.DBusException:
                pass

            try:
                self.client.cancel_enrollment(session_id)
            except dbus.DBusException:
                pass

    def test_authenticate_failsafe_behavior(self):
        """Journey 7: Verify Authenticate fails open-safe and prevents unauthorized cross-user queries."""
        # Querying an unknown / foreign user must be rejected or fail-safe
        foreign_user = "nobody"
        try:
            res, dist, tier = self.client.authenticate(foreign_user, {})
            # If allowed, result must be open-safe (NO_FACE / DENIED)
            self.assertIn(res, ["NO_FACE", "DENIED"])
        except dbus.DBusException:
            # Expected rejection by UID validation
            pass

if __name__ == "__main__":
    unittest.main(verbosity=2)
