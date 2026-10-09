# Sentinel Recreated

![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)
![Platform: Linux](https://img.shields.io/badge/Platform-Linux-informational)
![Language: Rust](https://img.shields.io/badge/Language-Rust-orange)

Face authentication for Linux — unlock sudo, your login screen, and lock screen by looking at your webcam.

## What It Does

Sentinel runs as a root systemd daemon that performs face recognition via DBus, integrating with PAM so any application that uses PAM (sudo, GDM, greetd, SDDM, swaylock, hyprlock) can authenticate you biometrically. It uses SCRFD for face detection, MobileFaceNet for 512-dimensional embeddings, and two MiniFASNet models for anti-spoofing. On an Intel i3 10th gen with integrated graphics one frame takes about **43 ms**, and a typical unlock takes **0.4–1 second**, most of it the camera switching on. If the daemon is unavailable, no face is found, or the scan is not conclusive within 7 seconds, PAM falls through transparently to password — you are never locked out.

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
- No discrete GPU required — runs entirely on CPU

**Software**
- Linux with systemd (kernel ≥ 6.6 recommended)
- Wayland (recommended) or X11
- GStreamer 1.x with PipeWire or V4L2 support
- Python 3.11+
- Rust toolchain — install from [rustup.rs](https://rustup.rs) if not present

## Installation

```bash
git clone https://github.com/MSpider3/Sentinel-Recreated.git
cd Sentinel-Recreated
sudo ./setup.sh
sentinel enroll $USER
```

The installer auto-detects your distro, display manager, and lock screen. Run `sudo ./setup.sh --dry-run` first to preview what will be detected and configured without touching any files.

Face unlock is always set up for the lock screen. The installer **asks** before enabling it for `sudo` and for the login screen (`--yes` answers yes to both). Keep a root shell open while installing; `sudo ./scripts/emergency_restore_pam.sh` removes Sentinel from every PAM file if anything goes wrong.

## Usage

```bash
# Enroll your face (run once — asks whether you wear glasses, then guides you through 5 poses)
sentinel enroll $USER
sentinel enroll $USER --glasses      # or --no-glasses: answer the question in advance

# Check daemon and enrollment status
sentinel status

# Run one face scan and show the result, match distance and anti-spoof score
sentinel auth $USER

# Launch the terminal dashboard
sentinel dashboard
```

The dashboard has four screens: **Dashboard** (`d`: daemon status and recent scans with distance and anti-spoof score), **Users** (`u`: enroll, remove), **Intrusions** (`i`: view or dismiss photos of faces that were clearly not you) and **Settings** (`s`: thresholds, timeout, camera). `t` runs a test scan, `q` quits.

After enrollment, face unlock is active for the PAM services you enabled during setup. A scan answers in about a second in good conditions and gives up after at most 7 seconds, handing over to the password prompt.

## Upgrading

Run `sudo ./setup.sh` again. It rebuilds and reinstalls the daemon, the PAM module and the CLI together, and asks whether to keep face unlock for `sudo` and the login screen.

**From 0.1.3 or earlier, re-enroll afterwards** (`sentinel enroll $USER`): templates are built differently since 0.1.4, and re-enrolling also clears templates learned by the old version. See [`CHANGELOG.md`](CHANGELOG.md).

## Testing

```bash
cargo test --lib                          # unit tests (alignment, decision rules, config, storage)
python3 tests/general/test_tui.py         # dashboard, headless, no daemon needed
python3 tests/general/test_spoof.py       # live: your face, then a photo/video of you
python3 tests/general/test_recognition.py # live: distance, lighting, glasses, another person
sudo ./target/release/auth-test --user $USER   # one scan with the built code, without installing it
```

## How It Works

```
Webcam → [Rust daemon] → SCRFD detect → 5-pt align → MobileFaceNet embed
                       → match → MiniFASNet anti-spoof → multi-frame decision → DBus result
[C PAM module] ←────────────────────────────────────────────────────────────
     ↓
PAM_SUCCESS (face matched) or PAM_IGNORE (fall through to password)
```

- **`sentinel-core`** — Rust daemon running as root. Owns the camera, models, and gallery. Exposes a DBus interface (`com.sentinel.Sentinel`) for authentication, enrollment, configuration, and intrusion review.
- **`pam-sentinel`** — Thin C shared library. Calls the daemon over DBus and maps the result to PAM return codes. Contains zero biometric code.
- **`sentinel-py`** — Python CLI and Textual TUI for enrollment, status, and configuration.

Full architecture: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | Pipeline details: [`docs/FRS_PIPELINE.md`](docs/FRS_PIPELINE.md)

## Security Model

**Sentinel provides:**
- ✓ A passive check against printed photos and simple screen replays (MiniFASNet anti-spoof models), on the same frame that matched
- ✓ Multi-frame decisions: one clean strong match, or three matching frames in a row
- ✓ An attempt limit: five failed face attempts in a minute pause face unlock
- ✓ Adaptive gallery that handles gradual appearance changes over time
- ✓ Audit logging of all authentication attempts to `/var/log/sentinel/`
- ✓ Automatic password fallback if the camera or daemon is unavailable

**Sentinel does NOT protect against:**
- ✗ A good video of you played on a good screen (no RGB-only webcam system reliably stops this)
- ✗ High-quality 3D mask attacks
- ✗ Complete darkness — face detection requires ambient light
- ✗ Physical camera tampering (V4L2 loopback injection)
- ✗ Kernel-level compromise

Face authentication is a **convenience factor and anti-shoulder-surfing measure**, not a replacement for a strong password. Password fallback is always available and cannot be disabled through Sentinel.

## Known Limitations

- **Low light** — Dim frames are brightened for face detection, but a face that is too dark is skipped and the scan times out to the password prompt.
- **Distance** — Reliable detection range is approximately 30–80 cm from camera. Beyond ~80 cm, the face bounding box may fall below the minimum size for SCRFD-500M at 320×320 input. Set `scrfd_input_size = 640` in `/etc/sentinel/config.toml` for better range at the cost of ~7 ms additional latency.
- **Anti-spoofing is not proof of presence** — the anti-spoof models were trained on other cameras; test them on yours with `tests/general/test_spoof.py` using a photo and a phone video *of yourself*.
- **Thresholds are hardware-dependent** — the face-match thresholds (`golden_threshold = 0.28`, `standard_threshold = 0.42`) and the anti-spoof thresholds (`spoof_threshold = 0.80`, `spoof_threshold_standard = 0.70`) were set from measurements on the maintainer's webcam. Check your own numbers with `sentinel auth` or the dashboard and adjust in the Settings screen or `/etc/sentinel/config.toml`. The daemon refuses values outside safe ranges.

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
