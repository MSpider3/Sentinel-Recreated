import os
os.environ["QT_QPA_PLATFORM"] = "xcb"

import sys
import time
import cv2
import numpy as np
from sentinel_py.dbus_client import SentinelDBusClient

POSES = [
    {"name": "Center", "instruction": "Look directly at the camera lens"},
    {"name": "Left",   "instruction": "Turn your head LEFT"},
    {"name": "Right",  "instruction": "Turn your head RIGHT"},
    {"name": "Up",     "instruction": "Tilt your head UP"},
    {"name": "Down",   "instruction": "Tilt your head DOWN"},
]

class EnrollmentWizard:
    def __init__(self, username: str, glasses: bool = False, append_glasses: bool = False):
        self.username = username
        self.glasses = glasses
        self.append_glasses = append_glasses
        self.client = SentinelDBusClient()

    def run(self):
        print(f"=== Sentinel Face ID Enrollment Wizard ===")
        print(f"User: {self.username} | Glasses mode: {'YES' if self.glasses else 'NO'}")

        # Continuous camera stream
        cap = cv2.VideoCapture(0)
        if not cap.isOpened():
            print("Error: Could not open camera /dev/video0 for preview.")
            return False

        cap.set(cv2.CAP_PROP_FRAME_WIDTH, 640)
        cap.set(cv2.CAP_PROP_FRAME_HEIGHT, 480)

        # Start DBus enrollment session
        session_id = self.client.start_enrollment(self.username)

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
            # Send frame to daemon via SubmitEnrollmentFrameData at ~10 Hz
            if now - last_check >= 0.10:
                last_check = now
                ok, jpeg_bytes = cv2.imencode('.jpg', frame, [int(cv2.IMWRITE_JPEG_QUALITY), 80])
                if ok:
                    try:
                        res_status, _, _, res_data = self.client.submit_enrollment_frame_data(session_id, jpeg_bytes.tobytes())
                        status = res_status
                        if len(res_data) >= 4:
                            face_bbox = [int(res_data[0]), int(res_data[1]), int(res_data[2]), int(res_data[3])]
                        else:
                            face_bbox = None
                    except Exception:
                        status, face_bbox = "ERROR", None

            # User presses SPACE to capture a sub-sample
            key = cv2.waitKey(30) & 0xFF
            if key == 27: # ESC
                raise KeyboardInterrupt()

            if key == 32: # SPACE
                if status in ("ACCEPTED", "COMPLETE"):
                    sub_captured += 1
                    print(f"Captured sub-sample {sub_captured}/{target_sub} for {pose_name}")
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
        elif status == "MULTIPLE_FACES":
            cv2.putText(frame, "Multiple faces in frame", (15, h - 60), cv2.FONT_HERSHEY_SIMPLEX, 0.6, (0, 0, 255), 2)
        elif status == "NO_FACE":
            cv2.putText(frame, "No face detected, position yourself in frame", (15, h - 60), cv2.FONT_HERSHEY_SIMPLEX, 0.6, (0, 0, 255), 2)

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

def main():
    username = sys.argv[1] if len(sys.argv) > 1 else "mehulgolecha"
    glasses = "--glasses" in sys.argv
    append_glasses = "--append-glasses" in sys.argv

    wizard = EnrollmentWizard(username, glasses=glasses, append_glasses=append_glasses)
    wizard.run()

if __name__ == "__main__":
    main()
