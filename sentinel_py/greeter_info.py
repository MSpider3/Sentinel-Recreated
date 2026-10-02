"""Greeter and display manager detection for Sentinel Recreated.

Mirror of sentinel-core/src/greeter_detect.rs.
Zero external dependencies (pure Python standard library).
Works standalone even if DBus is down or daemon is stopped.
"""

import os
from pathlib import Path
from typing import Optional, Dict, Any


def is_sentinel_in_pam(pam_file: Path) -> bool:
    """Check if a PAM configuration file exists and contains an active pam_sentinel.so line."""
    if not pam_file.exists() or not pam_file.is_file():
        return False
    try:
        with open(pam_file, "r", encoding="utf-8", errors="replace") as f:
            for line in f:
                trimmed = line.strip()
                if not trimmed.startswith("#") and "pam_sentinel.so" in trimmed:
                    return True
    except Exception:
        pass
    return False


def _detect_from_proc(proc_root: Path) -> Optional[str]:
    """Inspect /proc for running display manager processes."""
    if not proc_root.exists() or not proc_root.is_dir():
        return None

    found_greetd = False
    try:
        for entry in os.scandir(proc_root):
            if not entry.name.isdigit():
                continue

            pid_dir = Path(entry.path)
            comm_file = pid_dir / "comm"
            cmdline_file = pid_dir / "cmdline"

            comm = ""
            if comm_file.exists():
                try:
                    comm = comm_file.read_text(encoding="utf-8", errors="replace").strip()
                except Exception:
                    pass

            cmdline = ""
            if cmdline_file.exists():
                try:
                    cmdline = cmdline_file.read_text(encoding="utf-8", errors="replace")
                except Exception:
                    pass

            if "dms-greeter" in comm or "dms-greeter" in cmdline:
                return "DankGreeter"

            if comm == "greetd" or "greetd" in cmdline:
                found_greetd = True
            elif comm.startswith("gdm") or "/gdm" in cmdline:
                return "Gdm"
            elif comm.startswith("sddm") or "sddm" in cmdline:
                return "Sddm"
            elif comm.startswith("lightdm") or "lightdm" in cmdline:
                return "LightDm"
    except Exception:
        pass

    if found_greetd:
        return "Greetd"
    return None


def _detect_from_systemd(root: Path) -> Optional[str]:
    """Inspect systemd display-manager.service symlink."""
    dm_symlink = root / "etc" / "systemd" / "system" / "display-manager.service"
    if dm_symlink.is_symlink():
        try:
            target = str(os.readlink(dm_symlink)).lower()
            if "greetd" in target:
                return "Greetd"
            elif "gdm" in target:
                return "Gdm"
            elif "sddm" in target:
                return "Sddm"
            elif "lightdm" in target:
                return "LightDm"
        except Exception:
            pass
    return None


def _is_greetd_using_dms(root: Path) -> bool:
    """Check if /etc/greetd/config.toml references dms-greeter."""
    config_file = root / "etc" / "greetd" / "config.toml"
    if config_file.exists() and config_file.is_file():
        try:
            content = config_file.read_text(encoding="utf-8", errors="replace")
            return "dms-greeter" in content
        except Exception:
            pass
    return False


def detect_greeter(root: str = "/") -> Dict[str, Any]:
    """Detect active display manager and its Sentinel compatibility.

    Returns a dictionary structured identically to Rust's GreeterInfo.
    """
    root_path = Path(root)
    proc_path = root_path / "proc"

    detected_type: Optional[str] = None
    if proc_path.exists():
        detected_type = _detect_from_proc(proc_path)

    if detected_type is None:
        detected_type = _detect_from_systemd(root_path)

    # Refine Greetd -> DankGreeter if config.toml specifies dms-greeter
    if detected_type in ("Greetd", None) and _is_greetd_using_dms(root_path):
        detected_type = "DankGreeter"

    # If still None, check existing pam.d files
    if detected_type is None:
        if (root_path / "etc" / "pam.d" / "greetd").exists():
            detected_type = "Greetd"
        elif (root_path / "etc" / "pam.d" / "gdm-password").exists():
            detected_type = "Gdm"
        elif (root_path / "etc" / "pam.d" / "sddm").exists():
            detected_type = "Sddm"
        elif (root_path / "etc" / "pam.d" / "lightdm").exists():
            detected_type = "LightDm"

    greeter_type = detected_type or "Unknown"

    display_names = {
        "DankGreeter": "Dank Greeter (dms-greeter)",
        "Greetd": "Greetd (Generic)",
        "Gdm": "GDM (GNOME Display Manager)",
        "Sddm": "SDDM (Simple Desktop Display Manager)",
        "LightDm": "LightDM",
        "Unknown": "Unknown / Custom Lock Screen",
    }
    greeter_name = display_names.get(greeter_type, "Unknown / Custom Lock Screen")

    default_pam_services = {
        "DankGreeter": "greetd",
        "Greetd": "greetd",
        "Gdm": "gdm-password",
        "Sddm": "sddm",
        "LightDm": "lightdm",
        "Unknown": "system-auth",
    }
    pam_service = default_pam_services.get(greeter_type, "system-auth")
    pam_service_path = root_path / "etc" / "pam.d" / pam_service
    sentinel_pam_configured = is_sentinel_in_pam(pam_service_path)

    has_tab_trigger = greeter_type == "DankGreeter"
    has_face_pam_icon = greeter_type == "DankGreeter"

    setup_hints = {
        "DankGreeter": (
            "Native Tab-key face authentication and face icon active (upstream PR #21). Press Tab on an empty password field to scan face."
            if sentinel_pam_configured
            else "Dank Greeter detected, but pam_sentinel.so is missing from /etc/pam.d/greetd. Add 'auth sufficient pam_sentinel.so' to enable face authentication."
        ),
        "Greetd": (
            "Generic greetd active. Face authentication is triggered via the PAM conversation prompt on Enter."
            if sentinel_pam_configured
            else "Configure pam_sentinel.so in /etc/pam.d/greetd to enable face authentication."
        ),
        "Gdm": (
            "GDM does not support custom key triggers or biometric icons. Press Enter on an empty password field to trigger face scan via PAM conversation."
            if sentinel_pam_configured
            else "Configure pam_sentinel.so in /etc/pam.d/gdm-password to enable face authentication."
        ),
        "Sddm": (
            "SDDM active. Face authentication is triggered via the PAM conversation prompt on Enter."
            if sentinel_pam_configured
            else "Configure pam_sentinel.so in /etc/pam.d/sddm to enable face authentication."
        ),
        "LightDm": (
            "LightDM active. Face authentication is triggered via the PAM conversation prompt on Enter."
            if sentinel_pam_configured
            else "Configure pam_sentinel.so in /etc/pam.d/lightdm to enable face authentication."
        ),
        "Unknown": (
            "Custom display manager or lock screen. Face authentication works via standard PAM conversation."
            if sentinel_pam_configured
            else "Configure pam_sentinel.so in your display manager's PAM service file (e.g., /etc/pam.d/login or /etc/pam.d/system-auth)."
        ),
    }
    setup_hint = setup_hints.get(greeter_type)

    return {
        "greeter_type": greeter_type,
        "greeter_name": greeter_name,
        "has_tab_trigger": has_tab_trigger,
        "has_face_pam_icon": has_face_pam_icon,
        "pam_service": pam_service,
        "pam_service_path": str(pam_service_path),
        "sentinel_pam_configured": sentinel_pam_configured,
        "setup_hint": setup_hint,
    }
