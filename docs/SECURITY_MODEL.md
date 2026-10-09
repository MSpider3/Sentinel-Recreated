# Security Model & Threat Specification — Sentinel Recreated

**Document**: `docs/SECURITY_MODEL.md`  
**Subsystem**: `sentinel-core/src/audit.rs` & Security Submodules

---

## 1. Comprehensive Threat Matrix

| Threat Vector | Severity | Mitigation Strategy in Sentinel Recreated |
|---|---|---|
| **Static Photo / Screen Display Spoofing** | High | **MiniFASNet anti-spoofing** (V2, plus V1SE when installed) on every matching frame. A matching face that scores low three times ends the scan as `SPOOF`. If no anti-spoof model is loaded the daemon refuses every request. |
| **Video Replay Attacks** | High | **Not reliably stopped.** A good video of the owner on a good screen can pass any RGB-only check. Mitigations: the Enter-key consent prompt, the attempt limiter, and the option to keep face unlock off `sudo` and the login screen. |
| **Unknown Intruder Attempts** | High | Ten clearly-different frames in a row ($d \ge$ `two_factor_threshold` + 0.20) end the scan as `DENIED` and save an intrusion photo. Five failed attempts per minute pause face unlock for that user. |
| **Adversarial Gallery Poisoning** | Critical | **Anchored adaptive gallery.** A learned template must strongly match an *enrolled* template ($d <$ `golden_threshold`), have a clean anti-spoof score, and respect the limit of **1 update per day**. Enrolled templates are never modified; re-enrolling deletes all learned ones. |
| **Unauthorized DBus IPC Calls** | Medium | **PolicyKit Authorization**. Administrative DBus methods (`StartEnrollment`, `RemoveUser`, `SetConfig`) require PolicyKit admin authentication (`auth_admin`). |
| **Remote Session Abuse** | High | The daemon asks logind whether the caller's session is remote; the caller-supplied `SSH_*` variables are only a second check. Remote sessions never start the camera. |
| **Unsafe Configuration** | Medium | `SetConfig` and config loading range-check every value (thresholds, camera source). `SetConfig` refuses a bad config. If the file on disk is invalid at start-up the daemon does not start (it never guesses other values), so face unlock is off and the password is used until the file is fixed; unknown keys are logged as warnings. |
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

## 3. Attempt Limits

- **Within one scan**: `max_retries` (3) low anti-spoof scores on a matching face end it as `SPOOF`; ten clearly-different frames in a row end it as `DENIED`; `global_session_timeout` (max 7 s) ends it as `TIMEOUT`.
- **Across scans**: five scans per user within 60 s in which a face was seen but not granted make the daemon answer `RATE_LIMITED` until the oldest failure is a minute old. Scans where nobody was in view do not count. A successful scan clears the count.
- **No lockout**: every result except `GRANTED` is `PAM_IGNORE` in the PAM module. Biometric failures never reach `pam_faillock`; the password always works.

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
2026-07-21T14:32:10.104Z|testuser|GRANTED|0.182|1|PASSIVE|0.984|38
2026-07-21T14:35:22.881Z|testuser|GRANTED|0.312|2|PASSIVE|0.941|1420
2026-07-21T14:40:01.002Z|unknown|DENIED|0.641|4|SKIPPED|N/A|410
2026-07-21T14:42:15.510Z|unknown|SPOOF|0.210|1|PASSIVE|0.320|620
```

> [!NOTE]
> `LIVENESS_STATUS` is `PASSIVE` when the anti-spoof model scored the session's last matching frame, and `SKIPPED` when no frame matched well enough to be scored. `USER` is the target user for `GRANTED` and `unknown` otherwise. Match distances and anti-spoof scores appear only here (root-only) and at debug log level — never in DBus replies or the normal journal.

---

## 5. Known Limitations

> [!CAUTION]
> The following limitations are **verified empirical constraints** observed on the target hardware (Intel i3 10th Gen, standard 720p USB webcam). They are known, documented, and do not represent bugs — they are physical and algorithmic boundaries of the default 2D RGB + 320×320 pipeline configuration.

| Limitation | Condition | Impact | Workaround |
|---|---|---|---|
| **Low light** | Dim room, screen light only | Dim frames are brightened for detection, but a face that is too dark fails the quality gate and is skipped; the scan ends as `TIMEOUT` → password | Improve ambient lighting, or enroll once more in the dim conditions you actually use. |
| **Distance > 60 cm detection failure** | Using default 320×320 SCRFD input resolution | SCRFD-500M at 320×320 cannot reliably detect faces below the 120px bounding box height quality gate at distances beyond ~60 cm | Set `scrfd_input_size = 640` in `/etc/sentinel/config.toml`. Increases total pipeline mean from ~33 ms to ~71 ms, but maintains reliable detection at distance. |
| **Face turned away** | Head turned or tilted well away from the camera | The quality gate skips the frame (it is not counted as a mismatch) | Face the camera. |
| **Near-identical twins / siblings** | Cosine distance may fall inside the Golden or Standard range | System may authenticate a sibling | Known limitation of 2D RGB recognition. Lower `golden_threshold` / `standard_threshold`, or do not use face unlock for `sudo` and login. |
| **IR camera unsupported** | Infrared-only or structured-light depth sensors | V4L2 capture and SCRFD are tuned for visible-spectrum 2D RGB | IR support is deferred to v2. Depth-based anti-spoofing is out of scope for v1. |

> [!NOTE]
> In all failure cases above, Sentinel fails **open-safe**: the daemon returns `PAM_IGNORE`, allowing PAM to fall through to the standard password prompt. No biometric failure will lock a user out of their system.

---

## 6. Tier Behavior in Practice

- **Good conditions**: a Golden frame with a clean anti-spoof score grants on the first usable frame.
- **Average conditions**: Standard frames (or Golden frames with a middling anti-spoof score) need three in a row — a fraction of a second longer.
- **Poor conditions**: frames fail the quality gate or land in the "not sure" tier; the scan runs to the timeout and the password prompt takes over.

The default `golden_threshold = 0.28` is calibrated for the maintainer's hardware (Intel i3, 720p webcam, Fedora 44). Users with different hardware may want to adjust this in `/etc/sentinel/config.toml`.


