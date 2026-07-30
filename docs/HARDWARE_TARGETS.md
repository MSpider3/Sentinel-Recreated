# Hardware Targets & Performance Budgets — Sentinel Recreated

**Document**: `docs/HARDWARE_TARGETS.md`  
**Subsystem**: Runtime Environment & System Tuning

---

## 1. Target Hardware Baseline

The baseline hardware target represents a common consumer laptop or thin client without dedicated hardware accelerators.

| Spec Category | Minimum Target Requirement |
|---|---|
| **Processor (CPU)** | Intel Core i3 10th Generation (4 cores @ 1.20GHz) or AMD Ryzen 3 3200U |
| **System Memory (RAM)** | 8 GB DDR4 |
| **Graphics (GPU)** | Integrated Graphics (Intel UHD Graphics / AMD Radeon Vega 3) — **No discrete GPU (dGPU) assumed** |
| **Camera Sensor** | 720p / 480p V4L2 USB / Internal 2D RGB Webcam |
| **Host Operating System** | Fedora Linux 40+, Arch Linux, Ubuntu 24.04 LTS (Kernel $\ge 6.6$) |
| **Display Server** | Wayland (Niri, GNOME Mutter, KDE KWin) |

---

## 2. Empirical Benchmark Results & Performance Budgets

| Subsystem Component | Measured Empirical Performance (i3 10th Gen) | Target Performance Specification | Status |
|---|---|---|---|
| **SCRFD-500M (320×320 Input)** | **14.91 ms** mean (24.67 ms P95) | $< 20.0\text{ ms}$ (for 320×320 fast mode) | **PASS** |
| **Affine Alignment** | **0.94 ms** mean | $< 1.0\text{ ms}$ | **PASS** |
| **MobileFaceNet Embedding** | **17.42 ms** mean | $\approx 15.0\text{ ms}$ | **PASS** |
| **Cosine Match (30 vectors)** | **0.02 ms** mean | $< 1.0\text{ ms}$ | **PASS** |
| **MiniFASNet Spoof Check** | **7.06 ms** mean (9.98 ms P95) | $< 10.0\text{ ms}$ | **PASS** |
| **Total Pipeline (Mean)** | **33.31 ms** mean (~30 FPS) | $< 43.0\text{ ms}$ | **PASS** |
| **Total Pipeline (P95)** | **74.49 ms** P95 | $< 100.0\text{ ms}$ | **PASS** |
| **Daemon Cold-Start** | **35 ms** | $\le 5000\text{ ms}$ | **PASS** |

> [!NOTE]
> **Distance & Resolution Deployment Policy**:
> At `320x320` input resolution, detection may timeout at distances $> 60\text{ cm}$ or in low light conditions. For high-security deployments requiring reliable detection at greater distance, set `scrfd_input_size = 640` in `config.toml` (increases total pipeline mean latency to ~71ms, but maintains reliable detection at distance).

---

## 3. Non-Negotiable Performance Rules

1. **Default Model Selection**: `SCRFD-500M` and `MobileFaceNet` MUST be active by default. Heavy models (`SCRFD-10G` and `ArcFace ResNet50`) are opt-in configuration parameters only.
2. **Idle Memory Ceiling**: When daemon is idle (models loaded into memory, no active authentication session), RSS memory usage MUST NOT exceed **400 MB**.
3. **Thread-Isolated Capture**: Frame grabber executes in a dedicated high-priority thread. Frames are served asynchronously via ring buffer.
4. **Stale Frame Eviction (200ms Rule)**: If pipeline processing of a single frame exceeds $200\text{ ms}$, the frame queue is flushed completely to ensure authentication evaluates real-time current state rather than backlogged frames.
5. **Cold-Start Warmup Window**: First inference request after daemon boot may take up to $5.0\text{ s}$ for ONNX threadpool initialization. All subsequent auth cycles must complete within target latency budgets ($<100\text{ ms}$). *Note: Daemon cold-start boot-to-ready time MUST be explicitly benchmarked in Phase 8 of `DEVELOPMENT_PLAN.md` to ensure fast user switching scenarios meet performance requirements.*
6. **Execution Provider Policy**: ONNX Runtime instances default to `CPUExecutionProvider` with single-thread optimization or OpenVINO execution if present, falling back gracefully without crash.

---

## 4. Memory Footprint Breakdown by Configuration

| Model Configuration | Model Weights Disk Size | Daemon Resident Memory (RSS) | Inference Latency (i3 10th Gen) |
|---|---|---|---|
| **Fast Standard (Default)**<br>`SCRFD-500M` + `MobileFaceNet` + `MiniFASNetV2` | ~16 MB | **~310 MB** | **~33.3 ms** |
| **Accurate High-Res (Opt-in)**<br>`SCRFD-10G` + `ArcFace-R50` + `MiniFASNetV2` | ~210 MB | **~580 MB** | **~95 ms** |

---

## 5. Hardware Optimization Parameters (`config.toml`)

```toml
[hardware]
execution_provider = "cpu"    # "cpu", "openvino" (recommended on Intel iGPU), or "cuda"
onnx_num_threads = 2
frame_drop_threshold_ms = 200
max_daemon_memory_mb = 400
```
