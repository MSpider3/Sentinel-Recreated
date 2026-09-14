# PAM Module Integration Specification — Sentinel Recreated

**Document**: `docs/PAM_INTEGRATION.md`  
**Subsystem**: `pam-sentinel/` (`pam_sentinel.c`)

---

## 1. Architectural Mandate: Thin PAM Wrapper

The PAM module (`pam_sentinel.so`) acts strictly as a lightweight bridge connecting the Linux Pluggable Authentication Modules (PAM) infrastructure to the `sentinel-daemon` via DBus. 

**Strict Design Constraints**:
- Source code length MUST NOT exceed 200 lines of standard C99 code.
- **Zero** image processing, ONNX runtime linkage, or filesystem gallery lookups.
- Memory allocations must be statically bounded to prevent memory leaks during repeated authentication requests.

---

## 2. PAM Return Code Mapping

| Daemon DBus Result | PAM Return Code | System Auth Behavior |
|---|---|---|
| `GRANTED` | `PAM_SUCCESS` | Authentication successful; bypass password prompt if configured as `sufficient`. |
| `REQUIRE_2FA` | `PAM_AUTH_ERR` | Biometric valid, but 2FA policy enforced; PAM shows failure notice then falls through to password prompt (second factor). |
| `DENIED` | `PAM_AUTH_ERR` | Face distance exceeds all thresholds; PAM shows failure notice then falls through to password prompt. |
| `TIMEOUT` | `PAM_AUTH_ERR` | Active liveness session timed out (120 s global, or 20 s challenge). See note below. |
| `SPOOF` | `PAM_AUTH_ERR` | Anti-spoofing score violated live threshold; PAM shows failure notice then falls through to password prompt. |
| `NO_FACE` | `PAM_IGNORE` | No face present in field of view; transparently fall back to password with **no** failure notice. See note below. |
| **Daemon Offline / DBus Error** | **`PAM_IGNORE`** | **Fail-safe fallback: transparently hand off control to standard unix password module.** |

> [!NOTE]
> **`PAM_IGNORE` vs `PAM_AUTH_ERR` — semantic distinction**
>
> - **`PAM_IGNORE`** means *"I have no opinion — try the next module."*  PAM falls through to `pam_unix.so` silently with no failure message. Use this when the camera never meaningfully engaged (daemon offline, no face detected at all, SSH session).
> - **`PAM_AUTH_ERR`** means *"I tried and it failed."*  PAM shows an "authentication failed" notice in the lock screen before falling through to the password prompt. Use this when a camera session was opened and a face was involved (recognition failure, spoof, timeout).
>
> **Rule of thumb:** if the daemon opened the camera and actively attempted recognition, use `PAM_AUTH_ERR`. If the camera never meaningfully engaged, use `PAM_IGNORE`.

> [!NOTE]
> **Why `TIMEOUT → PAM_AUTH_ERR` (not `PAM_IGNORE`)**
>
> `TIMEOUT` means the camera was open and a liveness session ran, but the user did not complete the challenge within the time limit. This is an *active* failure, not a silent non-event. Returning `PAM_AUTH_ERR` causes the lock screen to show a clear failure notice before the password prompt appears, so the user understands that face auth was attempted and expired. A silent `PAM_IGNORE` would look indistinguishable from the daemon being offline.

> [!NOTE]
> **Why `NO_FACE → PAM_IGNORE` (not `PAM_AUTH_ERR`)**
>
> `NO_FACE` means the daemon opened the camera but found no face in the field of view — the user may have walked away, covered the camera, or is outside the frame. No recognition was ever attempted, so this is not a security failure. Returning `PAM_IGNORE` gives a silent fallback to password with no failure message, which is the correct UX for "user not present".

---

## 3. Fail-Safe Execution Flow (`pam_sm_authenticate`)

```c
PAM Entry (pam_sm_authenticate)
       │
       ▼
1. Fetch target username via pam_get_user()
       │
       ▼
2. Check if DBus System Bus is accessible & daemon is registered
       ├──► Daemon Offline/Unreachable ──► RETURN PAM_IGNORE (Fallback to Password)
       │
       ▼
3. Call com.sentinel.Sentinel.Authenticate(username) via DBus (Timeout: 10s)
       │
       ├──► DBus RPC Timeout/Failure  ──► RETURN PAM_IGNORE
       │
       ▼
4. Map result string to standard PAM return code
       └──► RETURN PAM_SUCCESS / PAM_AUTH_ERR / PAM_IGNORE
```

---

## 4. Session Context Awareness (Daemon Enforced)

The PAM module passes session context details to the daemon, which evaluates environment invariants before capturing camera frames:
1. **Remote SSH Sessions**: If `SSH_CLIENT` or `SSH_TTY` environment variables are present, the daemon immediately returns `NO_FACE` / `PAM_IGNORE` to prevent attempting local camera capture.
2. **Laptop Lid State**: The daemon checks `/sys/class/drm/card0/` or `/proc/acpi/button/lid/*/state`. If closed, camera capture is skipped and `NO_FACE` is returned.

---

## 5. Linux PAM Configuration Snippets

> [!WARNING]
> PAM configuration files control critical system authentication pathways. Always back up existing PAM configuration files and verify these snippets against your distribution's native `/etc/pam.d/` setup before applying changes. Incorrect PAM rules can lock out authentication.

### GDM Desktop Login (`/etc/pam.d/gdm-password`)
```pam
# %PAM-1.0
auth        sufficient    pam_sentinel.so
auth        substack      system-auth
auth        include       postlogin
account     include       system-auth
password    include       system-auth
session     include       system-auth
session     include       postlogin
```

