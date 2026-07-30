# Security Model & Threat Specification — Sentinel Recreated

**Document**: `docs/SECURITY_MODEL.md`  
**Subsystem**: `sentinel-core/src/audit.rs` & Security Submodules

---

## 1. Comprehensive Threat Matrix

| Threat Vector | Severity | Mitigation Strategy in Sentinel Recreated |
|---|---|---|
| **Static Photo / Screen Display Spoofing** | High | **MiniFASNetV2 ONNX anti-spoofing model**. Evaluates Fourier high-frequency components, reflection, and depth queues. Self-calibrated per camera sensor. |
| **Video Replay Attacks** | High | **Dynamic Liveness Challenges**. Requires active eye blinks validated via a 5-state Eye Aspect Ratio (EAR) state machine combined with randomized head rotational requests (`TURN_LEFT`, `TURN_RIGHT`, `TILT_UP`, `TILT_DOWN`). |
| **Unknown Intruder Attempts** | High | **Tier 4 Threshold Enforcement ($d > 0.50$)**. Immediate access denial, automatic capture of an intrusion screenshot image, and creation of a temporary candidate vector in the blacklist database. |
| **Adversarial Gallery Poisoning** | Critical | **Adaptive Gallery Rate-Limiting**. Adaptive updates are restricted to Tier 1 ($d < 0.25$), rate-limited to a maximum of **1 update per day**, and subject to a 1-in-11 probabilistic roll ($p \approx 0.09$) per session. |
| **Unauthorized DBus IPC Calls** | Medium | **PolicyKit Authorization**. Administrative DBus methods (`StartEnrollment`, `RemoveUser`, `SetConfig`) require PolicyKit admin authentication (`auth_admin`). |
| **Embedding Template Theft** | Medium | **Strict Storage Controls**. Embedding arrays (`gallery.npy`, `adaptive.npy`) are owned by `root:root` with strict `0600` file permissions in `/var/lib/sentinel/`. |

---

## 2. System Scope & Explicit Exclusions

> [!WARNING]
> Facial recognition on 2D RGB consumer webcams is a **convenience and anti-shoulder-surfing security layer**, NOT an absolute physical security barrier.

### Explicitly Out-of-Scope Threat Vectors:
- **Physical Hardware Tampering**: Man-in-the-middle attacks on the USB camera bus or virtual video device loopbacks (`v4l2loopback`).
- **High-Fidelity 3D Sculpted Masks**: Beyond the texture analysis scope of 2D MiniFASNet.
- **Kernel-Level Compromise**: Subversion of standard Linux kernel execution or systemd runtime memory.

---

## 3. Session Lockout & Retry Limits

- **Maximum Allowed Attempts**: 3 consecutive failed biometric evaluations (anti-spoof fail, liveness timeout, or frame rejection) per PAM session.
- **Lockout Scope**: Session-level transient lockout. Returns `PAM_AUTH_ERR` immediately on subsequent attempts within the same session.
- **No Global System Lockout**: System-level account locking is NOT performed on biometric failures to prevent Denial of Service (DoS) attacks against legitimate users. Password fallback remains available.

---

## 4. Structured Audit Log Specification

Audit events are appended to `/var/log/sentinel/auth_YYYY-MM-DD.log` in pipe-separated value format (`|`).

### File Permissions & Retention
- Path: `/var/log/sentinel/auth_YYYY-MM-DD.log`
- Owner/Group: `root:root`
- Mode: `0640`
- Retention Policy: 30 days maximum. FIFO cleanup executed on daemon startup.

### Audit Log Record Format
```
TIMESTAMP|USER|RESULT|DISTANCE|TIER|LIVENESS_STATUS|SPOOF_SCORE|DURATION_MS
```

### Example Log Entries
```
2026-07-21T14:32:10.104Z|mehulgolecha|GRANTED|0.182|1|SKIPPED|0.984|38
2026-07-21T14:35:22.881Z|mehulgolecha|GRANTED|0.312|2|BLINK_PASSED|0.941|1420
2026-07-21T14:40:01.002Z|guest|DENIED|0.641|4|SKIPPED|0.912|41
2026-07-21T14:42:15.510Z|mehulgolecha|SPOOF|0.210|1|SKIPPED|0.320|22
```

> [!NOTE]
> **Audit Note on `LIVENESS_STATUS: SKIPPED`**: Log entries displaying `SKIPPED` under `LIVENESS_STATUS` for Tier 1 (Golden) authentication grants or Tier 4 (Denied) rejections are **expected and by design**:
> 1. **Tier 1 (Golden, $d < 0.25$)** bypasses the interactive liveness challenge (blinks/head pose) to provide low-friction instant authentication ($<45\text{ ms}$), while still enforcing static anti-spoofing score checks (`MiniFASNetV2`).
> 2. **Tier 4 (Denied, $d > 0.50$)** fails fast immediately following feature matching to prevent wasting CPU cycles on unauthorized attempts.

---

## 5. Known Limitations

> [!CAUTION]
> The following limitations are **verified empirical constraints** observed on the target hardware (Intel i3 10th Gen, standard 720p USB webcam). They are known, documented, and do not represent bugs — they are physical and algorithmic boundaries of the default 2D RGB + 320×320 pipeline configuration.

| Limitation | Condition | Impact | Workaround |
|---|---|---|---|
| **Low-light detection failure** | Ambient luma $Y < 30$ (per BT.601 weighted average) | Frames are rejected by the dark-frame filter; daemon returns `NO_FACE` → `PAM_IGNORE` (password fallback) | Improve ambient lighting. CLAHE preprocessing mitigates mild low light but cannot compensate for near-darkness. |
| **Distance > 60 cm detection failure** | Using default 320×320 SCRFD input resolution | SCRFD-500M at 320×320 cannot reliably detect faces below the 120px bounding box height quality gate at distances beyond ~60 cm | Set `scrfd_input_size = 640` in `/etc/sentinel/config.toml`. Increases total pipeline mean from ~33 ms to ~71 ms, but maintains reliable detection at distance. |
| **Extreme face angles** | Yaw > ~40° or pitch > ~30° from camera normal | SCRFD landmark quality degrades; affine alignment produces distorted crops that fail embedding matching | User should face the camera directly. Enrollment pose guidance (5-pose wizard) builds tolerance for mild variation. |
| **Near-identical twins / siblings** | Cosine distance may fall inside Tier 1 or Tier 2 range | System may authenticate a sibling | Known limitation of 2D RGB recognition without IR depth sensing. Enable `require_liveness = true` in config to force blink challenge even for Tier 1 matches in high-security scenarios. |
| **IR camera unsupported** | Infrared-only or structured-light depth sensors | V4L2 capture and SCRFD are tuned for visible-spectrum 2D RGB | IR support is deferred to v2. Depth-based anti-spoofing is out of scope for v1. |

> [!NOTE]
> In all failure cases above, Sentinel fails **open-safe**: the daemon returns `PAM_IGNORE`, allowing PAM to fall through to the standard password prompt. No biometric failure will lock a user out of their system.

