# Explicitly Excluded & Removed Features — Sentinel Recreated

**Document**: `docs/REMOVED_FEATURES.md`  
**Subsystem**: System Architectural Boundaries & Exclusions

---

## 1. Summary of Architectural Cleanups

**Sentinel Recreated** deliberately discards anti-patterns, legacy hacks, and fragile implementations present in earlier prototype and production attempts. Every exclusion listed below has been chosen to guarantee security stability, deterministic latency, and system maintainability.

---

## 2. Table of Excluded Features & Technical Justifications

| Discarded Feature / Pattern | Legacy Location | Technical Rationale for Removal |
|---|---|---|
| **Unix Domain Sockets + JSON-RPC IPC** | Old Production | **Fragile File Permissions**. PAM runs under various EUIDs (`root`, `gdm`, `user`), causing socket access errors. DBus handles bus permissions natively via PolicyKit. |
| **SFace Embedding Model (128-d)** | Prototype | **Weak Feature Discrimination**. SFace produces 128-d vectors that exhibit high cosine similarity variance under minor pose shifts. Replaced by 512-d MobileFaceNet/ArcFace. |
| **YuNet Face Detector** | Prototype | **Missing Landmarks**. YuNet detection does not reliably output the 5 canonical keypoints required for affine alignment matrices. Replaced by SCRFD. |
| **Direct Bounding-Box Bypassing Alignment** | Prototype | **PRIMARY RECOGNITION FAILURE CAUSE**. Feeding unaligned face crops into embedding extractors corrupts distance metrics. Fixed by mandatory 112×112 5-point similarity warping. |
| **Tkinter Intrusion Review UI** | Prototype | **Threading & Event Loop Conflicts**. Running Tkinter alongside OpenCV camera streams caused X11 threading locks. Replaced by Textual TUI. |
| **Hardcoded Global Session Timeouts** | Prototype | **Rigid Detector Logic**. Detector state timeouts are now dynamically configurable parameters in `/etc/sentinel/config.toml`. |
| **Vala / GTK4 Desktop GUI** | Old Production | **Unnecessary Bloat**. Native GTK compilation added complex build toolchain dependencies (`valac`, GTK libraries) for simple admin tasks. Replaced by Textual TUI. |
| **`pam_exec.so` Shell Script Integration** | Legacy Attempt | **Fragile Process Spawning**. Invoking shell scripts from PAM creates execution latency and handles errors poorly. Replaced by compiled C module `pam_sentinel.so`. |
| **Python Biometric Engine Daemon** | Prototype | **Python GIL & High Memory Usage**. Python runtime memory footprint exceeded 800MB and GIL limited multi-threaded ONNX pipeline performance. Replaced by compiled Rust core. |
| **Audio / TTS Voice Guidance** | Legacy Production | **Bloat & Audio Server Conflicts**. TTS engines (`pyttsx3`) frequently lock PipeWire/ALSA sound cards during PAM login prompts, blocking user desktop sessions. |

## 3. Tab-key face auth / Enter-key password separation (login & lock screens)

Status: Researched, verified not natively possible across GDM, DMS greeter, or desktop shells without custom forking.

Investigated the installed Dank Material Shell (DMS 1.6.1), `dms-greeter` (1.6.2), and **GDM 50 / GNOME Shell 50**
to determine whether tab-key triggering or external factor separation is supported.

Detailed Findings:

1. **GDM / GNOME Shell Architecture (`gdm-password`, `gnome-shell`)**:
   - **Hardcoded PAM Services**: In GNOME Shell's GDM utility (`/org/gnome/shell/gdm/util.js`), authentication services are strictly hardcoded to `gdm-password`, `gdm-fingerprint`, and `gdm-smartcard`. There is no configuration key or D-Bus API to register a third-party auth service.
   - **Parallel Biometrics Architecture**: GDM runs `gdm-password` in the foreground and can run `gdm-fingerprint` simultaneously in the background if enrolled hardware is detected over D-Bus (`net.reactivated.Fprint`). It does not use keybindings to trigger biometrics; it polls the sensor continuously while the password entry is active.
   - **Keypress Routing**: In `authPrompt.js` and `unlockDialog.js`, keypresses on the password entry (`St.PasswordEntry`) are handled by Clutter/St. `Enter` triggers `_activateNext()`, while `Tab` is hardwired to Clutter's widget focus navigation (`TAB_FORWARD`) to cycle focus to UI buttons (Cancel, Switch User, Power). There is no hook to bind `Tab` to an auth action.
   - **Extension Isolation**: Extensions are strictly disabled in GDM display manager mode (`--mode=gdm`) for security. Even on the user lock screen (`unlockDialog.js`), extensions cannot safely intercept PAM queries without monkey-patching GNOME Shell's internal JavaScript engine.

