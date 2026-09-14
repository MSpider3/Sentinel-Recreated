# Architecture Specification — Sentinel Recreated

**Project Identifier**: `sentinel_recreated`  
**System Name**: Sentinel Recreated Facial Biometric Authentication Framework

---

## 1. System Overview

Sentinel Recreated is a low-latency, privacy-focused facial recognition authentication system designed for Linux environments (specifically modern distros running systemd, Wayland, and PipeWire/V4L2). It replaces legacy password-only local authentication (e.g., GDM login, `sudo`, lock screens) with a high-accuracy, 4-tiered biometric challenge system, retaining password fallback for security and robustness.

The architecture strictly separates high-performance biometric processing from privilege control and user interfaces.

---

## 2. Language & Subsystem Boundaries

| Subsystem / Layer | Implementation Language | Justification & Architectural Boundary |
|---|---|---|
| **Biometric Engine & Core Daemon** (`sentinel-core`) | **Rust** | Critical for zero-cost abstractions, memory safety, thread concurrency without GIL, and predictable <100ms processing times. Interfaces with ONNX Runtime (`ort`) and handles hardware camera capture via V4L2/GStreamer. |
| **PAM Security Integration Module** (`pam-sentinel`) | **C** (Thin Wrapper) | PAM modules in Linux must compile to standard C dynamic libraries (`.so`). To prevent memory leaks, crashes, or unhandled exceptions in critical auth paths, this module contains **zero** biometric logic—it solely issues synchronous DBus IPC calls to the daemon. |
| **CLI & Enrollment Wizard** (`sentinel-py`) | **Python 3** | Rapid UI development, rich interactive TUI via `Textual`, and OpenCV-based live camera feedback during enrollment. Interacts with the Rust daemon exclusively via DBus. |
| **Privilege Escalation & IPC** | **DBus + PolicyKit** | Standard system IPC bus (`com.sentinel.Sentinel`). Unprivileged clients interact with system methods controlled by PolicyKit rules (`com.sentinel.policy`). |
| **Service Management** | **Native systemd Unit** | `sentinel.service` running as root for standard service life cycle, logging (`journalctl`), auto-restart, and hardware device access. |

---

## 3. Directory Layout

```
sentinel_recreated/
├── sentinel-core/                  # Rust: System Daemon (gazed equivalent)
│   ├── src/
│   │   ├── main.rs                 # Daemon entry point, signal handling, zbus setup
│   │   ├── pipeline/               # FRS Pipeline modules
│   │   │   ├── mod.rs
│   │   │   ├── capture.rs          # V4L2 / PipeWire frame streaming thread
│   │   │   ├── detect.rs           # SCRFD-500M ONNX detection & 5-landmark extraction
│   │   │   ├── align.rs            # 5-point affine transformation matrix (112×112)
│   │   │   ├── embed.rs            # MobileFaceNet / ArcFace ONNX inference
│   │   │   ├── match.rs            # Cosine distance computation & tier decision engine
│   │   │   ├── spoof.rs            # MiniFASNetV2 anti-spoofing checker
│   │   │   └── liveness.rs         # MediaPipe EAR blink & head-pose state machine
│   │   ├── gallery/                # Embedding store, adaptive FIFO gallery & blacklist
│   │   │   ├── mod.rs
│   │   │   ├── store.rs
│   │   │   └── adaptive.rs
│   │   ├── dbus/                   # DBus interface implementation (zbus crate)
│   │   │   ├── mod.rs
│   │   │   └── service.rs
│   │   ├── config.rs               # config.toml parser and validator
│   │   └── audit.rs                # Structured pipe-separated audit log writer
│   └── Cargo.toml
│
├── pam-sentinel/                   # C: PAM Shared Object Wrapper
│   ├── pam_sentinel.c              # DBus RPC caller (< 200 lines)
│   └── meson.build                 # Build definition
│
├── sentinel-py/                    # Python: User Utilities & Management
│   ├── cli.py                      # Main entrypoint (`sentinel enroll`, `sentinel status`, etc.)
│   ├── enroll.py                   # OpenCV interactive enrollment wizard
│   ├── dbus_client.py              # DBus communication wrapper
│   └── tui/                        # Textual terminal UI dashboard
│       ├── app.py
│       └── screens/
│           ├── dashboard.py
│           ├── live_test.py
│           └── settings.py
│
├── packaging/                      # System deployment files
│   ├── sentinel.service            # systemd service unit
│   ├── com.sentinel.Sentinel.xml   # DBus introspection XML schema
│   ├── com.sentinel.policy         # PolicyKit action rules
│   └── 99-sentinel-webcam.rules    # udev device permission rules
│
├── models/                         # ONNX Model cache (downloaded by setup.sh)
│   ├── scrfd_500m_kps.onnx
│   ├── mobile_facenet.onnx
│   └── MiniFASNetV2.onnx
│
├── docs/                           # System specification documentation
├── tests/                          # Automated integration and unit test suite
├── Cargo.toml                      # Workspace root manifest
├── meson.build                     # Top-level meson build file
├── pyproject.toml                  # Python package configuration
├── config.toml.default             # Default configuration template
├── setup.sh                        # Automated deployment script
└── uninstall.sh                    # Complete system clean uninstaller
```