### Sudo Escalation (`/etc/pam.d/sudo`)
```pam
# %PAM-1.0
auth        sufficient    pam_sentinel.so
auth        include       system-auth
account     include       system-auth
password    include       system-auth
session     include       system-auth
```

*Note: The `sufficient` control flag on `pam_sentinel.so` ensures that successful facial authentication satisfies PAM immediately. If the daemon is offline, returns `PAM_IGNORE` (e.g., no face detected or remote SSH session), or returns `PAM_AUTH_ERR`, PAM control falls through transparently to standard password prompts via `system-auth` (`pam_unix.so`).*

---

## 6. Supported Configurations

`setup.sh` automatically detects the running environment and configures the correct PAM files. Use `sudo ./setup.sh --dry-run` to preview what would be detected and configured on your system without modifying any files.

### Compatibility Table

| Distro | Display Manager | Desktop / Shell | Lock Screen | PAM Files Configured | Status |
|---|---|---|---|---|---|
| Fedora 40–44 | GDM | GNOME | *(via gdm-password)* | `gdm-password`, `gdm-autologin`, `sudo` | 🔲 Untested |
| Fedora 44 | greetd | Niri + DMS | dankshell | `greetd`, `dankshell`, `sudo` | ✅ Tested (maintainer's setup) |
| Ubuntu 22.04 / 24.04 | GDM | GNOME | *(via gdm-password)* | `gdm-password`, `gdm-autologin`, `sudo` | 🔲 Untested |
| Arch Linux | greetd | Hyprland | hyprlock | `greetd`, `hyprlock`, `sudo` | 🔲 Untested |
| Arch Linux | greetd | Sway | swaylock | `greetd`, `swaylock`, `sudo` | 🔲 Untested |
| Arch Linux | SDDM | KDE Plasma | kscreenlocker | `sddm`, `kde`, `sudo` | 🔲 Untested |
| Manjaro | SDDM | KDE Plasma | kscreenlocker | `sddm`, `kscreenlocker`, `sudo` | 🔲 Untested |

> [!NOTE]
> **GNOME lock screen**: On both Fedora and Ubuntu, the GNOME lock screen authenticates through the same GDM PAM stack (`gdm-password`). There is no separate `gnome-screensaver` PAM file on modern systems. Configuring `gdm-password` is sufficient for both login and lock screen.

> [!NOTE]
> **KDE lock screen**: The PAM service name varies by distro — `/etc/pam.d/kde` on Arch/Manjaro and `/etc/pam.d/kscreenlocker` on Ubuntu/Kubuntu. `setup.sh` injects into whichever file exists.

> [!NOTE]
> **DankMaterialShell (DMS)**: DMS is detected before other lock screens (it may coexist with swaylock). `setup.sh` writes to `/etc/pam.d/dankshell` and updates `~/.config/DankMaterialShell/settings.json` (as the invoking user, preserving file ownership).

---

### Per-Environment PAM Snippets

#### GDM + GNOME (`/etc/pam.d/gdm-password`)
```pam
#%PAM-1.0
auth        sufficient    pam_sentinel.so
auth        substack      system-auth
auth        include       postlogin
account     include       system-auth
password    include       system-auth
session     include       system-auth
session     include       postlogin
```

#### SDDM + KDE (`/etc/pam.d/sddm` and `/etc/pam.d/kde` or `/etc/pam.d/kscreenlocker`)
```pam
#%PAM-1.0
auth        sufficient    pam_sentinel.so
auth        include       system-auth
account     include       system-auth
password    include       system-auth
session     include       system-auth
```

#### greetd + Hyprland (`/etc/pam.d/greetd` and `/etc/pam.d/hyprlock`)
```pam
#%PAM-1.0
auth        sufficient    pam_sentinel.so
auth        include       system-auth
account     include       system-auth
password    include       system-auth
session     include       system-auth
```

#### greetd + Sway (`/etc/pam.d/greetd` and `/etc/pam.d/swaylock`)
```pam
#%PAM-1.0
auth        sufficient    pam_sentinel.so
auth        include       system-auth
account     include       system-auth
password    include       system-auth
session     include       system-auth
```

> [!WARNING]
> `swaylock` must be compiled with PAM support. Verify with `swaylock --help | grep -i pam`. If PAM support is missing, install the distro package (`sudo pacman -S swaylock`, `sudo apt install swaylock`, or `sudo dnf install swaylock`) rather than a custom build.

#### greetd + Niri + DMS (`/etc/pam.d/greetd` and `/etc/pam.d/dankshell`)
```pam
#%PAM-1.0
auth        sufficient    pam_sentinel.so
auth        include       system-auth
account     include       system-auth
password    include       system-auth
session     include       system-auth
```

---

### Manual PAM Configuration

If `setup.sh` cannot detect your environment (e.g., custom compositor), manually add the sentinel line to your lock screen's PAM service file:

```bash
sudo cp /etc/pam.d/<your-lock-screen> /etc/pam.d/<your-lock-screen>.bak
# Insert as the first auth line:
sudo sed -i '0,/^auth/s//auth       sufficient    pam_sentinel.so\nauth/' /etc/pam.d/<your-lock-screen>
```

Then verify: `grep pam_sentinel /etc/pam.d/<your-lock-screen>`

---

### Reporting a Tested Configuration

If Sentinel works on a configuration marked 🔲 Untested, please open an issue or PR with:
- Your distro name and version (`cat /etc/os-release`)
- Display manager (`systemctl list-units --type=service --state=active | grep -E 'gdm|sddm|greetd|lightdm'`)
- Lock screen binary (`command -v hyprlock swaylock waylock`)
- Output of `sudo ./setup.sh --dry-run`

