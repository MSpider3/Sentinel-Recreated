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
ASSUME_YES=0
for arg in "$@"; do
    [ "$arg" = "--dry-run" ] && DRY_RUN=1
    [ "$arg" = "--yes" ] && ASSUME_YES=1
done

# Ask a yes/no question. $2 is the answer used when the user just presses
# Enter ("y" or "n"). --yes answers yes to everything.
ask() {
    [ "$ASSUME_YES" -eq 1 ] && return 0
    local reply="" default="${2:-n}" hint="[y/N]"
    [ "$default" = "y" ] && hint="[Y/n]"
    read -r -p "$1 $hint " reply || true
    case "${reply:-$default}" in y|Y|yes|YES) return 0 ;; *) return 1 ;; esac
}

# Build steps run as the user who called sudo, not as root: root often has no
# Rust toolchain on its PATH and would leave root-owned files in the checkout.
BUILD_USER="${SUDO_USER:-root}"
as_builder() {
    if [ "$BUILD_USER" = "root" ]; then
        bash -lc "$1"
    else
        sudo -u "$BUILD_USER" -H bash -lc "cd '$PWD' && $1"
    fi
}

if [ "$DRY_RUN" -eq 1 ]; then
    echo "=== Sentinel Face ID Setup (DRY RUN — no files will be modified) ==="
else
    echo "=== Sentinel Face ID Automated Setup ==="
fi

# ---- Preflight check ---------------------------------------
if [ "$EUID" -ne 0 ]; then
    echo "Error: Must be run as root. Usage: sudo ./setup.sh [--dry-run] [--yes]"
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
        grep -E "(^|[[:space:]])(gdm|sddm|greetd|lightdm|ly)\.service" | \
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
                python3-dbus python3-gobject wget unzip
            ;;
        ubuntu|debian|linuxmint|pop)
            apt-get install -y \
                libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
                gstreamer1.0-plugins-good gstreamer1.0-pipewire \
                libpam0g-dev libdbus-1-dev meson ninja-build pkg-config \
                python3-dbus python3-gi python3-venv wget unzip
            ;;
        arch|manjaro|endeavouros)
            pacman -S --noconfirm \
                gstreamer gst-plugins-base gst-plugins-good \
                pam dbus meson ninja pkg-config \
                python-dbus python-gobject wget unzip
            ;;
        *)
            echo "WARNING: Unknown distro '$DISTRO_ID'."
            echo "Install manually: gstreamer, pam-devel, dbus-devel, meson, ninja, python3-dbus, python3-gobject"
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
    if grep -q "pam_sentinel" "$PAM_FILE"; then
        echo "  Configured: $PAM_FILE"
    else
        echo "  WARNING: $PAM_FILE has no line starting with 'auth' — left unchanged."
        echo "  Add this line by hand above its auth lines:  $SENTINEL_LINE"
    fi
}

# Remove the sentinel line from a PAM file (keeps a backup). Used when the
# user says no to a service that an earlier run had enabled.
remove_pam_line() {
    local PAM_FILE="$1"
    [ -f "$PAM_FILE" ] && grep -q "pam_sentinel" "$PAM_FILE" || return 0
    cp "$PAM_FILE" "${PAM_FILE}.bak.$(date +%Y%m%d)"
    sed -i '/pam_sentinel\.so/d' "$PAM_FILE"
    echo "  Removed face unlock from: $PAM_FILE"
}

# Decide whether a service should use face unlock. If it already does, the
# question is whether to keep it (default yes); otherwise whether to turn it
# on (default no). $1 = what to call it, remaining args = its PAM files.
wants_face_unlock() {
    local label="$1"; shift
    local f
    for f in "$@"; do
        if [ -f "$f" ] && grep -q "pam_sentinel" "$f"; then
            ask "Face unlock is already on for $label. Keep it?" y
            return
        fi
    done
    ask "Use face unlock for $label?" n
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

    # Face recognition with a normal webcam is a convenience, not a strong
    # lock: a good photo or video of you may pass. The lock screen is always
    # set up; sudo and the login screen are your choice.
    local LOGIN_FILES=()
    case "$DM" in
        gdm)     LOGIN_FILES=("/etc/pam.d/gdm-password") ;;
        sddm)    LOGIN_FILES=("/etc/pam.d/sddm") ;;
        greetd)  LOGIN_FILES=("/etc/pam.d/greetd") ;;
        lightdm) LOGIN_FILES=("/etc/pam.d/lightdm") ;;
    esac

    if [ "$DRY_RUN" -eq 1 ]; then
        echo "  [dry-run] Would ask whether to use face unlock for sudo (/etc/pam.d/sudo)"
        echo "  [dry-run] and for the login screen (${LOGIN_FILES[*]:-not detected})."
    else
        if wants_face_unlock "sudo (root commands)" "/etc/pam.d/sudo"; then
            inject_pam_line "/etc/pam.d/sudo"
        else
            remove_pam_line "/etc/pam.d/sudo"
            echo "  sudo: face unlock off."
        fi

        if [ "${#LOGIN_FILES[@]}" -eq 0 ]; then
            echo "  WARNING: Could not detect display manager. Login screen PAM not configured."
        elif wants_face_unlock "the login screen ($DM)" "${LOGIN_FILES[@]}"; then
            for f in "${LOGIN_FILES[@]}"; do inject_pam_line "$f"; done
        else
            for f in "${LOGIN_FILES[@]}"; do remove_pam_line "$f"; done
            echo "  Login screen: face unlock off."
            # On GNOME the lock screen uses the same PAM file as the login screen.
            [ "$DM" = "gdm" ] && echo "  NOTE: the GNOME lock screen shares gdm-password, so it is off as well."
        fi
    fi

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