2. **DMS Greeter Architecture (`dms-greeter` / `greetd`)**:
   - **Embedded Binary UI**: `dms-greeter` is a compiled Go binary (`/usr/bin/dms-greeter`). At launch, its QML UI is unpacked read-only into `/var/cache/dms-greeter/.cache/dms-greeter-shell/<hash>/` and checksum-verified on every launch. Modifying the unpacked QML has no effect; overriding requires passing `--shell-dir <dir>` or compiling from source (`dms-greeter sync --local`).
   - **Hardcoded External Auth**: In `GreeterContent.qml` (lines 71–76), external auth is explicitly hardcoded to `pam_fprintd` and `pam_u2f` (`greeterPamStackHasModule("pam_fprintd")` / `"pam_u2f"`). There is no generic secondary factor registration or third-party auth hook.
   - **No Key Interception**: In `GreeterContent.qml`, the password field is a raw QtQuick `TextInput` with no `Keys.onPressed` handlers. Pressing `Tab` simply advances GUI focus rather than triggering authentication.
   - **Single Greetd Session IPC**: `Quickshell.Services.Greetd` communicates over `/var/run/greetd.sock` with a single PAM conversation session at a time, running `/etc/pam.d/greetd` sequentially.

3. **DMS Desktop Lock Screen (`dankshell` / `Quickshell.Services.Pam`)**:
   - **Dual PAM Architecture**: Unlike the greeter, the DMS Lock Screen implements independent `PamContext` objects: `pam.passwd` (runs `/etc/pam.d/dankshell` on Enter) and `pam.u2f` (runs a custom PAM service on shortcut).
   - **Extensible PAM Source**: Under **Settings → Lock Screen → Security Key**, DMS exposes `SettingsData.lockU2fPamPath` ("Security Key PAM Source"), allowing any custom PAM file (e.g. `/etc/pam.d/dankshell-sentinel`) in `Alternative (OR)` mode.
   - **Shortcut Constraints**: In `LockScreenContent.qml` (line 1030), the shortcut handler requires `Ctrl` (`(event.modifiers & Qt.ControlModifier)`). A modified shortcut like `Ctrl+Q` or `Ctrl+Tab` works natively via settings, but a bare `Tab` key without modifiers is not recognized as a shortcut.
   - **Plugin Limitations**: DMS's `PluginService.qml` (line 222) restricts plugin surfaces to `["widget", "desktop", "daemon", "launcher"]`. There is no auth or lock-screen plugin API.

4. **Universal Sentinel Solution (PAM Level)**:
   - In `pam_sentinel.c` (lines 108–130), the underlying race condition (password entry racing camera timeout) is solved directly in the PAM conversation:
     - If the user types a password, Sentinel preserves it in `PAM_AUTHTOK` and returns `PAM_IGNORE` immediately without camera activation or timeout latency.
     - If the user presses Enter without typing a password, Sentinel proceeds with face scanning.
     - If the camera fails or times out, it silently steps aside to standard password verification.
   - This approach is universal and works seamlessly across GDM, DMS Greeter, SDDM, LightDM, Swaylock, and Hyprlock without requiring display manager forks or shell modifications.

Decision: Not pursuing separate Tab/Enter keybinding. It is impossible in both GDM and `dms-greeter` without maintaining fragile forks, and Sentinel's PAM conversation architecture in `pam_sentinel.c` already provides a clean, race-free, universal experience.

Revisit if: Upstream display managers (GDM, greetd) standardize an external biometric provider or custom keybinding API.