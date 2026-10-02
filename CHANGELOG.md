# Changelog

All notable changes to Sentinel Recreated will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.3] - 2026-10-02

### Added
- **Display Manager & Greeter Detection Engine (`greeter_detect.rs`)**: Implemented isolated detection in the Rust daemon that inspects `/proc`, systemd display-manager services, and greeter configs (`/etc/greetd/config.toml`) with zero runtime overhead or external crate dependencies.
- **Informational D-Bus API (`GetGreeterInfo`)**: Exposed read-only `com.sentinel.Sentinel.GetGreeterInfo` method returning serialized greeter detection metadata, Tab-key trigger status, face PAM icon capability, and active PAM service configuration.
- **Python Greeter Diagnostics Module (`greeter_info.py`)**: Added native Python detection mirroring the Rust detection logic, enabling offline diagnosis without requiring the D-Bus daemon to be running.
- **Greeter Inspection CLI Subcommand (`sentinel greeter-info`)**: Added `sentinel greeter-info` (with `--json` support) to check active greeter, PAM configuration status (`pam_sentinel.so`), and recommended authentication interaction mode.
- **Greeter Integration Documentation (`GREETER_INTEGRATION.md`)**: Comprehensive architectural guide covering native Dank Greeter Tab-key & icon integration (upstream PR #21), GDM/GNOME Shell interaction constraints, SDDM, Hyprlock/Swaylock setups, and the universal fail-safe PAM conversation model.

### Integration
- **Upstream Dank Greeter (`dms-greeter`) Native Support**: Upstream Dank Greeter has officially merged native secondary authentication on `Tab` and generalized face PAM detection ([PR #21](https://github.com/AvengeMedia/dank-greeter/pull/21), commit `334b2c93d444b65e4fad5b16f3409bcdc013e40f`). Pressing `Tab` on an empty password prompt in `dms-greeter` triggers face authentication natively with dynamic face recognition tooltip and icon, requiring zero custom forks.

## [0.1.2] - 2026-09-23

### Security & Hardening
- **Interactive Attention Confirmation**: The PAM module now prompts users to press Enter before activating the camera (`Press Enter to authenticate with face...`). This prevents unattended or passive logins from authenticating without the user's active intent.
- **Strict Remote Login Blocking**: Face authentication is now reliably disabled when connecting over SSH or remote terminals, ensuring facial recognition is only triggered for physically present users.
- **Protected Face Enrollment**: Face registration sessions now use unguessable cryptographic tokens, are locked to the application that started them, and expire automatically after 30 minutes to prevent unauthorized session takeovers.
- **Input Sanitization & Path Protection**: Added strict validation on all username and filename inputs, blocking directory traversal attempts (`..` or `/`) and preventing access or deletion outside designated Sentinel folders.
- **Continuous Verification in Liveness Checks**: The verification pipeline now confirms that the user completing blink challenges remains the same face throughout the entire check, preventing face-swap bypasses. Also introduced a configurable `require_liveness` toggle for high-security environments.
- **Privacy & Distance Oracle Protection**: Users can no longer query face recognition match scores for other accounts, and returned distances are normalized to prevent biometric profiling of subjects in front of the camera.
- **Audit Log Access Controls**: Restricted log file permissions so only administrators can view authentication history and timestamps.
- **Memory & Crash Protection**: Added an 8192px limit on processed image dimensions and limited image decoders to JPEG and PNG, protecting the daemon from memory exhaustion or crashes caused by oversized or malformed image inputs.
- **System Service Sandboxing**: Hardened the systemd background daemon with strict filesystem isolation, restricted privileges, and memory usage ceilings.
- **Lockout Prevention Fail-Safes**: All non-granted states (timeout, mismatch, un-enrolled account, or camera errors) safely return `PAM_IGNORE`, allowing standard password login to proceed seamlessly without triggering account lockout in `pam_faillock`. Added password pass-through if entered at the prompt, and created a single-command emergency recovery tool (`scripts/emergency_restore_pam.sh`) to instantly restore default system password authentication whenever needed.

## [0.1.1] - 2026-08-01

### Fixed
- **Continuous Scanning after Face Obstruction**: Fixed `STATE_WAITING` state machine logic so face obstruction or transient no-detection frames stay in `STATE_WAITING` rather than timing out prematurely or failing.
- **Lock Screen Password Field Behavior**: Added `PAM_AUTHTOK` check in `pam_sentinel.c` so entering a password directly in the DMS/lock screen input field steps aside (`PAM_IGNORE`) and bypasses opening the camera for face authentication.
- **Explicit Auth Failure Handling**: Updated PAM return code mapping to return `PAM_AUTH_ERR` for `TIMEOUT`, `DENIED`, `SPOOF`, and `REQUIRE_2FA` outcomes so lock screens present a clear failure notification before prompting for password.

### Added
- **Camera Auto-Exposure Warmup**: Added initial frame skip counter (5 frames) on cold camera startup to allow sensor auto-exposure and white balance to stabilize before starting face detection.

## [0.1.0] - 2026-07-31

### Added
- **Generalized Installation & Environment Detection**: `setup.sh` auto-detects distribution (Fedora/RHEL, Ubuntu/Debian, Arch/Manjaro), display manager (GDM, SDDM, greetd, LightDM), and lock screen (`dankshell`, `hyprlock`, `swaylock`, `waylock`, `kscreenlocker`, `gdm-password`). Added `--dry-run` flag support.
- **Biometric Core Subsystem (`sentinel-core`)**:
  - SCRFD face detection with 5-point landmark extraction (320×320 fast mode default).
  - 2D affine similarity transformation targeting ArcFace 112×112 canonical landmarks.
  - MobileFaceNet 512-dimensional embedding engine with CPU/OpenVINO execution provider support.
  - MiniFASNetV2 static anti-spoofing engine with first-run sensor self-calibration.
  - 4-Tier decision engine (Golden, Standard, 2FA, Denied).
  - Active liveness challenge (EAR blink state machine & randomized head pose checks).
  - Dynamic adaptive FIFO gallery update system with daily rate-limiting.
  - Asynchronous DBus system service interface (`com.sentinel.Sentinel`).
- **PAM Integration (`pam-sentinel`)**:
  - Thin C shared object module (`< 200 LOC`) with fail-safe password fallback (`PAM_IGNORE`).
  - Distro-aware dynamic library installation (`install_pam_module`).
- **Python Management Suite (`sentinel_py`)**:
  - CLI tool (`sentinel`) for enrollment, authentication, and status checks.
  - Interactive OpenCV 5-pose enrollment wizard.
  - Textual Terminal UI dashboard (`sentinel dashboard`).
- **Documentation & Packaging**:
  - Comprehensive specification documents in `docs/`.
  - Systemd service unit and DBus/PolicyKit security policy files.
