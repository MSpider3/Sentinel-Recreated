#!/usr/bin/env bash
set -e

# ============================================================
# Sentinel Face ID — Automated Setup
# Supports: Fedora/RHEL/CentOS, Ubuntu/Debian/Mint, Arch/Manjaro
# Display Managers: GDM, SDDM, greetd, LightDM
# Lock Screens: gnome-screensaver (via GDM), kscreenlocker, hyprlock,
#               swaylock, waylock, DankMaterialShell (dankshell)
# ============================================================

# ---- Flag parsing ------------------------------------------
DRY_RUN=0
for arg in "$@"; do
    [ "$arg" = "--dry-run" ] && DRY_RUN=1
done

if [ "$DRY_RUN" -eq 1 ]; then
    echo "=== Sentinel Face ID Setup (DRY RUN — no files will be modified) ==="
else
    echo "=== Sentinel Face ID Automated Setup ==="
fi

# ---- Preflight check ---------------------------------------
if [ "$EUID" -ne 0 ]; then
    echo "Error: Must be run as root. Usage: sudo ./setup.sh [--dry-run]"
    exit 1
fi

# ============================================================
# ENVIRONMENT DETECTION FUNCTIONS
# ============================================================

detect_distro() {
    if [ -f /etc/os-release ]; then
        . /etc/os-release
        DISTRO_ID="${ID:-unknown}"
        DISTRO_LIKE="${ID_LIKE:-}"
        DISTRO_VERSION="${VERSION_ID:-}"
    else
        DISTRO_ID="unknown"
        DISTRO_LIKE=""
        DISTRO_VERSION=""
    fi
    echo "Detected distro: $DISTRO_ID $DISTRO_VERSION${DISTRO_LIKE:+ (like: $DISTRO_LIKE)}"
}

detect_display_manager() {
    DM=""
    # Primary: check active systemd services
    local DM_SERVICE
    DM_SERVICE=$(systemctl list-units --type=service --state=active 2>/dev/null | \
        grep -E "gdm\.service|sddm\.service|greetd\.service|lightdm\.service|ly\.service" | \
        awk '{print $1}' | head -1)

    case "$DM_SERVICE" in
        gdm*)     DM="gdm" ;;
        sddm*)    DM="sddm" ;;
        greetd*)  DM="greetd" ;;
        lightdm*) DM="lightdm" ;;
        ly*)      DM="ly" ;;
    esac

    # Fallback: check /etc/pam.d file presence
    [ -z "$DM" ] && [ -f /etc/pam.d/gdm-password ] && DM="gdm"
    [ -z "$DM" ] && [ -f /etc/pam.d/sddm ]         && DM="sddm"
    [ -z "$DM" ] && [ -f /etc/pam.d/greetd ]        && DM="greetd"
    [ -z "$DM" ] && [ -f /etc/pam.d/lightdm ]       && DM="lightdm"

    echo "Detected display manager: ${DM:-unknown}"
}