---

## 4. System Intercommunication Architecture

```
                       ┌─────────────────────────┐
                       │  PAM Trigger Event      │
                       │  (GDM / Sudo / Lock)    │
                       └───────────┬─────────────┘
                                   │
                                   ▼
                       ┌─────────────────────────┐
                       │   pam_sentinel.so (C)   │
                       │   - Get pam_user        │
                       │   - Check daemon online │
                       └───────────┬─────────────┘
                                   │ DBus Method Call:
                                   │ com.sentinel.Sentinel.Authenticate(username)
                                   ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  sentinel-daemon (Rust Core Service — Root Systemd Unit)                   │
│                                                                             │
│  1. Camera Capture ──► Frame Grabber (V4L2/PipeWire, dark-frame skip)      │
│  2. SCRFD Detect   ──► Bounding box + 5 facial landmarks                    │
│  3. Affine Warp    ──► Transform 5 landmarks to 112×112 canonical template  │
│  4. MobileFaceNet  ──► Extract 512-dimensional normalized embedding          │
│  5. Gallery Match  ──► Min cosine distance calculation against gallery      │
│  6. Anti-Spoof     ──► MiniFASNet score validation                           │
│  7. Tier Decision  ──► Determine Tier 1/2/3/4                               │
│  8. Liveness Check ──► Blink EAR state machine (if Tier 2/3)                 │
└──────────────────────────┬──────────────────────────────────────────────────┘
                           │
                           │ Returns: "GRANTED" | "DENIED" | "REQUIRE_2FA" | "TIMEOUT" | "NO_FACE"
                           ▼
               ┌─────────────────────────┐
               │   pam_sentinel.so (C)   │
               │   - GRANTED    -> SUCCESS│
               │   - DENIED     -> AUTH_ERR│
               │   - Error/Off  -> IGNORE │
               └─────────────────────────┘
```

---

## 5. Architectural Integrity Rules

1. **Strict Daemon Authority**: The PAM module, CLI client, and TUI dashboard **never** touch model ONNX files, embedding files (`.npy`), or raw camera hardware directly. All operations are brokered by the daemon via DBus.
2. **Fail-Safe PAM Policy**: The C PAM module must always return `PAM_IGNORE` if the daemon is unavailable, offline, or experiencing hardware timeouts. Under no circumstances may a daemon failure lock a user out of password login.
3. **Canonical Alignment Invariance**: Embeddings are calculated **only** from 112×112 affine-aligned crops. Unaligned crops are strictly rejected at the core pipeline boundary.
