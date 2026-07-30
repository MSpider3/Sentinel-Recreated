#!/usr/bin/env python3
import sys
import os
import time
import argparse
from datetime import datetime

# Add sentinel_py to sys.path
sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(__file__), '..')))

from sentinel_py.dbus_client import SentinelDBusClient

def parse_args():
    default_user = os.environ.get("SUDO_USER") or os.environ.get("USER") or "mehulgolecha"
    parser = argparse.ArgumentParser(description="Sentinel Recognition Accuracy Benchmark Test")
    parser.add_argument("--user", type=str, default=default_user, help="Username to test authentication against")
    return parser.parse_args()

def tier_to_str(tier_num: int) -> str:
    mapping = {1: "Tier 1 (Golden)", 2: "Tier 2 (Standard)", 3: "Tier 3 (TwoFactor)", 4: "Tier 4 (Denied)"}
    return mapping.get(tier_num, f"Tier {tier_num}")

def run_auth_attempts(client: SentinelDBusClient, username: str, count: int, sleep_sec: float = 2.0):
    results = []
    for i in range(count):
        print(f"  Attempt {i+1}/{count} ...", end="", flush=True)
        res, dist, tier = client.authenticate(username)
        print(f" Result: {res} | Distance: {dist:.4f} | {tier_to_str(tier)}")
        results.append((res, dist, tier))
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
    t1_passes = sum(1 for res, dist, tier in t1_results if tier in (1, 2) and dist < 0.42)
    t1_mean_d = sum(dist for _, dist, _ in t1_results) / len(t1_results)

    print(f"\nTest 1 Summary: {t1_passes}/10 attempts produced Tier 1 or Tier 2 (d < 0.42). Mean distance: {t1_mean_d:.4f}")
    t1_pass = t1_passes >= 8
    print(f"Test 1 Status: {'PASS' if t1_pass else 'FAIL'}")

    report_lines.append("\n[Test 1: Self Recognition]")
    for idx, (res, dist, tier) in enumerate(t1_results, 1):
        report_lines.append(f"  Attempt {idx:2d}: status={res:<10} dist={dist:.4f} tier={tier_to_str(tier)}")
    report_lines.append(f"  Pass Rate: {t1_passes}/10 in Tier 1/2 | Mean Distance: {t1_mean_d:.4f} | Result: {'PASS' if t1_pass else 'FAIL'}")

    # -------------------------------------------------------------------------
    # Test 2 — Distance sensitivity
    # -------------------------------------------------------------------------
    print("\n--- Test 2: Distance Sensitivity ---")

    input("Prompt 2.1: Sit at your NORMAL distance. Press ENTER when ready...")
    t2_normal = run_auth_attempts(client, username, 5, sleep_sec=2.0)
    mean_normal = sum(d for _, d, _ in t2_normal) / len(t2_normal)

    input("Prompt 2.2: Move 30cm FURTHER BACK. Press ENTER when ready...")
    t2_back = run_auth_attempts(client, username, 5, sleep_sec=2.0)
    mean_back = sum(d for _, d, _ in t2_back) / len(t2_back)

    input("Prompt 2.3: Move 30cm CLOSER. Press ENTER when ready...")
    t2_closer = run_auth_attempts(client, username, 5, sleep_sec=2.0)
    mean_closer = sum(d for _, d, _ in t2_closer) / len(t2_closer)

    print("\nTest 2 Summary:")
    print(f"  Normal position mean distance : {mean_normal:.4f}")
    print(f"  +30cm further mean distance   : {mean_back:.4f}")
    print(f"  -30cm closer mean distance    : {mean_closer:.4f}")

    report_lines.append("\n[Test 2: Distance Sensitivity]")
    report_lines.append(f"  Normal position mean distance : {mean_normal:.4f}")
    report_lines.append(f"  +30cm further mean distance   : {mean_back:.4f}")
    report_lines.append(f"  -30cm closer mean distance    : {mean_closer:.4f}")

    # -------------------------------------------------------------------------
    # Threshold Recommendations (from Observed Distances)
    # -------------------------------------------------------------------------
    rec_golden = max(0.15, round(mean_normal - 0.05, 3))
    rec_standard = min(0.42, round(mean_normal + 0.05, 3))
    print("\n----------------------------------------------------------")
    print(f"[THRESHOLD CALIBRATION RECOMMENDATION]")
    print(f"  Observed normal position mean distance: {mean_normal:.4f}")
    print(f"  Recommended golden_threshold   : {rec_golden:.3f}")
    print(f"  Recommended standard_threshold : {rec_standard:.3f}")
    print("----------------------------------------------------------")

    report_lines.append("\n[Threshold Recommendations]")
    report_lines.append(f"  Observed mean distance at normal position: {mean_normal:.4f}")
    report_lines.append(f"  Recommended golden_threshold   : {rec_golden:.3f}")
    report_lines.append(f"  Recommended standard_threshold : {rec_standard:.3f}")

    # -------------------------------------------------------------------------
    # Test 3 — Lighting variation
    # -------------------------------------------------------------------------
    print("\n--- Test 3: Lighting Variation ---")

    input("Prompt 3.1: NORMAL lighting. Press ENTER when ready...")
    t3_normal = run_auth_attempts(client, username, 3, sleep_sec=2.0)
    mean_l_normal = sum(d for _, d, _ in t3_normal) / len(t3_normal)

    input("Prompt 3.2: Turn OFF overhead light (use screen light only). Press ENTER when ready...")
    t3_dark = run_auth_attempts(client, username, 3, sleep_sec=2.0)
    mean_l_dark = sum(d for _, d, _ in t3_dark) / len(t3_dark)

    input("Prompt 3.3: Turn lights BACK ON. Press ENTER when ready...")
    t3_restored = run_auth_attempts(client, username, 3, sleep_sec=2.0)
    mean_l_restored = sum(d for _, d, _ in t3_restored) / len(t3_restored)

    print("\nTest 3 Summary:")
    print(f"  Normal lighting mean distance  : {mean_l_normal:.4f}")
    print(f"  Low/Screen light mean distance : {mean_l_dark:.4f}")
    print(f"  Restored light mean distance   : {mean_l_restored:.4f}")

    report_lines.append("\n[Test 3: Lighting Variation]")
    report_lines.append(f"  Normal light mean distance   : {mean_l_normal:.4f}")
    report_lines.append(f"  Screen-only mean distance    : {mean_l_dark:.4f}")
    report_lines.append(f"  Restored light mean distance : {mean_l_restored:.4f}")

    # -------------------------------------------------------------------------
    # Test 4 — Glasses cross-test
    # -------------------------------------------------------------------------
    print("\n--- Test 4: Glasses Cross-Test ---")
    do_glasses = input("Would you like to run the Glasses Cross-Test? (y/n): ").strip().lower()

    t4_pass = True
    if do_glasses == 'y':
        input("Prompt 4.1: Put GLASSES ON. Press ENTER when ready...")
        t4_on = run_auth_attempts(client, username, 3, sleep_sec=2.0)
        mean_on = sum(d for _, d, _ in t4_on) / len(t4_on)

        input("Prompt 4.2: Take GLASSES OFF. Press ENTER when ready...")
        t4_off = run_auth_attempts(client, username, 3, sleep_sec=2.0)
        mean_off = sum(d for _, d, _ in t4_off) / len(t4_off)

        t4_pass = all(d < 0.50 for _, d, _ in t4_on + t4_off)
        print(f"\nTest 4 Summary: Glasses ON mean={mean_on:.4f} | Glasses OFF mean={mean_off:.4f} | Status: {'PASS' if t4_pass else 'FAIL'}")

        report_lines.append("\n[Test 4: Glasses Cross-Test]")
        report_lines.append(f"  Glasses ON mean distance  : {mean_on:.4f}")
        report_lines.append(f"  Glasses OFF mean distance : {mean_off:.4f}")
        report_lines.append(f"  Result                    : {'PASS' if t4_pass else 'FAIL'}")
    else:
        print("Skipping Test 4 (Glasses Cross-Test).")
        report_lines.append("\n[Test 4: Glasses Cross-Test]\n  SKIPPED")

    # -------------------------------------------------------------------------
    # Test 5 — False acceptance threshold
    # -------------------------------------------------------------------------
    print("\n--- Test 5: False Acceptance Threshold ---")
    print("NOTE: Have a DIFFERENT person sit in front of the camera.")
    print("      (Alternative: If a second person is not available, point the camera at a photo of a completely different person).")
    input("Press ENTER when ready to run 5 false-acceptance attempts...")

    t5_results = run_auth_attempts(client, username, 5, sleep_sec=2.0)
    t5_denied_count = sum(1 for res, dist, tier in t5_results if tier == 4 and dist > 0.50)

    t5_pass = t5_denied_count == 5
    if not t5_pass:
        print("\n!!! SECURITY FAILURE: Non-enrolled face was NOT rejected as Tier 4 Denied !!!")
    else:
        print("\nTest 5 Summary: ALL 5/5 attempts correctly DENIED (d > 0.50). PASS!")

    report_lines.append("\n[Test 5: False Acceptance Threshold]")
    for idx, (res, dist, tier) in enumerate(t5_results, 1):
        report_lines.append(f"  Attempt {idx}: status={res:<10} dist={dist:.4f} tier={tier_to_str(tier)}")
    report_lines.append(f"  Denied Count: {t5_denied_count}/5 | Result: {'PASS' if t5_pass else 'SECURITY FAILURE'}")

    # -------------------------------------------------------------------------
    # Report File Output
    # -------------------------------------------------------------------------
    today_str = datetime.now().strftime("%Y%m%d")
    report_file = os.path.join(os.path.dirname(__file__), f"recognition_report_{today_str}.txt")
    with open(report_file, "w") as f:
        f.write("\n".join(report_lines) + "\n")

    print(f"\n==========================================================")
    print(f"Full accuracy report saved to: {report_file}")
    print(f"Overall Biometric Tests Result: {'PASS' if (t1_pass and t4_pass and t5_pass) else 'FAIL'}")
    print(f"==========================================================")

if __name__ == "__main__":
    main()
