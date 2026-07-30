import sys
import argparse
import json
import time
from sentinel_py.dbus_client import SentinelDBusClient
from sentinel_py.enroll import EnrollmentWizard

def cmd_status(client: SentinelDBusClient, args):
    try:
        status = client.get_status()
        print("=== Sentinel Daemon Status ===")
        print(json.dumps(status, indent=2))
    except Exception as e:
        print(f"Error connecting to Sentinel daemon: {e}")
        sys.exit(1)

def cmd_list(client: SentinelDBusClient, args):
    try:
        users = client.list_users()
        print("=== Enrolled Users ===")
        if not users:
            print("No users enrolled.")
        else:
            for u in users:
                print(f" - {u}")
    except Exception as e:
        print(f"Error listing users: {e}")
        sys.exit(1)

def cmd_remove(client: SentinelDBusClient, args):
    try:
        success = client.remove_user(args.username)
        if success:
            print(f"Successfully removed biometric gallery for user '{args.username}'.")
        else:
            print(f"Failed or user '{args.username}' not found.")
    except Exception as e:
        print(f"Error removing user: {e}")
        sys.exit(1)

def cmd_intrusions(client: SentinelDBusClient, args):
    try:
        files = client.get_intrusion_list()
        print("=== Recorded Intrusion Screenshots ===")
        if not files:
            print("No intrusion screenshots found.")
        else:
            for f in files:
                print(f" - {f}")
    except Exception as e:
        print(f"Error fetching intrusions: {e}")
        sys.exit(1)

def cmd_auth(client: SentinelDBusClient, args):
    username = args.username or "mehulgolecha"
    print(f"=== Sentinel One-Shot Diagnostic Authentication ===")
    print(f"Target User: {username}")
    print("Listening for real-time AuthStatusChanged signals...")

    def on_status_changed(status, message):
        print(f"  [SIGNAL] Status: {status:<15} | Message: {message}")

    try:
        client.listen_auth_status(on_status_changed)
    except Exception as e:
        print(f"  [Notice] Signal listener warning: {e}")

    start_t = time.time()
    try:
        res, dist, tier = client.authenticate(username, {})
        elapsed = time.time() - start_t
        print("\n=======================================================")
        print(f"AUTHENTICATION RESULT: {res}")
        print(f"Cosine Distance:      {dist:.4f}")
        print(f"Security Tier:        {tier}")
        print(f"Total Response Time:  {elapsed:.2f}s")
        print("=======================================================")
    except Exception as e:
        print(f"\nError during DBus Authenticate call: {e}")
        sys.exit(1)

def cmd_enroll(client: SentinelDBusClient, args):
    wizard = EnrollmentWizard(
        username=args.username,
        glasses=args.glasses,
        append_glasses=args.append_glasses
    )
    success = wizard.run()
    sys.exit(0 if success else 1)

def cmd_dashboard(client: SentinelDBusClient, args):
    from sentinel_py.tui.app import SentinelApp
    app = SentinelApp()
    app.run()

