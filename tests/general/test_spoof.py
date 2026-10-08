import sys
import os
import time
import argparse
import getpass
from datetime import datetime

sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..')))

from sentinel_py.dbus_client import SentinelDBusClient

def parse_args():
    default_user = os.environ.get("SUDO_USER") or os.environ.get("USER") or getpass.getuser()
    parser = argparse.ArgumentParser(description="Sentinel Anti-Spoofing Verification Test")
    parser.add_argument("--user", type=str, default=default_user, help="Username to test authentication against")
    parser.add_argument("--output-dir", type=str, default=None, help="Directory to save report output")
    return parser.parse_args()

def main():
    args = parse_args()
    username = args.user

    print("==========================================================")
    print("        Sentinel Face Anti-Spoofing Benchmark Test        ")
    print("==========================================================")
    print(f"Target user: {username}\n")

    client = SentinelDBusClient()

    try:
        status = client.get_status()
        minifas_loaded = status.get("models_loaded", {}).get("minifasnetv2", False)
        print(f"[Daemon Status] MiniFASNetV2 Model Active: {minifas_loaded}")
    except Exception as e:
        print(f"Error connecting to Sentinel daemon DBus service: {e}")
        sys.exit(1)

    report_lines = []
    report_lines.append(f"Sentinel Anti-Spoofing Report — {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}")
    report_lines.append(f"Tested User: {username}")
    report_lines.append("=" * 70)

    # -------------------------------------------------------------------------
    # Test 1 — Live Face (Should Pass)
    # -------------------------------------------------------------------------
    print("\n--- Test 1: Live Face Authentication (5 Attempts) ---")
    print("Sit in front of your camera naturally with your real face.")
    input("Press ENTER to begin Test 1...")

    t1_results = []
    for i in range(5):
        print(f"  Attempt {i+1}/5 ...", end="", flush=True)
        t0 = time.time()
        res, _, tier = client.authenticate(username)
        scores = client.last_auth_scores(t0)
        print(f" Result: {res:<12} | {scores} | Tier: {tier}")
        t1_results.append((res, scores, tier))
        if i < 4:
            time.sleep(2.0)

    t1_passed = sum(1 for res, dist, tier in t1_results if res == "GRANTED")
    t1_pass = t1_passed == 5

    print(f"\nTest 1 Summary: {t1_passed}/5 live attempts succeeded. Result: {'PASS' if t1_pass else 'FAIL'}")
    report_lines.append("\n[Test 1: Live Face]")
    for idx, (res, scores, tier) in enumerate(t1_results, 1):
        report_lines.append(f"  Attempt {idx}: status={res:<12} {scores} tier={tier}")
    report_lines.append(f"  Live Pass Rate: {t1_passed}/5 | Result: {'PASS' if t1_pass else 'FAIL'}")

    # -------------------------------------------------------------------------
    # Test 2 — Photo Spoof Attack
    # -------------------------------------------------------------------------
    print("\n--- Test 2: Photo Spoof Attack (5 Attempts) ---")
    print("Hold a PHOTO of yourself (printed photo or smartphone screen showing your face) in front of the camera.")
    print("NOTE ON REJECTION TYPES:")
    print("  - SPOOF        : the face matched you, but the anti-spoof model saw a photo/screen")
    print("  - DENIED       : the face did not match at all")
    print("  - TIMEOUT      : no decision within the time limit")
    print("  - RATE_LIMITED : too many failed attempts in a minute (face unlock paused)")
    print("Anything except GRANTED is a successful rejection. For a real test the photo must be OF YOU.\n")

    input("Hold photo in front of camera and press ENTER to begin Test 2...")

    t2_results = []
    for i in range(5):
        print(f"  Attempt {i+1}/5 ...", end="", flush=True)
        t0 = time.time()
        res, _, tier = client.authenticate(username)
        scores = client.last_auth_scores(t0)
        print(f" Result: {res:<12} | {scores} | Tier: {tier}")
        t2_results.append((res, scores, tier))
        if i < 4:
            time.sleep(2.0)

    spoof_count = sum(1 for res, _, _ in t2_results if res == "SPOOF")
    granted_count = sum(1 for res, _, _ in t2_results if res == "GRANTED")
    denied_count = len(t2_results) - spoof_count - granted_count

    total_rejected = spoof_count + denied_count
    t2_pass = granted_count == 0

    print("\nTest 2 Breakdown:")
    print(f"  Explicit SPOOF rejections  : {spoof_count}")
    print(f"  Other rejections           : {denied_count}")
    print(f"  Unwanted GRANTED matches   : {granted_count}")
    print(f"  Total Security Rejections  : {total_rejected}/5")
    print(f"Test 2 Result: {'PASS' if t2_pass else 'FAIL'}")

    report_lines.append("\n[Test 2: Photo Spoof Attack]")
    for idx, (res, scores, tier) in enumerate(t2_results, 1):
        report_lines.append(f"  Attempt {idx}: status={res:<12} {scores} tier={tier}")
    report_lines.append(f"  Explicit SPOOF Rejections  : {spoof_count}")
    report_lines.append(f"  Other Rejections           : {denied_count}")
    report_lines.append(f"  Total Security Rejections  : {total_rejected}/5 | Result: {'PASS' if t2_pass else 'FAIL'}")

    if spoof_count == 0 and granted_count == 0:
        note = "Note: no explicit SPOOF result. If the photo was not recognised as you, the anti-spoof model was never tested — repeat with a clear photo or phone video of yourself."
        print(f"\n{note}")
        report_lines.append(f"  {note}")

    today_str = datetime.now().strftime("%Y%m%d")
    out_dir = args.output_dir or os.path.join(os.path.dirname(__file__), '..', 'personal')
    if not os.path.isdir(out_dir):
        out_dir = os.path.dirname(__file__)
    report_file = os.path.join(out_dir, f"spoof_report_{today_str}.txt")
    with open(report_file, "w") as f:
        f.write("\n".join(report_lines) + "\n")

    print(f"\n==========================================================")
    print(f"Full spoof test report saved to: {report_file}")
    print(f"Overall Anti-Spoofing Result: {'PASS' if (t1_pass and t2_pass) else 'FAIL'}")
    print(f"==========================================================")

if __name__ == "__main__":
    main()
