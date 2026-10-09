import os
os.environ["QT_QPA_PLATFORM"] = "xcb"

import sys
import getpass
import time
import tomllib
import cv2
import numpy as np
from sentinel_py.dbus_client import SentinelDBusClient

# Why the daemon did not accept a frame, in words for the preview window.
STATUS_HINTS = {
    "NO_FACE": "No face detected, position yourself in frame",
    "MULTIPLE_FACES": "Multiple faces in frame",
    "OUT_OF_FRAME": "Move your whole face into view",
    "NOT_FRONTAL": "Turned too far - turn back towards the camera a little",
    "TOO_DARK": "Too dark - add some light",
    "TOO_BRIGHT": "Too bright - reduce the light on your face",
    "BLURRY": "Hold still",
    "TOO_SIMILAR": "Same as the last capture - move your head slightly",
    "FULL": "Enough samples collected",
    "ERROR": "Cannot reach the Sentinel daemon",
}

POSES = [
    {"name": "Center", "instruction": "Look directly at the camera lens"},
    {"name": "Left",   "instruction": "Turn your head LEFT"},
    {"name": "Right",  "instruction": "Turn your head RIGHT"},
    {"name": "Up",     "instruction": "Tilt your head UP"},
    {"name": "Down",   "instruction": "Tilt your head DOWN"},
]

