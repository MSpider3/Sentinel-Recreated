# Changelog

All notable changes to Sentinel Recreated will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.4] - 2026-10-09

Result of a full code review. The daemon, the PAM module and the Python CLI changed together and must be installed together (`sudo ./setup.sh`). **Re-enroll after upgrading** (`sentinel enroll $USER`): templates are now built differently.

### Fixed — recognition
- **Face alignment rotated tilted heads the wrong way** (a 10° tilt became 20°). Upright faces were unaffected, which hid the bug.
- **Detector input was stretched** from 4:3 to a square. It is now letterboxed as SCRFD expects; detection scores on test frames rose from 0.61–0.85 to 0.83–0.89.
- **The same camera frame was processed several times**, and decisions were made on the first frames of a cold camera. Frames are now numbered and used once; recognition starts when brightness has settled.
- **Enrollment stored every preview frame** (about ten per second), not just the captures. Preview and capture are now separate; captures are quality-checked, de-duplicated and capped at 40.
- **Re-enrolling left old learned templates active.** They are now deleted.
- A frame row-padding assumption that silently dropped all frames at some camera widths.

### Fixed — anti-spoofing
- **MiniFASNet was fed the wrong input** (RGB instead of BGR, a 1.5× crop instead of 2.7×, stretched at frame edges). It is now fed exactly as upstream trains it, verified against OpenCV's DNN output for the same model.
- **Anti-spoof thresholds raised** to 0.80 (one-frame grant) and 0.70 (voted grant), from measurements on the target webcam: the live owner scored 0.84–1.00, photo and phone-video attacks 0.00–0.24 with one photo session at 0.58–0.64.
- **Removed the first-run "self-calibration"**, which collected 80 frames and then wrote a fixed result, and `sentinel calibrate-spoof`, which printed made-up scores.
- **Anti-spoofing no longer fails open.** A missing or broken model means no face unlock (password still works), instead of unlocking without the check.

### Changed — decisions
- **Tiers now decide how much proof is needed** instead of triggering blink / head-turn challenges (removed: the blink check could not see eyelids, the head-turn check passed when a photo was slid sideways, and neither could be shown on a login screen):
  - strong match with a clean anti-spoof score → granted on that frame;
  - normal match → three matching, live frames in a row;
  - "not sure" or a near miss → keep looking until the timeout; clearly a different person (distance 0.20 past `two_factor_threshold`) for ten frames in a row → denied.
- **A scan ends after at most 7 s** (was up to 20 s in the daemon while PAM gave up after 5 s). The PAM module waits 8 s, so the daemon always answers first.
- **Removed the embedding blacklist.** One poor frame of the owner could ban the owner, and it added no security. Intrusion *photos* are still saved (after ten clearly-different frames; newest 20 / 30 days kept).
- **Adaptive gallery**: a learned template must strongly match an *enrolled* template, be a little different from what is stored, and have a clean anti-spoof score. The random 1-in-11 roll is gone.

### Added
- **Frame quality gate** (pose, framing, exposure, sharpness) before recognition; poor frames are skipped, not counted as mismatches.
- **Dark-frame brightening** for the detector only.
- **Second anti-spoof model** (`MiniFASNetV1SE`, 4.0× crop), fused with the first when installed.
- **Attempt limit**: five failed face attempts per user per minute pause face unlock (`RATE_LIMITED`).
- **Remote-session check through logind**, in addition to the caller-supplied `SSH_*` variables.
- **Mirror-image augmentation at enrollment.**
- **`sentinel enroll` asks whether you wear glasses** instead of assuming you do not; `--glasses` / `--no-glasses` skip the question.
- Config validation (ranges, camera source), per-key defaults, atomic file writes for galleries and config. An invalid config file now stops the daemon with a clear error instead of silently running on other values; unknown keys are logged.
- The daemon exits (and systemd restarts it) on any internal panic, instead of staying up but unresponsive. Damaged gallery files are reported as errors.
- Model SHA-256 verification in `setup.sh`.
- `sentinel auth` and the test scripts show the real match distance and anti-spoof score of each scan, read from the audit log (the daemon still never returns them over DBus).

### Changed — hardening
- Usernames are checked against an allow-list (letters, digits, `_ - . @`) and must exist, closing audit-log injection.
- The camera source must be `/dev/videoN` or `pipewiresrc`; it can no longer carry extra GStreamer pipeline text.
- PAM module: a typed password is wiped from memory before it is freed, and the result no longer passes through a shared static buffer.
- `SubmitEnrollmentFrameData` takes a third argument, `capture` (false = preview only).
- `tests/general/test_recognition.py` no longer recommends thresholds (it was deriving them from the constant distances the daemon returns); `test_spoof.py` fails if any photo attempt is granted.

### Measured on the maintainer's machine (Fedora 44, i3-1005G1, built-in 640×480 webcam)
- Unlock through PAM in an isolated test: 0.4–0.9 s. Timeout 7.05 s, no-face exit 3.5 s, daemon idle CPU 0, memory about 97 MB.
- Old code: a photo of the owner was granted 3 times out of 5.
- New code, before the thresholds were raised: photo refused 5 of 6, phone video refused 4 of 4.
- New code with the final thresholds: phone video refused 9 of 9, real face granted 4 of 4. The photo was not re-tested after the change.
- These are small samples from one camera. Test on your own with `tests/general/test_spoof.py`.

### Changed — speed
- **Models are loaded once at daemon start** (they were reloaded on every unlock and every enrollment frame), share one ONNX Runtime thread pool, and are warmed up with a dummy run.
- The camera is started first, the reply no longer waits for the camera to shut down, and the loop waits for new frames instead of sleeping.
- Per-frame debug output moved to the `debug` log level. Match distances no longer appear in the journal at the default level.

### Changed — dashboard (`sentinel dashboard`)
- Settings screen can now edit `spoof_threshold_standard` and `global_session_timeout`, explains each value, and reports exactly which input is wrong or why the daemon refused a change.
- Config values are written as short decimals (`0.4`, not `0.4000000059604645`).
- Errors are shown on screen instead of being silently ignored (users, intrusions, log, daemon not running).
- The log shows the newest entries first in local time, with labelled distance and anti-spoof columns, and no longer goes empty at midnight (`GetRecentAuthLog` reads back through the daily files).
- Intrusion photos can be opened from the dashboard (asks for the administrator password, as the files are root-only).
- A "Test face unlock now" button / `t` key runs `sentinel auth` for the current user.
- New headless test: `python3 tests/general/test_tui.py`.

### Changed — installer
- Builds as the invoking user, installs the CLI into its own virtual environment (`/opt/sentinel/venv`), no longer needs OpenCV development packages.
- **Asks before enabling face unlock for `sudo` and the login screen** (and, if it is already on, whether to keep it — answering no removes it); no longer touches `gdm-autologin`.
- The systemd unit lets the daemon write `/etc/sentinel` so `SetConfig` works.

### Removed
- DBus methods `SubmitEnrollmentFrame`, `ResetSpoofCalibration`, `RunSpoofCalibration`; CLI `calibrate-spoof` and `enroll --append-glasses` (it never did anything).
- Config keys `recognition_threshold`, `spoof_threshold_golden` (use `spoof_threshold`), `challenge_timeout_secs`, `require_liveness`, `execution_provider`, `frame_drop_threshold_ms`. Old files containing them still load.
- Files no longer read (safe to delete): `/var/lib/sentinel/blacklist/embeddings.npy`, `/var/lib/sentinel/minifas_calib.json`.

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
