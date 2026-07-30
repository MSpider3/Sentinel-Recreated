# Changelog

All notable changes to Sentinel Recreated will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
