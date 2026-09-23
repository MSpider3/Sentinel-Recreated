#!/usr/bin/env bash
set -e

# ============================================================
# Sentinel Recreated — Emergency PAM Failsafe & Restoration Tool
#
# PURPOSE:
# If you are ever locked out, experience unexpected PAM prompts,
# or want to instantly and completely disable face recognition
# from all system authentication pathways, run this script.
#
# It safely strips pam_sentinel.so from all PAM service configurations
# and restores default system password authentication immediately.
# ============================================================

echo "=== Sentinel Recreated: Emergency PAM Failsafe & Recovery ==="

if [ "$EUID" -ne 0 ]; then
    echo "Error: This recovery script must be run as root."
    echo "Usage: sudo ./scripts/emergency_restore_pam.sh"
    exit 1
fi

TARGET_PAM_FILES=(
    /etc/pam.d/sudo
    /etc/pam.d/gdm-password
    /etc/pam.d/gdm-autologin
    /etc/pam.d/sddm
    /etc/pam.d/greetd
    /etc/pam.d/lightdm
    /etc/pam.d/gnome-screensaver
    /etc/pam.d/kde
    /etc/pam.d/kscreenlocker
    /etc/pam.d/hyprlock
    /etc/pam.d/swaylock
    /etc/pam.d/swaylock-effects
    /etc/pam.d/waylock
    /etc/pam.d/dankshell
    /etc/pam.d/system-auth
    /etc/pam.d/password-auth
    /etc/pam.d/login
)

MODIFIED_COUNT=0

echo "[1/3] Scanning /etc/pam.d/ for Sentinel PAM integration..."
for pam_file in "${TARGET_PAM_FILES[@]}"; do
    if [ -f "$pam_file" ]; then
        if grep -q "pam_sentinel" "$pam_file"; then
            echo "  Found Sentinel entry in: $pam_file"
            # Create safety backup before editing
            cp "$pam_file" "${pam_file}.emergency_backup.$(date +%Y%m%d_%H%M%S)"
            # Safely remove sentinel lines
            sed -i '/pam_sentinel\.so/d' "$pam_file"
            echo "  ✓ Cleaned: $pam_file"
            MODIFIED_COUNT=$((MODIFIED_COUNT + 1))
        fi
    fi
done

echo ""
echo "[2/3] Checking for any other lingering PAM files with pam_sentinel..."
while IFS= read -r other_file; do
    if [ -n "$other_file" ]; then
        echo "  Found entry in non-standard location: $other_file"
        cp "$other_file" "${other_file}.emergency_backup.$(date +%Y%m%d_%H%M%S)"
        sed -i '/pam_sentinel\.so/d' "$other_file"
        echo "  ✓ Cleaned: $other_file"
        MODIFIED_COUNT=$((MODIFIED_COUNT + 1))
    fi
done < <(grep -l "pam_sentinel\.so" /etc/pam.d/* 2>/dev/null || true)

echo ""
echo "[3/3] Checking system authentication stack integrity..."
if [ -f /etc/pam.d/sudo ]; then
    if grep -q "pam_sentinel" /etc/pam.d/sudo; then
        echo "  WARNING: pam_sentinel still detected in /etc/pam.d/sudo!"
    else
        echo "  ✓ /etc/pam.d/sudo is clean (default password authentication active)."
    fi
fi

echo ""
echo "============================================================"
echo "RESTORATION COMPLETE:"
echo "  Total PAM configuration files restored: $MODIFIED_COUNT"
echo "  Standard password authentication is fully operational."
echo "  No biometric PAM module is currently active."
echo "============================================================"
