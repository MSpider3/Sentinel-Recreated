#!/usr/bin/env python3
import sys
import os
import time
import argparse
import getpass
from datetime import datetime

# Add sentinel_py to sys.path
sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..')))

from sentinel_py.dbus_client import SentinelDBusClient

def parse_args():
    default_user = os.environ.get("SUDO_USER") or os.environ.get("USER") or getpass.getuser()
    parser = argparse.ArgumentParser(description="Sentinel Recognition Accuracy Benchmark Test")
    parser.add_argument("--user", type=str, default=default_user, help="Username to test authentication against")
    parser.add_argument("--output-dir", type=str, default=None, help="Directory to save report output")
    return parser.parse_args()

# NOTE: the daemon never returns the real match distance over DBus (it would
# let a caller probe someone's face template). These tests judge by the result
# string and tier, and show the real distance and anti-spoof score by reading
# the audit log (SentinelDBusClient.last_auth_scores).

def tier_to_str(tier_num: int) -> str:
    mapping = {0: "No face", 1: "Tier 1 (Golden)", 2: "Tier 2 (Standard)", 4: "Not granted"}
    return mapping.get(tier_num, f"Tier {tier_num}")

def granted_count(results) -> int:
    return sum(1 for res, _, _ in results if res == "GRANTED")

def run_auth_attempts(client: SentinelDBusClient, username: str, count: int, sleep_sec: float = 2.0):
    results = []
    for i in range(count):
        print(f"  Attempt {i+1}/{count} ...", end="", flush=True)
        t0 = time.time()
        res, _, tier = client.authenticate(username)
        scores = client.last_auth_scores(t0)
        print(f" Result: {res} | {scores} | {tier_to_str(tier)}")
        results.append((res, scores, tier))
        if i < count - 1:
            time.sleep(sleep_sec)  # Inter-session cleanup delay
    return results