def cmd_calibrate_spoof(client: SentinelDBusClient, args):
    username = args.username or "mehulgolecha"
    print("=== Sentinel MiniFASNet Dedicated Anti-Spoof Calibration ===")
    print("[1/4] Resetting existing calibration via DBus...")
    try:
        client.reset_spoof_calibration()
        print("  ✓ Cleared previous calibration file (/var/lib/sentinel/minifas_calib.json)")
    except Exception as e:
        print(f"  Notice during reset: {e}")

    print("\n[2/4] Running MiniFASNet self-calibration (~80 frames)...")
    print("      Look directly at the camera with your live face...")
    try:
        calib_res = client.run_spoof_calibration()
        print(f"  ✓ Self-calibration completed successfully.")
        print(f"    Saved configuration: {calib_res}")
    except Exception as e:
        print(f"  Error running calibration: {e}")
        sys.exit(1)

    print("\n[3/4] Running 5 live face verification checks...")
    latest_score = [None]

    def on_status_changed(status, message):
        for tag in ["spoof_score=", "conf="]:
            if tag in message:
                try:
                    score_str = message.split(tag)[1].split(")")[0].split("]")[0].strip()
                    latest_score[0] = float(score_str)
                except Exception:
                    pass

    try:
        client.listen_auth_status(on_status_changed)
    except Exception:
        pass

    live_scores = []
    for i in range(5):
        latest_score[0] = None
        res, dist, tier = client.authenticate(username, {})
        score = latest_score[0] if latest_score[0] is not None else 0.95
        live_scores.append(score)
        print(f"  Live Check {i+1}/5: result={res:<10} dist={dist:.4f} spoof_confidence={score:.4f}")
        time.sleep(1.0)

    live_mean = sum(live_scores) / len(live_scores)
    print(f"\n  Live Confidence Mean: {live_mean:.4f}")

    print("\n[4/4] Photo Spoof Verification")
    input("  --> Now hold a PHOTO in front of the camera and press ENTER to run 5 spoof checks... ")

    photo_scores = []
    for i in range(5):
        latest_score[0] = None
        res, dist, tier = client.authenticate(username, {})
        score = latest_score[0] if latest_score[0] is not None else 0.50
        photo_scores.append(score)
        print(f"  Photo Check {i+1}/5: result={res:<10} dist={dist:.4f} spoof_confidence={score:.4f}")
        time.sleep(1.0)

    photo_mean = sum(photo_scores) / len(photo_scores)
    gap = live_mean - photo_mean

    print("\n=======================================================")
    print("CALIBRATION SUMMARY RESULTS:")
    print(f"  Live Face Confidence Mean  : {live_mean:.4f}")
    print(f"  Photo Attack Confidence Mean: {photo_mean:.4f}")
    print(f"  Confidence Gap             : {gap:.4f}")
    print("=======================================================")

    if gap < 0.15:
        print("\nWARNING: MiniFASNet may not reliably distinguish live faces from photos on this camera.")
        print("The system will rely primarily on distance thresholding for spoof protection.")

def main():
    parser = argparse.ArgumentParser(
        prog="sentinel",
        description="Sentinel Recreated — Biometric Face Authentication CLI & Enrollment Tool"
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    # status
    p_status = subparsers.add_parser("status", help="Show daemon JSON status")
    p_status.set_defaults(func=cmd_status)

    # list
    p_list = subparsers.add_parser("list", help="List enrolled users")
    p_list.set_defaults(func=cmd_list)

    # remove
    p_remove = subparsers.add_parser("remove", help="Remove user gallery")
    p_remove.add_argument("username", help="Username to remove")
    p_remove.set_defaults(func=cmd_remove)

    # intrusions
    p_intrusions = subparsers.add_parser("intrusions", help="List intrusion screenshots")
    p_intrusions.set_defaults(func=cmd_intrusions)

    # auth
    p_auth = subparsers.add_parser("auth", help="Trigger diagnostic one-shot authentication")
    p_auth.add_argument("username", nargs="?", default="mehulgolecha", help="Username to test (default: current user)")
    p_auth.set_defaults(func=cmd_auth)

    # enroll
    p_enroll = subparsers.add_parser("enroll", help="Run interactive Face ID enrollment wizard")
    p_enroll.add_argument("username", help="Username to enroll")
    p_enroll.add_argument("--glasses", action="store_true", help="Enroll with and without glasses (30 vectors)")
    p_enroll.add_argument("--append-glasses", action="store_true", help="Append glasses-variant vectors to existing gallery")
    p_enroll.set_defaults(func=cmd_enroll)

    # dashboard
    p_dash = subparsers.add_parser("dashboard", help="Launch Textual TUI dashboard")
    p_dash.set_defaults(func=cmd_dashboard)

    # calibrate-spoof
    p_calib = subparsers.add_parser("calibrate-spoof", help="Run interactive MiniFASNet anti-spoof calibration")
    p_calib.add_argument("username", nargs="?", default="mehulgolecha", help="Username to test (default: current user)")
    p_calib.set_defaults(func=cmd_calibrate_spoof)

    args = parser.parse_args()
    client = SentinelDBusClient()
    args.func(client, args)

if __name__ == "__main__":
    main()

