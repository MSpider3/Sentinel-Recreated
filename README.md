# Sentinel Recreated

![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)
![Platform: Linux](https://img.shields.io/badge/Platform-Linux-informational)
![Language: Rust](https://img.shields.io/badge/Language-Rust-orange)

Face authentication for Linux — unlock sudo, your login screen, and lock screen by looking at your webcam.

## What It Does

Sentinel runs as a root systemd daemon that performs face recognition via DBus, integrating with PAM so any application that uses PAM (sudo, GDM, greetd, SDDM, swaylock, hyprlock) can authenticate you biometrically. It uses SCRFD for face detection, MobileFaceNet for 512-dimensional embeddings, and MiniFASNetV2 for anti-spoofing. On an Intel i3 10th gen with integrated graphics, the full pipeline runs at **33ms average latency (~30 FPS)**. If the daemon is unavailable or no face is detected, PAM falls through transparently to password — you are never locked out.

## Tested Configuration

> **Only one configuration has been personally tested by the maintainer:**
>
> - **Fedora 44**, Niri compositor, DankMaterialShell, greetd
>
> All other configurations are based on code logic and community reports. See the compatibility table below.

## Compatibility

`setup.sh` auto-detects your distro, display manager, and lock screen and configures the correct PAM files.

| Distro | Display Manager | Desktop / Shell | Lock Screen | Status |
|---|---|---|---|---|
| Fedora 44 | greetd | Niri + DMS | dankshell | ✅ Tested (maintainer) |
| Fedora 40–44 | GDM | GNOME | *(via gdm-password)* | 🔲 Untested |
| Ubuntu 22.04 / 24.04 | GDM | GNOME | *(via gdm-password)* | 🔲 Untested |
| Arch Linux | greetd | Hyprland | hyprlock | 🔲 Untested |
| Arch Linux | greetd | Sway | swaylock | 🔲 Untested |
| Arch Linux | SDDM | KDE Plasma | kscreenlocker | 🔲 Untested |
| Manjaro | SDDM | KDE Plasma | kscreenlocker | 🔲 Untested |

Full per-environment PAM configuration details: [`docs/PAM_INTEGRATION.md`](docs/PAM_INTEGRATION.md)

## Requirements

**Hardware**
- Any Linux system with a 2D RGB webcam (V4L2 compatible)
- Minimum: Intel Core i3 10th gen or equivalent AMD, 8 GB RAM
- No discrete GPU required — runs entirely on CPU (or Intel iGPU via OpenVINO)

**Software**
- Linux with systemd (kernel ≥ 6.6 recommended)
- Wayland (recommended) or X11
- GStreamer 1.x with PipeWire or V4L2 support
- Python 3.10+
- Rust toolchain — install from [rustup.rs](https://rustup.rs) if not present

## Installation

```bash
git clone https://github.com/MSpider3/Sentinel-Recreated.git
cd Sentinel-Recreated
sudo ./setup.sh
sentinel enroll $USER
```

The installer auto-detects your distro, display manager, and lock screen. Run `sudo ./setup.sh --dry-run` first to preview what will be detected and configured without touching any files.

## Usage

```bash
# Enroll your face (run once — guides you through 5 poses)
sentinel enroll $USER

# Check daemon and enrollment status
sentinel status

# Manually trigger an authentication attempt
sentinel auth $USER

# Launch the terminal dashboard (live view of auth sessions)
sentinel dashboard

# Re-run the anti-spoof camera calibration
sentinel calibrate-spoof
```

After enrollment, face unlock is active automatically for any PAM-integrated service (sudo, login screen, lock screen).

## How It Works

```
Webcam → [Rust daemon] → SCRFD detect → 5-pt align → MobileFaceNet embed
                       → MiniFASNet anti-spoof → Tier decision → DBus result
[C PAM module] ←────────────────────────────────────────────────────────────
     ↓
PAM_SUCCESS (face matched) or PAM_IGNORE (fall through to password)
```

- **`sentinel-core`** — Rust daemon running as root. Owns the camera, models, and gallery. Exposes a DBus interface (`com.sentinel.Sentinel`) for authentication, enrollment, configuration, and intrusion review.
- **`pam-sentinel`** — Thin C shared library (`< 200 LOC`). Calls the daemon over DBus and maps the result to PAM return codes. Contains zero biometric code.
- **`sentinel-py`** — Python CLI and Textual TUI for enrollment, status, and configuration.

Full architecture: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | Pipeline details: [`docs/FRS_PIPELINE.md`](docs/FRS_PIPELINE.md)

## Security Model

**Sentinel provides:**
- ✓ Protection against photo and screen spoofing (MiniFASNetV2 anti-spoof model)
- ✓ Active liveness detection — blink + head pose challenge for Tier 2/3 matches
- ✓ Adaptive gallery that handles gradual appearance changes over time
- ✓ Audit logging of all authentication attempts to `/var/log/sentinel/`
- ✓ Automatic password fallback if the camera or daemon is unavailable

**Sentinel does NOT protect against:**
- ✗ High-quality 3D mask attacks
- ✗ Complete darkness — face detection requires ambient light
- ✗ Physical camera tampering (V4L2 loopback injection)
- ✗ Kernel-level compromise

Face authentication is a **convenience factor and anti-shoulder-surfing measure**, not a replacement for a strong password. Password fallback is always available and cannot be disabled through Sentinel.

## Known Limitations

- **Low light** — Authentication fails when ambient light is too low for face detection. CLAHE preprocessing helps with mild low light but cannot compensate for near-darkness.
- **Distance** — Reliable detection range is approximately 30–80 cm from camera. Beyond ~80 cm, the face bounding box may fall below the minimum size for SCRFD-500M at 320×320 input. Set `scrfd_input_size = 640` in `/etc/sentinel/config.toml` for better range at the cost of ~7 ms additional latency.
- **MiniFASNet calibration** — On some cameras, the anti-spoof model relies primarily on distance thresholding rather than texture analysis. Run `sentinel calibrate-spoof` after enrollment to optimize for your camera.
- **Tier thresholds are hardware-dependent** — The default `golden_threshold = 0.28` may result in Tier 1 on high-quality setups. Adjust in `/etc/sentinel/config.toml` based on your observed authentication distances (visible via `sentinel dashboard` or `journalctl -u sentinel`).

## Contributing

### Reporting a Working Configuration

If Sentinel works on your setup, please open an issue titled:

```
Tested: [Distro] + [Display Manager] + [Desktop] + [Lock Screen]
```

Include the output of `sudo ./setup.sh --dry-run` and confirmation that both login and lock screen authentication work. Verified configs will be promoted to ✅ Tested in the compatibility table.

### Adding Support for New Environments

PAM configuration for new display managers and lock screens can be added to the `detect_display_manager()`, `detect_lock_screen()`, and `configure_pam()` functions in `setup.sh`. See [`docs/PAM_INTEGRATION.md`](docs/PAM_INTEGRATION.md) for the full list of PAM files by environment.

## License

[GNU General Public License v3.0](LICENSE) — you are free to use, modify, and distribute this software under the terms of the GPL v3. Any derivative work must also be licensed under GPL v3.

## Acknowledgements

- [GunduLabs/gaze](https://github.com/GunduLabs/gaze) — architecture reference for Rust-based face authentication with DBus and PAM integration
- [InsightFace](https://github.com/deepinsight/insightface) — SCRFD detection and MobileFaceNet embedding models
- [minivision-ai](https://github.com/minivision-ai/Silent-Face-Anti-Spoofing) — MiniFASNetV2 anti-spoofing model
