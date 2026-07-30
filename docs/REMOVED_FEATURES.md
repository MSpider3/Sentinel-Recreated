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
| **Unix Domain Sockets + JSON-RPC IPC** | Old Production | **Fragile File Permissions**. PAM runs under various EUIDs (`root`, `gdm`, `mehulgolecha`), causing socket access errors. DBus handles bus permissions natively via PolicyKit. |
| **SFace Embedding Model (128-d)** | Prototype | **Weak Feature Discrimination**. SFace produces 128-d vectors that exhibit high cosine similarity variance under minor pose shifts. Replaced by 512-d MobileFaceNet/ArcFace. |
| **YuNet Face Detector** | Prototype | **Missing Landmarks**. YuNet detection does not reliably output the 5 canonical keypoints required for affine alignment matrices. Replaced by SCRFD. |
| **Direct Bounding-Box Bypassing Alignment** | Prototype | **PRIMARY RECOGNITION FAILURE CAUSE**. Feeding unaligned face crops into embedding extractors corrupts distance metrics. Fixed by mandatory 112×112 5-point similarity warping. |
| **Tkinter Intrusion Review UI** | Prototype | **Threading & Event Loop Conflicts**. Running Tkinter alongside OpenCV camera streams caused X11 threading locks. Replaced by Textual TUI. |
| **Hardcoded Global Session Timeouts** | Prototype | **Rigid Detector Logic**. Detector state timeouts are now dynamically configurable parameters in `/etc/sentinel/config.toml`. |
| **Vala / GTK4 Desktop GUI** | Old Production | **Unnecessary Bloat**. Native GTK compilation added complex build toolchain dependencies (`valac`, GTK libraries) for simple admin tasks. Replaced by Textual TUI. |
| **`pam_exec.so` Shell Script Integration** | Legacy Attempt | **Fragile Process Spawning**. Invoking shell scripts from PAM creates execution latency and handles errors poorly. Replaced by compiled C module `pam_sentinel.so`. |
| **Python Biometric Engine Daemon** | Prototype | **Python GIL & High Memory Usage**. Python runtime memory footprint exceeded 800MB and GIL limited multi-threaded ONNX pipeline performance. Replaced by compiled Rust core. |
| **Audio / TTS Voice Guidance** | Legacy Production | **Bloat & Audio Server Conflicts**. TTS engines (`pyttsx3`) frequently lock PipeWire/ALSA sound cards during PAM login prompts, blocking user desktop sessions. |
