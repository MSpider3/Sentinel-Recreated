# Complete Feature Registry — Sentinel Recreated

**Document**: `docs/FEATURE_REGISTRY.md`  
**Subsystem**: Complete System Architecture Scope

---

## 1. Feature Registry & Origin Traceability Matrix

| Feature Description | Source Origin | Status | Architectural Rationale |
|---|---|---|---|
| **SCRFD Face Detection (500M & 10G)** | Gaze Reference | **Include** | Superior detection accuracy over YuNet at extreme angles and small face scales. Outputs 5 key landmarks. |
| **5-Point Landmark Extraction** | Gaze Reference | **Include** | Essential for calculating facial pose geometry and alignment matrices. |
| **Affine Similarity Transformation (112×112)** | Gaze Reference | **Include** | **CRITICAL FIX**. Eliminates embedding distance skew caused by face rotation or tilt. |
| **MobileFaceNet Embedding Engine** | Gaze Reference | **Include** | Default recognizer. 512-dimensional output optimized for CPU execution on i3 10th gen targets. |
| **ArcFace ResNet50 Embedding Engine** | Gaze Reference | **Not implemented** | Too large and slow for the weak-CPU target; MobileFaceNet is the only recogniser. |
| **MiniFASNet Anti-Spoofing (V2 + V1SE)** | Prototype / Upstream | **Include** ✅ | Passive photo/screen check on every matching frame. V2 (2.7× crop) is required, V1SE (4.0× crop) is fused when installed. Fed exactly as upstream trains it (BGR, 0–255). |
| **MiniFASNet First-Run Self-Calibration** | Prototype | **Removed** | It guessed the input format from whoever sat in front of the camera, which hid a wrong input format and could learn a photo as "real". Replaced by a fixed, correct input format and a start-up self-check. |
| **4-Tier Decision Engine** | Prototype | **Include** | Tiers decide how many frames of proof are needed: Golden (one clean frame), Standard (three in a row), TwoFactor and near-miss Denied frames (keep looking), clearly-different frames (ten in a row end the scan). |
| **Eye Aspect Ratio (EAR) Blink Challenge** | Prototype | **Removed** | The 5-point landmarks cannot see eyelids, so the check was not real; a replayed video blinks anyway. |
| **Randomized Head Pose Challenge** | Prototype | **Removed** | Could not be shown on login screens, took up to 20 s, and passed when a photo was slid sideways. Replaced by multi-frame voting. |
| **Adaptive FIFO Gallery (Max 20)** | Prototype | **Include** | Learns gradual change. A new template must strongly match an *enrolled* template (never only a learned one), differ a little from what is stored, have a clean anti-spoof score, and respect the daily limit. |
| **Intrusion Photo Log** | Prototype | **Include** | Saves a photo after ten clearly-different frames in a row (distance 0.20 past `two_factor_threshold`). Newest 20 / 30 days kept. The embedding blacklist was removed: it could ban the owner after one bad frame and added no security. |
| **Pipe-Separated Audit Logging** | Prototype | **Include** | Essential for system auditability, security analysis, and log retention compliance. |
| **Kalman Filter Bounding Box Tracking** | Prototype | **Not implemented** | A scan lasts only a handful of frames; there is nothing to smooth. |
| **DBus System Service (`zbus`)** | Gaze Reference | **Include** | Linux standard IPC mechanism. Solves socket permission issues and integrates with PolicyKit. |
| **Thin C PAM Shared Module** | Gaze Reference | **Include** ✅ | PAM boundary requirement. Contains zero biometric code and fails safe with `PAM_IGNORE`. Installed to distro-correct path via `install_pam_module()`. |
| **Python CLI (`sentinel`)** | Prototype / New | **Include** | Convenient administration interface (`sentinel enroll`, `status`, `config`). |
| **Textual Dashboard TUI** | Prototype / New | **Include** | Terminal UI dashboard for real-time monitoring and configuration edits. |
| **OpenCV Interactive Enrollment Preview** | Prototype | **Include** | Provides real-time visual feedback to the user during multi-pose enrollment wizard. |
| **TOML Configuration System (`/etc/sentinel`)** | Gaze Reference | **Include** | Standard, human-readable config format natively supported by Rust (`serde` + `toml`). |
| **PipeWire / V4L2 Camera Capture Engine** | Gaze Reference | **Include** | Native Wayland and modern Linux camera subsystem support. |
| **Dark-Frame Brightening (replaces CLAHE)** | Prototype idea | **Include** | A simple brightness gain on the detector's copy of a dim frame. Recognition and anti-spoofing see the untouched frame. |
| **Frame Quality Gate** | New | **Include** | Skips frames that are turned away, cut off, too dark, too bright or blurred before they can count against the user. |
| **Attempt Limiter** | New | **Include** | Five failed face attempts per user per minute pause face unlock (password still works). |
| **TPM Template Encryption** | Gaze Reference | **Defer (v2)** | Advanced hardware security feature; deferred to v2 to focus on core stability. |
| **Infrared (IR) Camera Sensor Support** | Gaze Reference | **Defer (v2)** | Deferred until IR target hardware baseline is defined. |
| **GNOME Shell Status Bar Extension** | Gaze Reference | **Defer (v2)** | Desktop GUI integration; deferred to v2. |
| **Audio / TTS Voice Guidance** | Legacy Production | **SKIP** | Adds unnecessary heavy dependencies (`pyttsx3`) without security benefit. |
| **GTK4 / Vala GUI Dashboard** | Legacy Production | **SKIP** | Replaced entirely by lighter Textual TUI dashboard. |
