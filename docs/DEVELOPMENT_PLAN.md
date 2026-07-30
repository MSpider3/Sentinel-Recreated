# Phased Development Plan — Sentinel Recreated

**Document**: `docs/DEVELOPMENT_PLAN.md`  
**Subsystem**: Project Delivery Roadmap & Milestones

---

## 1. Phased Execution Overview

The implementation of **Sentinel Recreated** follows a strict linear sequence: system cleanup and documentation MUST be verified prior to writing any compiled biometric core or client code.

```
┌──────────────┐     ┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│ Phase 0:     │────►│ Phase 1:     │────►│ Phase 2:     │────►│ Phase 3:     │
│ Cleanup &    │     │ Rust FRS     │     │ Gallery &    │     │ DBus Core    │
│ Specifications     │ Pipeline     │     │ Audit Log    │     │ Service      │
└──────────────┘     └──────────────┘     └──────────────┘     └──────────────┘
                                                                      │
┌──────────────┐     ┌──────────────┐     ┌──────────────┐            │
│ Phase 7/8:   │◄────│ Phase 5/6:   │◄────│ Phase 4:     │◄───────────┘
│ Deploy & Test│     │ Python Tools │     │ C PAM        │
│ Tuning       │     │ & TUI        │     │ Wrapper      │
└──────────────┘     └──────────────┘     └──────────────┘
```

---

## 2. Phase Breakdown & Acceptance Criteria

### Phase 0 — Environment Cleanup & Specification ✅ Complete (2026-07-21)
- **Goal**: Clean legacy installation traces from the host system and author complete documentation suite before creating source files.
- **Deliverables**:
  - Environment cleanup verification script.
  - All 12 specification documents created in `sentinel_recreated/docs/`.
  - Directory structure initialized (`sentinel-core/`, `pam-sentinel/`, `sentinel-py/`, `packaging/`).
- **Acceptance Criteria**: All 12 `.md` files present in `docs/`; user explicit sign-off on `ARCHITECTURE.md`. ✅ **Met.**

---

### Phase 1 — Rust Core: FRS Biometric Engine Pipeline ✅ Complete (2026-07-23)
- **Goal**: Implement standalone Rust pipeline modules without DBus or PAM wrappers.
- **Deliverables**:
  - `sentinel-core/src/pipeline/detect.rs` (SCRFD ONNX inference)
  - `sentinel-core/src/pipeline/align.rs` (5-point affine similarity matrix)
  - `sentinel-core/src/pipeline/embed.rs` (MobileFaceNet ONNX inference)
  - `sentinel-core/src/pipeline/match.rs` (Cosine distance & tier logic)
  - `sentinel-core/src/pipeline/spoof.rs` (MiniFASNet ONNX checker)
  - `sentinel-core/src/pipeline/liveness.rs` (EAR blink state machine & head pose)
  - `sentinel-core/src/pipeline/capture.rs` (Threaded V4L2 capture ring-buffer)
- **Acceptance Criteria**: `cargo test` passes for each pipeline stage; CLI test binaries (`cargo run --bin enroll-test` and `auth-test`) successfully detect, align, embed, and match local webcam frames. ✅ **Met.**

---

### Phase 2 — Rust Core: Gallery Management & Structured Audit System ✅ Complete (2026-07-24)
- **Goal**: Implement persistent embedding storage, adaptive FIFO updating, and audit logging.
- **Deliverables**:
  - `sentinel-core/src/gallery/store.rs` (`gallery.npy` read/write)
  - `sentinel-core/src/gallery/adaptive.rs` (FIFO 20-vector buffer & daily rate-limiter)
  - `sentinel-core/src/audit.rs` (Pipe-separated log writer for `/var/log/sentinel/`)
  - `sentinel-core/src/config.rs` (Serde TOML configuration parser)
- **Acceptance Criteria**: Core and adaptive vectors successfully save and load with `0600` root permissions; rate-limiting logic rejects multiple adaptive writes on the same calendar day. ✅ **Met.**

---

