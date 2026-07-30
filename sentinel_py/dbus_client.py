import json
import dbus
import dbus.mainloop.glib
from gi.repository import GLib

BUS_NAME = "com.sentinel.Sentinel"
OBJ_PATH = "/com/sentinel/Sentinel"
IFACE_NAME = "com.sentinel.Sentinel"

class SentinelDBusClient:
    """Client wrapper for com.sentinel.Sentinel DBus service."""

    def __init__(self):
        try:
            dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)
        except Exception:
            pass
        self.bus = dbus.SystemBus()
        self.proxy = self.bus.get_object(BUS_NAME, OBJ_PATH)
        self.iface = dbus.Interface(self.proxy, dbus_interface=IFACE_NAME)

    def get_status(self) -> dict:
        raw_json = self.iface.GetStatus()
        return json.loads(str(raw_json))

    def authenticate(self, username: str, session_env: dict = None) -> tuple[str, float, int]:
        if session_env is None:
            session_env = {}
        # Convert python dict to DBus a{ss}
        dbus_env = dbus.Dictionary(session_env, signature='ss')
        res, dist, tier = self.iface.Authenticate(username, dbus_env)
        return str(res), float(dist), int(tier)

    def start_enrollment(self, username: str) -> str:
        session_id = self.iface.StartEnrollment(username)
        return str(session_id)

    def submit_enrollment_frame(self, session_id: str) -> tuple[str, int, int, list[float]]:
        # Returns (status, pose_index, total_poses, landmarks_vec)
        status, pose_idx, total_poses, lms = self.iface.SubmitEnrollmentFrame(session_id)
        landmarks = [float(x) for x in lms]
        return str(status), int(pose_idx), int(total_poses), landmarks

    def submit_enrollment_frame_data(self, session_id: str, frame_data: bytes) -> tuple[str, int, int, list[float]]:
        # Encoded JPEG bytes passed to daemon without camera device contention
        byte_array = dbus.ByteArray(frame_data)
        status, pose_idx, total_poses, lms = self.iface.SubmitEnrollmentFrameData(session_id, byte_array)
        landmarks = [float(x) for x in lms]
        return str(status), int(pose_idx), int(total_poses), landmarks

    def finish_enrollment(self, session_id: str) -> tuple[bool, str]:
        success, message = self.iface.FinishEnrollment(session_id)
        return bool(success), str(message)

    def cancel_enrollment(self, session_id: str):
        self.iface.CancelEnrollment(session_id)

    def list_users(self) -> list[str]:
        users = self.iface.ListUsers()
        return [str(u) for u in users]

    def remove_user(self, username: str) -> bool:
        success = self.iface.RemoveUser(username)
        return bool(success)

    def get_intrusion_list(self) -> list[str]:
        files = self.iface.GetIntrusionList()
        return [str(f) for f in files]

    def dismiss_intrusion(self, filename: str):
        self.iface.DismissIntrusion(filename)

    def get_user_info(self, username: str) -> dict:
        raw_json = self.iface.GetUserInfo(username)
        return json.loads(str(raw_json))

    def get_config(self) -> str:
        return str(self.iface.GetConfig())

    def set_config(self, toml_string: str) -> tuple[bool, str]:
        res = self.iface.SetConfig(toml_string)
        return bool(res[0]), str(res[1])

    def reset_spoof_calibration(self) -> bool:
        res = self.iface.ResetSpoofCalibration()
        return bool(res)

    def run_spoof_calibration(self) -> str:
        res = self.iface.RunSpoofCalibration()
        return str(res)

    def listen_auth_status(self, callback_func):
        """Subscribe to AuthStatusChanged signals and run callback_func(status, message)."""
        def signal_handler(status, message):
            callback_func(str(status), str(message))

        self.bus.add_signal_receiver(
            signal_handler,
            dbus_interface=IFACE_NAME,
            signal_name="AuthStatusChanged"
        )