as_builder "command -v cargo" &>/dev/null || { echo "Error: Rust toolchain (cargo) not found for user '$BUILD_USER'. Install from https://rustup.rs"; exit 1; }
python3 -c "import sys; assert sys.version_info >= (3,11)" || { echo "Error: Python 3.11+ required"; exit 1; }

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

# The daemon runs these files as root, so each one must be exactly the file
# this release was tested with.
verify_model() {
    local path="$1" expected="$2" actual
    actual=$(sha256sum "$path" | awk '{print $1}')
    if [ "$actual" != "$expected" ]; then
        echo "ERROR: checksum mismatch for $path"
        echo "  expected $expected"
        echo "  got      $actual"
        echo "Delete the file and run setup again. If it still fails, do not use it."
        exit 1
    fi
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

# Anti-spoof: two models that look at the face at different zoom levels.
download_if_missing "$MODEL_DIR/MiniFASNetV2.onnx" \
    "https://github.com/yakhyo/face-anti-spoofing/releases/download/weights/MiniFASNetV2.onnx"
download_if_missing "$MODEL_DIR/MiniFASNetV1SE.onnx" \
    "https://github.com/yakhyo/face-anti-spoofing/releases/download/weights/MiniFASNetV1SE.onnx"

verify_model "$MODEL_DIR/scrfd_500m_kps.onnx" "5e4447f50245bbd7966bd6c0fa52938c61474a04ec7def48753668a9d8b4ea3a"
verify_model "$MODEL_DIR/mobile_facenet.onnx" "9cc6e4a75f0e2bf0b1aed94578f144d15175f357bdc05e815e5c4a02b319eb4f"
verify_model "$MODEL_DIR/MiniFASNetV2.onnx"   "b32929adc2d9c34b9486f8c4c7bc97c1b69bc0ea9befefc380e4faae4e463907"
verify_model "$MODEL_DIR/MiniFASNetV1SE.onnx" "ebab7f90c7833fbccd46d3a555410e78d969db5438e169b6524be444862b3676"

chmod 644 "$MODEL_DIR"/*.onnx
echo "Models ready."

# [3/10] Build Rust daemon
echo "[3/10] Building Rust daemon..."
as_builder "cargo build --release --package sentinel-core"
install -m 755 target/release/sentinel-core /usr/local/bin/sentinel-daemon
echo "Daemon installed."

# [4/10] Build C PAM module
echo "[4/10] Building C PAM module..."
as_builder "cd pam-sentinel && (meson setup build --wipe 2>/dev/null || meson setup build) && ninja -C build"
install_pam_module
echo "PAM module installed."

# [5/10] Install Python CLI
echo "[5/10] Installing Python CLI..."
# Own virtual environment: no pip changes to the system Python, and no link
# back to this source folder. dbus and GLib bindings come from distro packages.
VENV="/opt/sentinel/venv"
python3 -m venv --system-site-packages "$VENV"
"$VENV/bin/pip" install --quiet opencv-python numpy textual
# Build from a copy so pip leaves no root-owned build files in this folder.
PY_SRC=$(mktemp -d)
cp -r pyproject.toml README.md sentinel_py "$PY_SRC/"
"$VENV/bin/pip" install --quiet --no-deps "$PY_SRC"
rm -rf "$PY_SRC"
ln -sf "$VENV/bin/sentinel" /usr/local/bin/sentinel
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
echo "  Models      : $(ls /var/cache/sentinel/models/*.onnx 2>/dev/null | wc -l)/4 present"
echo "  Daemon      : $(systemctl is-active sentinel)"
echo "  PAM (sudo)  : $(grep -c pam_sentinel /etc/pam.d/sudo 2>/dev/null || echo 0) line(s) in /etc/pam.d/sudo"
echo "  CLI         : $(command -v sentinel && sentinel --version 2>/dev/null || echo 'not found')"
echo ""
echo "Next step: enroll your face with: sentinel enroll \$USER"