detect_lock_screen() {
    LOCK_SCREEN=""
    LOCK_PAM_FILE=""

    # DMS MUST be checked first — it coexists with swaylock on many systems
    local TARGET_HOME
    TARGET_HOME=$(eval echo "~${SUDO_USER:-$USER}")
    if [ -d "$TARGET_HOME/.config/DankMaterialShell" ]; then
        LOCK_SCREEN="dankshell"
        LOCK_PAM_FILE="/etc/pam.d/dankshell"
        echo "Detected lock screen: dankshell (PAM: $LOCK_PAM_FILE)"
        return 0
    fi

    # Wayland compositors — check in priority order
    command -v hyprlock  &>/dev/null && \
        LOCK_SCREEN="hyprlock"  && LOCK_PAM_FILE="/etc/pam.d/hyprlock"  && \
        echo "Detected lock screen: hyprlock (PAM: $LOCK_PAM_FILE)" && return 0
    command -v swaylock  &>/dev/null && \
        LOCK_SCREEN="swaylock"  && LOCK_PAM_FILE="/etc/pam.d/swaylock"  && \
        echo "Detected lock screen: swaylock (PAM: $LOCK_PAM_FILE)" && return 0
    command -v waylock   &>/dev/null && \
        LOCK_SCREEN="waylock"   && LOCK_PAM_FILE="/etc/pam.d/waylock"   && \
        echo "Detected lock screen: waylock (PAM: $LOCK_PAM_FILE)" && return 0

    # GNOME — lock screen handled by gdm-password (no separate PAM file needed)
    [ "$XDG_CURRENT_DESKTOP" = "GNOME" ] && \
        LOCK_SCREEN="gnome" && LOCK_PAM_FILE="" && \
        echo "Detected lock screen: GNOME (via gdm-password, no separate PAM file needed)" && return 0

    # KDE — detect via XDG_CURRENT_DESKTOP or kscreenlocker binary
    if [ "$XDG_CURRENT_DESKTOP" = "KDE" ] || command -v kscreenlocker_greet &>/dev/null; then
        LOCK_SCREEN="kscreenlocker"
        LOCK_PAM_FILE=""  # handled per-distro in configure_pam()
        echo "Detected lock screen: kscreenlocker (PAM: kde or kscreenlocker — distro dependent)"
        return 0
    fi

    echo "Detected lock screen: unknown"
}

# ============================================================
# DEPENDENCY INSTALLATION
# ============================================================

install_system_deps() {
    echo "[1/10] Installing system dependencies for: $DISTRO_ID"
    if [ "$DRY_RUN" -eq 1 ]; then
        echo "  [dry-run] Would install packages for distro: $DISTRO_ID"
        return 0
    fi

    # Normalise: treat ID_LIKE families the same as the primary ID
    local distro_family="$DISTRO_ID"
    case "$DISTRO_LIKE" in
        *fedora*|*rhel*) distro_family="fedora" ;;
        *debian*)        distro_family="ubuntu" ;;
        *arch*)          distro_family="arch" ;;
    esac

    case "$distro_family" in
        fedora|rhel|centos)
            dnf install -y --skip-unavailable \
                gstreamer1-devel gstreamer1-plugins-base-devel \
                gstreamer1-plugins-good pipewire-gstreamer \
                pam-devel dbus-devel meson ninja-build pkg-config \
                opencv-devel wget unzip
            ;;
        ubuntu|debian|linuxmint|pop)
            apt-get install -y \
                libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
                gstreamer1.0-plugins-good gstreamer1.0-pipewire \
                libpam0g-dev libdbus-1-dev meson ninja-build pkg-config \
                libopencv-dev wget unzip
            ;;
        arch|manjaro|endeavouros)
            pacman -S --noconfirm \
                gstreamer gst-plugins-base gst-plugins-good \
                pam dbus meson ninja pkg-config \
                opencv wget unzip
            ;;
        *)
            echo "WARNING: Unknown distro '$DISTRO_ID'."
            echo "Install manually: gstreamer, pam-devel, dbus-devel, meson, ninja, opencv"
            ;;
    esac
}

# ============================================================
# PAM MODULE INSTALLATION (path is distro-dependent)
# ============================================================

install_pam_module() {
    # Detect correct PAM security module directory
    local PAM_MODULE_DIR=""
    if   [ -d /usr/lib64/security ];                   then PAM_MODULE_DIR="/usr/lib64/security"                   # Fedora/RHEL
    elif [ -d /usr/lib/x86_64-linux-gnu/security ];    then PAM_MODULE_DIR="/usr/lib/x86_64-linux-gnu/security"    # Ubuntu/Debian x86_64
    elif [ -d /usr/lib/aarch64-linux-gnu/security ];   then PAM_MODULE_DIR="/usr/lib/aarch64-linux-gnu/security"   # Ubuntu/Debian ARM
    elif [ -d /usr/lib/security ];                     then PAM_MODULE_DIR="/usr/lib/security"                     # Arch/Manjaro
    else
        # Last resort: ask libpam via pkg-config
        PAM_MODULE_DIR=$(pkg-config --variable=securedir libpam 2>/dev/null || echo "/usr/lib/security")
    fi

    if [ "$DRY_RUN" -eq 1 ]; then
        echo "  [dry-run] Would install pam-sentinel/build/pam_sentinel.so → $PAM_MODULE_DIR/pam_sentinel.so"
        return 0
    fi

    install -m 755 pam-sentinel/build/pam_sentinel.so "$PAM_MODULE_DIR/pam_sentinel.so"
    echo "PAM module installed to: $PAM_MODULE_DIR/pam_sentinel.so"
}