### Phase 3 — Rust Core: DBus Service Daemon ✅ Complete (2026-07-24)
- **Goal**: Wrap FRS pipeline and storage layer into a root-level system daemon exposing DBus methods via `zbus`.
- **Deliverables**:
  - `sentinel-core/src/dbus/service.rs` (DBus interface implementation of `com.sentinel.Sentinel`)
  - `sentinel-core/src/main.rs` (Daemon lifecycle, signal handling, zbus bus name claim)
  - `packaging/sentinel.service` (Systemd unit definition)
- **Acceptance Criteria**: `busctl call com.sentinel.Sentinel /com/sentinel/Sentinel com.sentinel.Sentinel GetStatus` returns daemon health JSON. ✅ **Met.**

---

### Phase 4 — C PAM Dynamic Module ✅ Complete (2026-07-25)
- **Goal**: Build thin C PAM shared object wrapper (`pam_sentinel.so`).
- **Deliverables**:
  - `pam-sentinel/pam_sentinel.c` (<200 LOC DBus client caller)
  - `pam-sentinel/meson.build` (Meson build file)
- **Acceptance Criteria**: `pam_sentinel.so` installed to the correct distro-specific PAM security directory; `sudo` facial authentication succeeds when daemon is active and safely falls back to password (`PAM_IGNORE`) when daemon service is stopped. ✅ **Met.**

---

### Phase 5 — Python Management CLI & Enrollment Wizard ✅ Complete (2026-07-26)
- **Goal**: Develop end-user CLI tool and interactive OpenCV face enrollment wizard.
- **Deliverables**:
  - `sentinel-py/cli.py` (`sentinel enroll`, `list`, `remove`, `status`)
  - `sentinel-py/enroll.py` (OpenCV live preview & gesture state machine)
  - `sentinel-py/dbus_client.py` (DBus IPC wrapper)
- **Acceptance Criteria**: Running `sentinel enroll mehulgolecha` displays camera window, guides user through 5 poses, and registers valid `gallery.npy` vectors with daemon. ✅ **Met.**

---

### Phase 6 — Textual Terminal Dashboard (TUI) ✅ Complete (2026-07-27)
- **Goal**: Build interactive Terminal UI dashboard for monitoring and settings management.
- **Deliverables**:
  - `sentinel-py/tui/app.py` & screen modules
- **Acceptance Criteria**: Launching `sentinel dashboard` renders live distance meters, status indicators, and settings controls. ✅ **Met.**

---

### Phase 7 — Automated Installer & Packaging ✅ Complete (2026-07-30)
- **Goal**: Create idempotent deployment scripts and system configuration installers.
- **Deliverables**:
  - `setup.sh` (Downloads ONNX models, compiles Rust/C, installs Python package, provisions systemd/PolicyKit; environment-detection system for all major distros/DMs/lock screens; `--dry-run` flag)
  - `uninstall.sh` (Clean uninstaller covering all known PAM files)
  - `packaging/com.sentinel.policy` (PolicyKit action definitions)
- **Acceptance Criteria**: Running `sudo ./setup.sh` on a clean system produces a fully functional, auto-starting `sentinel.service`; `--dry-run` correctly detects distro, DM, and lock screen before touching any files. ✅ **Met.**

---

### Phase 8 — System Calibration, Benchmarking & Integration Testing ✅ Complete (2026-07-30)
- **Goal**: Tune distance thresholds on target hardware, benchmark daemon startup/auth performance, and execute comprehensive test suite.
- **Deliverables**:
  - Pipeline benchmark suite (`tests/benchmark.rs`)
  - DBus integration test script (`tests/test_dbus.py`)
  - Daemon cold-start & warmup latency benchmark (`tests/bench_coldstart.rs`)
- **Acceptance Criteria**:
  - Steady-state end-to-end authentication latency verified $< 100\text{ ms}$ on target Intel i3 10th Gen hardware. ✅ **Met (33.31 ms mean, 74.49 ms P95).**
  - Daemon cold-start startup time to model-ready state verified $\le 5.0\text{ s}$ (critical for fast user switching scenarios). ✅ **Met (35 ms cold-start).**