def main():
    args = parse_args()
    username = args.user

    print("==========================================================")
    print("      Sentinel Systematic Biometric Recognition Test      ")
    print("==========================================================")
    print(f"Target user: {username}\n")

    client = SentinelDBusClient()

    # Check status
    try:
        status = client.get_status()
        print(f"[Daemon Status] Uptime: {status.get('daemon_uptime_secs', 0)}s | Enrolled Users: {status.get('enrolled_users_count', 0)}")
    except Exception as e:
        print(f"Error connecting to Sentinel daemon DBus service: {e}")
        sys.exit(1)

    report_lines = []
    report_lines.append(f"Sentinel Biometric Recognition Accuracy Report — {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}")
    report_lines.append(f"Tested User: {username}")
    report_lines.append("=" * 70)

    # -------------------------------------------------------------------------
    # Test 1 — Self recognition
    # -------------------------------------------------------------------------
    print("\n--- Test 1: Self Recognition (10 Auth Attempts) ---")
    print("Sit in front of your camera in your normal position.")
    input("Press ENTER to begin Test 1...")

    t1_results = run_auth_attempts(client, username, 10, sleep_sec=2.0)
    t1_passes = granted_count(t1_results)

    print(f"\nTest 1 Summary: {t1_passes}/10 attempts were GRANTED.")
    t1_pass = t1_passes >= 8
    print(f"Test 1 Status: {'PASS' if t1_pass else 'FAIL'}")

    report_lines.append("\n[Test 1: Self Recognition]")
    for idx, (res, scores, tier) in enumerate(t1_results, 1):
        report_lines.append(f"  Attempt {idx:2d}: status={res:<10} {scores} tier={tier_to_str(tier)}")
    report_lines.append(f"  Pass Rate: {t1_passes}/10 GRANTED | Result: {'PASS' if t1_pass else 'FAIL'}")

    # -------------------------------------------------------------------------
    # Test 2 — Distance sensitivity
    # -------------------------------------------------------------------------
    print("\n--- Test 2: Distance Sensitivity ---")

    input("Prompt 2.1: Sit at your NORMAL distance. Press ENTER when ready...")
    t2_normal = granted_count(run_auth_attempts(client, username, 5, sleep_sec=2.0))

    input("Prompt 2.2: Move 30cm FURTHER BACK. Press ENTER when ready...")
    t2_back = granted_count(run_auth_attempts(client, username, 5, sleep_sec=2.0))

    input("Prompt 2.3: Move 30cm CLOSER. Press ENTER when ready...")
    t2_closer = granted_count(run_auth_attempts(client, username, 5, sleep_sec=2.0))

    print("\nTest 2 Summary (GRANTED out of 5):")
    print(f"  Normal position : {t2_normal}/5")
    print(f"  +30cm further   : {t2_back}/5")
    print(f"  -30cm closer    : {t2_closer}/5")

    report_lines.append("\n[Test 2: Distance Sensitivity — GRANTED out of 5]")
    report_lines.append(f"  Normal position : {t2_normal}/5")
    report_lines.append(f"  +30cm further   : {t2_back}/5")
    report_lines.append(f"  -30cm closer    : {t2_closer}/5")

    # -------------------------------------------------------------------------
    # Test 3 — Lighting variation
    # -------------------------------------------------------------------------
    print("\n--- Test 3: Lighting Variation ---")

    input("Prompt 3.1: NORMAL lighting. Press ENTER when ready...")
    t3_normal = granted_count(run_auth_attempts(client, username, 3, sleep_sec=2.0))

    input("Prompt 3.2: Turn OFF overhead light (use screen light only). Press ENTER when ready...")
    t3_dark = granted_count(run_auth_attempts(client, username, 3, sleep_sec=2.0))

    input("Prompt 3.3: Turn lights BACK ON. Press ENTER when ready...")
    t3_restored = granted_count(run_auth_attempts(client, username, 3, sleep_sec=2.0))

    print("\nTest 3 Summary (GRANTED out of 3):")
    print(f"  Normal lighting  : {t3_normal}/3")
    print(f"  Low/Screen light : {t3_dark}/3")
    print(f"  Restored light   : {t3_restored}/3")

    report_lines.append("\n[Test 3: Lighting Variation — GRANTED out of 3]")
    report_lines.append(f"  Normal light   : {t3_normal}/3")
    report_lines.append(f"  Screen-only    : {t3_dark}/3")
    report_lines.append(f"  Restored light : {t3_restored}/3")

    # -------------------------------------------------------------------------
    # Test 4 — Glasses cross-test
    # -------------------------------------------------------------------------
    print("\n--- Test 4: Glasses Cross-Test ---")
    do_glasses = input("Would you like to run the Glasses Cross-Test? (y/n): ").strip().lower()

    t4_pass = True
    if do_glasses == 'y':
        input("Prompt 4.1: Put GLASSES ON. Press ENTER when ready...")
        t4_on = granted_count(run_auth_attempts(client, username, 3, sleep_sec=2.0))

        input("Prompt 4.2: Take GLASSES OFF. Press ENTER when ready...")
        t4_off = granted_count(run_auth_attempts(client, username, 3, sleep_sec=2.0))

        t4_pass = t4_on >= 2 and t4_off >= 2
        print(f"\nTest 4 Summary: Glasses ON {t4_on}/3 | Glasses OFF {t4_off}/3 | Status: {'PASS' if t4_pass else 'FAIL'}")

        report_lines.append("\n[Test 4: Glasses Cross-Test — GRANTED out of 3]")
        report_lines.append(f"  Glasses ON  : {t4_on}/3")
        report_lines.append(f"  Glasses OFF : {t4_off}/3")
        report_lines.append(f"  Result      : {'PASS' if t4_pass else 'FAIL'}")
    else:
        print("Skipping Test 4 (Glasses Cross-Test).")
        report_lines.append("\n[Test 4: Glasses Cross-Test]\n  SKIPPED")

    # -------------------------------------------------------------------------
    # Test 5 — False acceptance threshold
    # -------------------------------------------------------------------------
    print("\n--- Test 5: False Acceptance Threshold ---")
    print("NOTE: Have a DIFFERENT person sit in front of the camera.")
    print("      (Alternative: If a second person is not available, point the camera at a photo of a completely different person).")
    print("NOTE: after 5 failed attempts in a minute the daemon answers RATE_LIMITED for a while — that is expected.")
    input("Press ENTER when ready to run 5 false-acceptance attempts...")

    t5_results = run_auth_attempts(client, username, 5, sleep_sec=2.0)
    t5_denied_count = sum(1 for res, _, _ in t5_results if res != "GRANTED")

    t5_pass = t5_denied_count == 5
    if not t5_pass:
        print("\n!!! SECURITY FAILURE: a non-enrolled face was GRANTED !!!")
    else:
        print("\nTest 5 Summary: ALL 5/5 attempts correctly refused. PASS!")

    report_lines.append("\n[Test 5: False Acceptance]")
    for idx, (res, scores, tier) in enumerate(t5_results, 1):
        report_lines.append(f"  Attempt {idx}: status={res:<12} {scores} tier={tier_to_str(tier)}")
    report_lines.append(f"  Refused Count: {t5_denied_count}/5 | Result: {'PASS' if t5_pass else 'SECURITY FAILURE'}")

    # -------------------------------------------------------------------------
    today_str = datetime.now().strftime("%Y%m%d")
    out_dir = args.output_dir or os.path.join(os.path.dirname(__file__), '..', 'personal')
    if not os.path.isdir(out_dir):
        out_dir = os.path.dirname(__file__)
    report_file = os.path.join(out_dir, f"recognition_report_{today_str}.txt")
    with open(report_file, "w") as f:
        f.write("\n".join(report_lines) + "\n")

    print(f"\n==========================================================")
    print(f"Full accuracy report saved to: {report_file}")
    print(f"Overall Biometric Tests Result: {'PASS' if (t1_pass and t4_pass and t5_pass) else 'FAIL'}")
    print(f"==========================================================")

if __name__ == "__main__":
    main()