# ============================================================
# PAM CONFIGURATION HELPERS
# ============================================================

# Idempotently inject sentinel line before the first 'auth' entry in a PAM file.
# Skips silently if file does not exist.
inject_pam_line() {
    local PAM_FILE="$1"
    local SENTINEL_LINE="auth       sufficient    pam_sentinel.so"

    [ ! -f "$PAM_FILE" ] && return 0

    if grep -q "pam_sentinel" "$PAM_FILE"; then
        echo "  Already configured: $PAM_FILE"
        return 0
    fi

    if [ "$DRY_RUN" -eq 1 ]; then
        echo "  [dry-run] Would inject into: $PAM_FILE"
        return 0
    fi

    cp "$PAM_FILE" "${PAM_FILE}.bak.$(date +%Y%m%d)"
    sed -i "0,/^auth/s//auth       sufficient    pam_sentinel.so\nauth/" "$PAM_FILE"
    echo "  Configured: $PAM_FILE"
}

# Create a minimal PAM file (Wayland lock screens that ship without one)
create_pam_file_if_missing() {
    local PAM_FILE="$1"
    [ -f "$PAM_FILE" ] && return 0
    if [ "$DRY_RUN" -eq 1 ]; then
        echo "  [dry-run] Would create: $PAM_FILE"
        return 0
    fi
    printf '#%%PAM-1.0\nauth include system-auth\n' > "$PAM_FILE"
    echo "  Created: $PAM_FILE"
}

configure_dms_settings() {
    # Run the Python update as the invoking user to preserve file ownership
    local TARGET_USER="${SUDO_USER:-$USER}"
    local TARGET_HOME
    TARGET_HOME=$(eval echo "~$TARGET_USER")
    local DMS_SETTINGS="$TARGET_HOME/.config/DankMaterialShell/settings.json"

    [ ! -f "$DMS_SETTINGS" ] && return 0

    if [ "$DRY_RUN" -eq 1 ]; then
        echo "  [dry-run] Would update DMS settings: $DMS_SETTINGS"
        return 0
    fi

    sudo -u "$TARGET_USER" python3 - "$DMS_SETTINGS" <<'EOF'
import json, pathlib, sys
p = pathlib.Path(sys.argv[1])
try:
    s = json.loads(p.read_text())
    changed = False
    if s.get('lockPamExternallyManaged') is not False:
        s['lockPamExternallyManaged'] = False
        changed = True
    if s.get('lockPamPath') != '/etc/pam.d/dankshell':
        s['lockPamPath'] = '/etc/pam.d/dankshell'
        changed = True
    if changed:
        p.write_text(json.dumps(s, indent=2))
        print(f'  DankMaterialShell settings updated: {p}')
    else:
        print(f'  DankMaterialShell settings already correct.')
except Exception as e:
    print(f'  WARNING: Could not update DMS settings: {e}')
EOF
}

