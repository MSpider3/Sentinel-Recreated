import os
import sys
import getpass
import argparse
import json
import time
from sentinel_py.dbus_client import SentinelDBusClient
from sentinel_py.enroll import EnrollmentWizard, ask_glasses

def get_current_user() -> str:
    return os.environ.get("SUDO_USER") or os.environ.get("USER") or getpass.getuser()

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
    username = args.username or get_current_user()
    try:
        success = client.remove_user(username)
        if success:
            print(f"Successfully removed biometric gallery for user '{username}'.")
        else:
            print(f"Failed or user '{username}' not found.")
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
    username = args.username or get_current_user()
    print(f"=== Sentinel One-Shot Diagnostic Authentication ===")
    print(f"Target User: {username}")
    print("Look at the camera...")

    start_t = time.time()
    try:
        res, dist, tier = client.authenticate(username, {})
        elapsed = time.time() - start_t
        print("\n=======================================================")
        print(f"AUTHENTICATION RESULT: {res}")
        print(f"Security Tier:        {tier}")
        print(f"Scores (audit log):   {client.last_auth_scores(start_t)}")
        print(f"Total Response Time:  {elapsed:.2f}s")
        print("=======================================================")
    except Exception as e:
        print(f"\nError during DBus Authenticate call: {e}")
        sys.exit(1)

def cmd_enroll(client: SentinelDBusClient, args):
    username = args.username or get_current_user()
    if os.geteuid() == 0 and os.environ.get("SUDO_USER"):
        # root is not allowed to open windows on the user's desktop, so the
        # camera preview would crash. The daemon asks for the administrator
        # password itself when enrollment starts.
        print("Run this without sudo:  sentinel enroll " + username)
        print("You will be asked for the administrator password in a dialog.")
        sys.exit(1)
    # No flag given: ask, rather than silently assuming "no glasses".
    glasses = args.glasses if args.glasses is not None else ask_glasses()
    wizard = EnrollmentWizard(username=username, glasses=glasses)
    success = wizard.run()
    sys.exit(0 if success else 1)

def cmd_dashboard(client: SentinelDBusClient, args):
    from sentinel_py.tui.app import SentinelApp
    app = SentinelApp()
    app.run()

def cmd_greeter_info(client: SentinelDBusClient, args):
    info = None
    if client is not None:
        try:
            raw_json = client.iface.GetGreeterInfo()
            info = json.loads(str(raw_json))
        except Exception:
            pass

    if not info:
        from sentinel_py.greeter_info import detect_greeter
        info = detect_greeter()

    if getattr(args, "json", False):
        print(json.dumps(info, indent=2))
        return

    greeter_name = info.get("greeter_name", "Unknown")
    pam_service_path = info.get("pam_service_path", "/etc/pam.d/greetd")
    sentinel_pam_configured = info.get("sentinel_pam_configured", False)
    has_tab_trigger = info.get("has_tab_trigger", False)
    has_face_pam_icon = info.get("has_face_pam_icon", False)
    setup_hint = info.get("setup_hint", "")

    pam_status_str = "✓ pam_sentinel.so configured" if sentinel_pam_configured else f"✗ Not configured in {pam_service_path}"
    tab_status_str = "✓ Supported (upstream PR #21)" if has_tab_trigger else "✗ Not supported"
    icon_status_str = "✓ Displayed in greeter" if has_face_pam_icon else "✗ Not supported"

    print("Greeter Detection")
    print("─────────────────")
    print(f"  Active Greeter:     {greeter_name}")
    print(f"  PAM Service:        {pam_service_path}")
    print(f"  Sentinel in PAM:    {pam_status_str}")
    print(f"  Tab-key Trigger:    {tab_status_str}")
    print(f"  Face Auth Icon:     {icon_status_str}")
    print()
    if setup_hint:
        print(f"  Status: {setup_hint}")

def main():
    from sentinel_py import __version__
    parser = argparse.ArgumentParser(
        prog="sentinel",
        description="Sentinel Recreated — Biometric Face Authentication CLI & Enrollment Tool"
    )
    parser.add_argument(
        "--version",
        action="version",
        version=f"%(prog)s {__version__}"
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
    p_remove.add_argument("username", nargs="?", default=None, help="Username to remove (default: current user)")
    p_remove.set_defaults(func=cmd_remove)

    # intrusions
    p_intrusions = subparsers.add_parser("intrusions", help="List intrusion screenshots")
    p_intrusions.set_defaults(func=cmd_intrusions)

    # auth
    p_auth = subparsers.add_parser("auth", help="Trigger diagnostic one-shot authentication")
    p_auth.add_argument("username", nargs="?", default=None, help="Username to test (default: current user)")
    p_auth.set_defaults(func=cmd_auth)

    # enroll
    p_enroll = subparsers.add_parser("enroll", help="Run interactive Face ID enrollment wizard")
    p_enroll.add_argument("username", nargs="?", default=None, help="Username to enroll (default: current user)")
    p_glasses = p_enroll.add_mutually_exclusive_group()
    p_glasses.add_argument("--glasses", dest="glasses", action="store_true", default=None,
                           help="Enroll with and without glasses (30 vectors) without being asked")
    p_glasses.add_argument("--no-glasses", dest="glasses", action="store_false",
                           help="Enroll without the glasses pass (15 vectors) without being asked")
    p_enroll.set_defaults(func=cmd_enroll)

    # dashboard
    p_dash = subparsers.add_parser("dashboard", help="Launch Textual TUI dashboard")
    p_dash.set_defaults(func=cmd_dashboard)

    # greeter-info
    p_greeter = subparsers.add_parser("greeter-info", help="Inspect display manager and greeter integration status")
    p_greeter.add_argument("--json", action="store_true", help="Output raw JSON")
    p_greeter.set_defaults(func=cmd_greeter_info)

    args = parser.parse_args()
    try:
        client = SentinelDBusClient()
    except Exception:
        client = None
    args.func(client, args)

if __name__ == "__main__":
    main()

