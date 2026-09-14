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
| **ArcFace ResNet50 Embedding Engine** | Gaze Reference | **Include (Opt-in)** | High-precision recognition engine for high-end CPU/GPU targets. |
| **MiniFASNetV2 Anti-Spoofing** | Prototype | **Include** ✅ | Preserved from prototype baseline. Provides static Fourier texture anti-spoofing. |
| **MiniFASNet First-Run Self-Calibration** | Prototype | **Include** ✅ | Preserved from prototype baseline. Tests channel permutations against camera sensor on first run; result cached to `/var/lib/sentinel/minifas_calib.json`. |
| **4-Tier Security Decision Engine** | Prototype | **Include** | Flexible securityUX: Golden (instant), Standard (liveness), 2FA (password), Denied (lock). |
| **Eye Aspect Ratio (EAR) Blink Challenge** | Prototype | **Include** | Corrected EAR state machine providing passive/active liveness validation. |
| **Randomized Head Pose Challenge** | Prototype | **Include** | Active liveness challenge (Turn Left/Right, Tilt Up/Down) prevents video playback spoofs. |
| **Adaptive FIFO Gallery (Max 20)** | Prototype | **Include** | Learns facial drift over time while enforcing daily rate limits and Tier 1 restrictions. |
| **Intrusion Screenshot & Vector Capture** | Prototype | **Include** | Logs Tier 4 failed attempts as JPEG images and populates blacklist queue. |
| **Pipe-Separated Audit Logging** | Prototype | **Include** | Essential for system auditability, security analysis, and log retention compliance. |
| **Kalman Filter Bounding Box Tracking** | Prototype | **Include** | Smooths face bounding box movement between frames to reduce jitter (landmarks from SCRFD remain per-frame). |
| **DBus System Service (`zbus`)** | Gaze Reference | **Include** | Linux standard IPC mechanism. Solves socket permission issues and integrates with PolicyKit. |
| **Thin C PAM Shared Module (<200 LOC)** | Gaze Reference | **Include** ✅ | PAM boundary requirement. Contains zero biometric code and fails safe with `PAM_IGNORE`. Installed to distro-correct path via `install_pam_module()`. |
| **Python CLI (`sentinel`)** | Prototype / New | **Include** | Convenient administration interface (`sentinel enroll`, `status`, `config`). |
| **Textual Dashboard TUI** | Prototype / New | **Include** | Terminal UI dashboard for real-time monitoring and configuration edits. |
| **OpenCV Interactive Enrollment Preview** | Prototype | **Include** | Provides real-time visual feedback to the user during multi-pose enrollment wizard. |
| **TOML Configuration System (`/etc/sentinel`)** | Gaze Reference | **Include** | Standard, human-readable config format natively supported by Rust (`serde` + `toml`). |
| **PipeWire / V4L2 Camera Capture Engine** | Gaze Reference | **Include** | Native Wayland and modern Linux camera subsystem support. |
| **CLAHE Preprocessing Pipeline** | Prototype | **Include** | Preserved from prototype baseline. Equalizes poor ambient lighting conditions. |
| **TPM Template Encryption** | Gaze Reference | **Defer (v2)** | Advanced hardware security feature; deferred to v2 to focus on core stability. |
| **Infrared (IR) Camera Sensor Support** | Gaze Reference | **Defer (v2)** | Deferred until IR target hardware baseline is defined. |
| **GNOME Shell Status Bar Extension** | Gaze Reference | **Defer (v2)** | Desktop GUI integration; deferred to v2. |
| **Audio / TTS Voice Guidance** | Legacy Production | **SKIP** | Adds unnecessary heavy dependencies (`pyttsx3`) without security benefit. |
| **GTK4 / Vala GUI Dashboard** | Legacy Production | **SKIP** | Replaced entirely by lighter Textual TUI dashboard. |
