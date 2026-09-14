#!/usr/bin/env bash
set -e

echo "=== Sentinel Face ID Clean Uninstaller ==="

if [ "$EUID" -ne 0 ]; then
    echo "Error: Must be run as root. Usage: sudo ./uninstall.sh"
    exit 1
fi

echo "[1/7] Stopping and removing systemd service..."
if systemctl is-active --quiet sentinel 2>/dev/null; then
    systemctl stop sentinel || true
fi
if systemctl is-enabled --quiet sentinel 2>/dev/null; then
    systemctl disable sentinel || true
fi
rm -f /etc/systemd/system/sentinel.service
rm -f /etc/systemd/system/greetd.service.d/sentinel.conf
rmdir /etc/systemd/system/greetd.service.d/ 2>/dev/null || true
systemctl daemon-reload

echo "[2/7] Removing daemon binary & PAM module..."
rm -f /usr/local/bin/sentinel-daemon
rm -f /usr/lib64/security/pam_sentinel.so

echo "[3/7] Cleaning up PAM configurations..."
PAM_FILES=(
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
)
for pam_file in "${PAM_FILES[@]}"; do
    if [ -f "$pam_file" ] && grep -q "pam_sentinel" "$pam_file"; then
        echo "Removing pam_sentinel entries from $pam_file..."
        sed -i '/pam_sentinel\.so/d' "$pam_file"
    fi
done

echo "[4/7] Removing DBus policy and PolicyKit rules..."
rm -f /etc/dbus-1/system.d/com.sentinel.Sentinel.conf
rm -f /usr/share/polkit-1/actions/com.sentinel.policy
systemctl reload dbus || true

echo "[5/7] Uninstalling Python package..."
pip3 uninstall -y sentinel-py sentinel &>/dev/null || true

echo "[6/7] Removing cache models & config..."
rm -rf /var/cache/sentinel
rm -rf /etc/sentinel

echo "[7/7] Uninstallation complete!"
echo "User data in /var/lib/sentinel/ NOT removed. Delete manually if desired."