configure_pam() {
    echo "=== Configuring PAM ==="

    if [ "$DRY_RUN" -eq 1 ]; then
        echo "  Dry-run summary:"
        echo "    Display manager : ${DM:-unknown}"
        echo "    Lock screen     : ${LOCK_SCREEN:-unknown}"
        echo "    Lock PAM file   : ${LOCK_PAM_FILE:-n/a}"
        echo ""
    fi

    # Always configure sudo
    inject_pam_line "/etc/pam.d/sudo"

    # Display manager PAM files
    case "$DM" in
        gdm)
            # gdm-password covers both login and GNOME lock screen — no separate lock screen file needed
            inject_pam_line "/etc/pam.d/gdm-password"
            inject_pam_line "/etc/pam.d/gdm-autologin"
            ;;
        sddm)
            inject_pam_line "/etc/pam.d/sddm"
            ;;
        greetd)
            inject_pam_line "/etc/pam.d/greetd"
            ;;
        lightdm)
            inject_pam_line "/etc/pam.d/lightdm"
            ;;
        "")
            echo "  WARNING: Could not detect display manager. Login screen PAM not configured."
            ;;
    esac

    # Lock screen PAM files (skipped for GNOME — handled via gdm-password above)
    case "$LOCK_SCREEN" in
        gnome)
            # No action needed — gdm-password (configured above) covers GNOME lock screen too
            ;;
        kscreenlocker)
            # PAM file name varies by distro: /etc/pam.d/kde (Arch) or /etc/pam.d/kscreenlocker (Ubuntu/Kubuntu)
            inject_pam_line "/etc/pam.d/kde"
            inject_pam_line "/etc/pam.d/kscreenlocker"
            ;;
        hyprlock)
            create_pam_file_if_missing "/etc/pam.d/hyprlock"
            inject_pam_line "/etc/pam.d/hyprlock"
            ;;
        swaylock)
            inject_pam_line "/etc/pam.d/swaylock"
            inject_pam_line "/etc/pam.d/swaylock-effects"
            # Warn if swaylock was built without PAM support
            if command -v swaylock &>/dev/null; then
                if ! swaylock --help 2>&1 | grep -qi "pam"; then
                    echo "  WARNING: swaylock may not have PAM support compiled in."
                    echo "  Install from your distro's package manager:"
                    echo "    Arch:   sudo pacman -S swaylock"
                    echo "    Ubuntu: sudo apt install swaylock"
                    echo "    Fedora: sudo dnf install swaylock"
                fi
            fi
            ;;
        waylock)
            create_pam_file_if_missing "/etc/pam.d/waylock"
            inject_pam_line "/etc/pam.d/waylock"
            ;;
        dankshell)
            inject_pam_line "/etc/pam.d/dankshell"
            configure_dms_settings
            ;;
        "")
            echo "  WARNING: Could not detect lock screen. Lock screen PAM not configured."
            echo "  Manually add the following line to your lock screen's /etc/pam.d/ file:"
            echo "    auth       sufficient    pam_sentinel.so"
            ;;
    esac
}

# ============================================================
# MAIN INSTALLATION SEQUENCE
# ============================================================

# Run detection up front (needed by steps 1, 9, and 10)
detect_distro
detect_display_manager
detect_lock_screen
echo ""

if [ "$DRY_RUN" -eq 1 ]; then
    echo "=== Dry-run complete. No files were modified. ==="
    configure_pam   # still prints the planned PAM actions
    exit 0
fi

# [1/10] System dependencies
install_system_deps

command -v cargo &>/dev/null || { echo "Error: Rust toolchain (cargo) not found. Install from https://rustup.rs"; exit 1; }
python3 -c "import sys; assert sys.version_info >= (3,10)" || { echo "Error: Python 3.10+ required"; exit 1; }

# [2/10] Download ONNX models (skip if present and non-zero)
echo "[2/10] Checking ONNX models..."
MODEL_DIR="/var/cache/sentinel/models"
mkdir -p "$MODEL_DIR"

download_if_missing() {
    local path="$1" url="$2"
    [ -s "$path" ] && return 0
    echo "Downloading $(basename "$path")..."
    wget -q --show-progress -O "$path" "$url" || { echo "FAILED: $url"; exit 1; }
}

if [ ! -s "$MODEL_DIR/scrfd_500m_kps.onnx" ] || [ ! -s "$MODEL_DIR/mobile_facenet.onnx" ]; then
    TMP=$(mktemp -d)
    download_if_missing "$TMP/buffalo_sc.zip" \
        "https://github.com/deepinsight/insightface/releases/download/v0.7/buffalo_sc.zip"
    unzip -q "$TMP/buffalo_sc.zip" -d "$TMP/buffalo_sc"
    cp "$TMP/buffalo_sc/det_500m.onnx"   "$MODEL_DIR/scrfd_500m_kps.onnx"
    cp "$TMP/buffalo_sc/w600k_mbf.onnx"  "$MODEL_DIR/mobile_facenet.onnx"
    rm -rf "$TMP"