class EnrollmentWizard:
    def __init__(self, username: str, glasses: bool = False):
        self.username = username
        self.glasses = glasses
        self.client = SentinelDBusClient()

    def _open_camera(self):
        """Open the same camera the daemon authenticates with (camera.source in its config)."""
        try:
            source = tomllib.loads(self.client.get_config()).get("camera", {}).get("source", "/dev/video0")
        except Exception:
            source = "/dev/video0"
        if not source.startswith("/dev/video"):
            source = "/dev/video0"
        return source, cv2.VideoCapture(source, cv2.CAP_V4L2)

    def run(self):
        print(f"=== Sentinel Face ID Enrollment Wizard ===")
        print(f"User: {self.username} | Glasses mode: {'YES' if self.glasses else 'NO'}")

        # Continuous camera stream
        source, cap = self._open_camera()
        if not cap.isOpened():
            print(f"Error: Could not open camera {source} for preview.")
            return False

        cap.set(cv2.CAP_PROP_FRAME_WIDTH, 640)
        cap.set(cv2.CAP_PROP_FRAME_HEIGHT, 480)

        # Start DBus enrollment session
        try:
            session_id = self.client.start_enrollment(self.username)
        except Exception as e:
            print(f"Error: Could not start enrollment: {e}")
            cap.release()
            return False

        passes = 2 if self.glasses else 1
        total_poses_count = len(POSES) * passes
        total_captured_vectors = 0
        window_name = "Sentinel Face ID Enrollment"

        cv2.namedWindow(window_name, cv2.WINDOW_NORMAL)
        cv2.resizeWindow(window_name, 640, 480)

        try:
            for pass_idx in range(passes):
                if self.glasses and pass_idx == 1:
                    self._show_pause_prompt(cap, window_name, "Please remove your glasses, then press SPACE to continue")

                pass_label = " (With Glasses)" if (self.glasses and pass_idx == 0) else (" (Without Glasses)" if self.glasses else "")

                for pose_idx, pose in enumerate(POSES):
                    pose_num = (pass_idx * len(POSES)) + pose_idx + 1
                    pose_title = f"{pose['name']}{pass_label}"
                    
                    sub_count = self._run_pose_loop(
                        cap, window_name, session_id, pose_title, pose['instruction'], pose_num, total_poses_count
                    )
                    total_captured_vectors += sub_count

            # Multi-lighting pass option
            if self._show_lighting_pass_prompt(cap, window_name, total_captured_vectors):
                lighting_pose = POSES[0]
                sub_count = self._run_pose_loop(
                    cap, window_name, session_id, "Center (Lighting Pass)", lighting_pose['instruction'], total_poses_count + 1, total_poses_count + 1
                )
                total_captured_vectors += sub_count

            # Finish DBus enrollment session
            success, msg = self.client.finish_enrollment(session_id)
            print(f"\n[ENROLLMENT RESULT]: {msg}")

            # Completion screen
            end_time = time.time() + 2.0
            while time.time() < end_time:
                ret, frame = cap.read()
                if not ret:
                    break
                self._draw_completion_overlay(frame, total_captured_vectors)
                cv2.imshow(window_name, frame)
                if cv2.waitKey(30) & 0xFF == 27:
                    break

            return success

        except KeyboardInterrupt:
            print("\nEnrollment cancelled by user.")
            self.client.cancel_enrollment(session_id)
            return False
        finally:
            cap.release()
            cv2.destroyAllWindows()

    def _show_pause_prompt(self, cap: cv2.VideoCapture, window_name: str, prompt_text: str):
        while True:
            ret, frame = cap.read()
            if not ret:
                break
            h, w = frame.shape[:2]
            cv2.rectangle(frame, (20, h // 2 - 40), (w - 20, h // 2 + 40), (20, 20, 20), -1)
            cv2.rectangle(frame, (20, h // 2 - 40), (w - 20, h // 2 + 40), (0, 255, 255), 2)
            cv2.putText(frame, prompt_text, (30, h // 2), cv2.FONT_HERSHEY_SIMPLEX, 0.6, (255, 255, 255), 2)
            cv2.putText(frame, "Press SPACE to continue", (30, h // 2 + 30), cv2.FONT_HERSHEY_SIMPLEX, 0.55, (0, 255, 255), 1)
            cv2.imshow(window_name, frame)
            key = cv2.waitKey(30) & 0xFF
            if key == 32: # SPACE
                break
            if key == 27: # ESC
                raise KeyboardInterrupt()

    def _show_lighting_pass_prompt(self, cap: cv2.VideoCapture, window_name: str, total_vectors: int) -> bool:
        prompt_lines = [
            f"Standard poses complete ({total_vectors} vectors).",
            "",
            "For better recognition in varied conditions, we recommend one additional pass:",
            "- Slightly dim your screen or change your lighting",
            "- You will repeat just the CENTER pose 3 more times",
            "",
            "Press SPACE to do the lighting variation pass, or ENTER to skip."
        ]
        print("\n" + "\n".join(prompt_lines))
        while True:
            ret, frame = cap.read()
            if not ret:
                break
            h, w = frame.shape[:2]
            cv2.rectangle(frame, (20, h // 2 - 100), (w - 20, h // 2 + 100), (20, 20, 20), -1)
            cv2.rectangle(frame, (20, h // 2 - 100), (w - 20, h // 2 + 100), (0, 255, 255), 2)

            cv2.putText(frame, f"Standard poses complete ({total_vectors} vectors).", (30, h // 2 - 60), cv2.FONT_HERSHEY_SIMPLEX, 0.6, (0, 255, 255), 2)
            cv2.putText(frame, "Recommended: 1 additional pass under varied lighting", (30, h // 2 - 30), cv2.FONT_HERSHEY_SIMPLEX, 0.5, (255, 255, 255), 1)
            cv2.putText(frame, "- Dim screen / change room lighting, repeat CENTER pose", (30, h // 2 - 5), cv2.FONT_HERSHEY_SIMPLEX, 0.5, (255, 255, 255), 1)
            cv2.putText(frame, "Press SPACE to do lighting pass, or ENTER to skip", (30, h // 2 + 35), cv2.FONT_HERSHEY_SIMPLEX, 0.55, (0, 255, 0), 2)

            cv2.imshow(window_name, frame)
            key = cv2.waitKey(30) & 0xFF
            if key == 32: # SPACE
                return True
            if key in (13, 10): # ENTER
                return False
            if key == 27: # ESC
                raise KeyboardInterrupt()

    def _run_pose_loop(self, cap: cv2.VideoCapture, window_name: str, session_id: str,
                       pose_name: str, instruction: str, pose_num: int, total_poses: int) -> int:
        sub_captured = 0
        target_sub = 3
        status = "NO_FACE"
        face_bbox = None
        last_check = 0

        while sub_captured < target_sub:
            ret, frame = cap.read()
            if not ret:
                time.sleep(0.03)
                continue

            now = time.time()
            # Preview check at ~10 Hz: the daemon only inspects the frame, nothing is stored
            if now - last_check >= 0.10:
                last_check = now
                status, face_bbox = self._submit(session_id, frame, capture=False)

            # User presses SPACE to capture a sub-sample
            key = cv2.waitKey(30) & 0xFF
            if key == 27: # ESC
                raise KeyboardInterrupt()

            if key == 32: # SPACE
                # Only now is a frame stored as a template, and only if the daemon accepts it
                status, face_bbox = self._submit(session_id, frame, capture=True)
                last_check = time.time()
                if status == "ACCEPTED":
                    sub_captured += 1
                    print(f"Captured sub-sample {sub_captured}/{target_sub} for {pose_name}")
                    # Render UI so screen reflects updated capture count before delay
                    self._render_ui(frame, pose_name, instruction, pose_num, total_poses, sub_captured, target_sub, status, face_bbox, is_complete=False)
                    cv2.imshow(window_name, frame)
                    cv2.waitKey(1)
                    # 500ms post-capture delay for natural angle micro-variation
                    time.sleep(0.5)
                elif status == "MULTIPLE_FACES":
                    print("[Warning] Multiple faces in frame — positioning required.")

            # Render UI
            self._render_ui(frame, pose_name, instruction, pose_num, total_poses, sub_captured, target_sub, status, face_bbox, is_complete=False)
            cv2.imshow(window_name, frame)

        # Pose Complete! Require SPACE to advance to next pose
        while True:
            ret, frame = cap.read()
            if not ret:
                break
            self._render_ui(frame, pose_name, instruction, pose_num, total_poses, 3, 3, "COMPLETE", face_bbox, is_complete=True)
            cv2.imshow(window_name, frame)
            key = cv2.waitKey(30) & 0xFF
            if key == 32: # SPACE
                break
            if key == 27: # ESC
                raise KeyboardInterrupt()

        return sub_captured

    def _submit(self, session_id: str, frame: np.ndarray, capture: bool):
        """Send one frame to the daemon; returns (status, face bbox or None)."""
        ok, jpeg_bytes = cv2.imencode('.jpg', frame, [int(cv2.IMWRITE_JPEG_QUALITY), 95])
        if not ok:
            return "ERROR", None
        try:
            status, _, _, data = self.client.submit_enrollment_frame_data(session_id, jpeg_bytes.tobytes(), capture)
        except Exception:
            return "ERROR", None
        bbox = [int(v) for v in data[:4]] if len(data) >= 4 else None
        return status, bbox

    def _render_ui(self, frame: np.ndarray, pose_name: str, instruction: str,
                   pose_num: int, total_poses: int, captured: int, target: int,
                   status: str, face_bbox: list[int], is_complete: bool):
        h, w = frame.shape[:2]

        # Top Header Bar (Black)
        cv2.rectangle(frame, (0, 0), (w, 50), (0, 0, 0), -1)
        header = f"Pose {pose_num}/{total_poses}: {instruction}"
        cv2.putText(frame, header, (15, 33), cv2.FONT_HERSHEY_SIMPLEX, 0.7, (255, 255, 255), 2)

        # Draw Face Bounding Box (Green = Face Found, Red = No Face)
        if face_bbox and status in ("ACCEPTED", "COMPLETE"):
            x1, y1, x2, y2 = face_bbox
            cv2.rectangle(frame, (x1, y1), (x2, y2), (0, 255, 0), 2)
        elif status in STATUS_HINTS:
            if face_bbox:
                x1, y1, x2, y2 = face_bbox
                cv2.rectangle(frame, (x1, y1), (x2, y2), (0, 0, 255), 2)
            cv2.putText(frame, STATUS_HINTS[status], (15, h - 60), cv2.FONT_HERSHEY_SIMPLEX, 0.6, (0, 0, 255), 2)

        # Bottom Bar (Black)
        cv2.rectangle(frame, (0, h - 45), (w, h), (0, 0, 0), -1)
        progress_bar = "■ " * captured + "□ " * (target - captured)
        sub_text = f"Sub-samples: {progress_bar} {captured}/{target}"
        cv2.putText(frame, sub_text, (15, h - 15), cv2.FONT_HERSHEY_SIMPLEX, 0.6, (255, 255, 255), 2)

        # Action Prompt
        if is_complete:
            cv2.putText(frame, "Pose complete! Press SPACE for next pose", (310, h - 15), cv2.FONT_HERSHEY_SIMPLEX, 0.55, (0, 255, 0), 2)
        else:
            cv2.putText(frame, "Press SPACE to capture", (360, h - 15), cv2.FONT_HERSHEY_SIMPLEX, 0.55, (0, 255, 255), 2)

    def _draw_completion_overlay(self, frame: np.ndarray, total_saved: int):
        h, w = frame.shape[:2]
        cv2.rectangle(frame, (0, 0), (w, h), (0, 150, 0), -1)
        text = f"Enrollment complete! {total_saved} vectors saved."
        cv2.putText(frame, text, (w // 2 - 220, h // 2), cv2.FONT_HERSHEY_SIMPLEX, 0.75, (255, 255, 255), 2)

def get_current_user() -> str:
    return os.environ.get("SUDO_USER") or os.environ.get("USER") or getpass.getuser()

def ask_glasses() -> bool:
    """Ask whether the user wears glasses, so both looks get enrolled.

    A face with glasses and the same face without them look different to the
    recogniser; someone who wears glasses only some of the time needs both.
    """
    if not sys.stdin.isatty():
        print("Not running in a terminal: enrolling without the glasses pass (use --glasses to include it).")
        return False
    while True:
        answer = input("Do you wear glasses, even only sometimes? [y/n]: ").strip().lower()
        if answer in ("y", "yes"):
            print("You will be enrolled twice: first WITH your glasses on, then without. Put them on now.")
            return True
        if answer in ("n", "no"):
            return False
        print("Please answer y or n.")

def main():
    target_user = None
    for arg in sys.argv[1:]:
        if not arg.startswith("--"):
            target_user = arg
            break
    username = target_user or get_current_user()
    if "--glasses" in sys.argv:
        glasses = True
    elif "--no-glasses" in sys.argv:
        glasses = False
    else:
        glasses = ask_glasses()

    wizard = EnrollmentWizard(username, glasses=glasses)
    wizard.run()

if __name__ == "__main__":
    main()