fi

download_if_missing "$MODEL_DIR/MiniFASNetV2.onnx" \
    "https://github.com/yakhyo/face-anti-spoofing/releases/download/weights/MiniFASNetV2.onnx"

chmod 644 "$MODEL_DIR"/*.onnx
echo "Models ready."

# [3/10] Build Rust daemon
echo "[3/10] Building Rust daemon..."
cargo build --release --package sentinel-core
install -m 755 target/release/sentinel-core /usr/local/bin/sentinel-daemon
echo "Daemon installed."

# [4/10] Build C PAM module
echo "[4/10] Building C PAM module..."
cd pam-sentinel
meson setup build --wipe 2>/dev/null || meson setup build
ninja -C build
cd ..
install_pam_module
echo "PAM module installed."

# [5/10] Install Python CLI
echo "[5/10] Installing Python CLI..."
pip3 install --quiet textual
pip3 install --quiet -e .
if [ -f /usr/local/sbin/sentinel ] && [ ! /usr/local/sbin/sentinel -ef /usr/local/bin/sentinel ]; then
    ln -sf /usr/local/sbin/sentinel /usr/local/bin/sentinel || true
fi
echo "CLI installed in /usr/local/bin/sentinel."

# [6/10] Create system directories
echo "[6/10] Creating system directories..."
install -d -m 700 -o root -g root /var/lib/sentinel
install -d -m 700 -o root -g root /var/lib/sentinel/users
install -d -m 700 -o root -g root /var/lib/sentinel/blacklist
install -d -m 755 -o root -g root /var/cache/sentinel/models
install -d -m 750 -o root -g root /var/log/sentinel
install -d -m 755 -o root -g root /etc/sentinel
echo "Directories created."

# [7/10] Install config (only if not present)
echo "[7/10] Installing configuration..."
[ -f /etc/sentinel/config.toml ] || install -m 644 config.toml.default /etc/sentinel/config.toml
echo "Config ready."

# [8/10] Install DBus policy and PolicyKit rules
echo "[8/10] Installing DBus policy and PolicyKit rules..."
install -m 644 packaging/com.sentinel.Sentinel.conf /etc/dbus-1/system.d/
install -m 644 packaging/com.sentinel.policy /usr/share/polkit-1/actions/
systemctl reload dbus
echo "DBus policy installed."

# [9/10] Install and enable systemd service
echo "[9/10] Installing systemd service..."
install -m 644 packaging/sentinel.service /etc/systemd/system/

# greetd ordering drop-in — only relevant when greetd is the display manager
if [ "$DM" = "greetd" ]; then
    mkdir -p /etc/systemd/system/greetd.service.d/
    install -m 644 packaging/greetd-sentinel.conf \
        /etc/systemd/system/greetd.service.d/sentinel.conf
    echo "greetd systemd ordering configured."
fi

systemctl daemon-reload
systemctl enable sentinel
systemctl restart sentinel
sleep 2
systemctl is-active sentinel && echo "Daemon running." || \
    echo "WARNING: Daemon failed to start — check: journalctl -u sentinel"

# [10/10] Configure PAM
echo "[10/10] Configuring PAM..."
configure_pam

# ============================================================
# FINAL SUMMARY
# ============================================================
echo ""
echo "=== Sentinel Face ID Installation Complete ==="
echo "  Distro      : $DISTRO_ID $DISTRO_VERSION"
echo "  Disp. Manager: ${DM:-unknown}"
echo "  Lock Screen : ${LOCK_SCREEN:-unknown}"
echo "  Models      : $(ls /var/cache/sentinel/models/*.onnx 2>/dev/null | wc -l)/3 present"
echo "  Daemon      : $(systemctl is-active sentinel)"
echo "  PAM (sudo)  : $(grep -c pam_sentinel /etc/pam.d/sudo 2>/dev/null || echo 0) line(s) in /etc/pam.d/sudo"
echo "  CLI         : $(command -v sentinel && sentinel --version 2>/dev/null || echo 'not found')"
echo ""
echo "Next step: enroll your face with: sentinel enroll \$USER"
