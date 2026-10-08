# Data Storage & Filesystem Layout Specification — Sentinel Recreated

**Document**: `docs/DATA_STORAGE.md`  
**Subsystem**: `sentinel-core/src/gallery/` & Storage Layer

---

## 1. Directory Structure & Permission Matrix

All storage locations in **Sentinel Recreated** are fixed system paths. Permissions are enforced by the daemon on startup (`chmod`/`chown` checks).

| Absolute Path | Content Description | Owner:Group | Mode |
|---|---|---|---|
| `/var/lib/sentinel/` | Base data directory for user galleries & blacklists | `root:root` | `0700` |
| `/var/lib/sentinel/users/<user>/gallery.npy` | Core enrollment embedding vectors ($N \times 512$) | `root:root` | `0600` |
| `/var/lib/sentinel/users/<user>/adaptive.npy` | Adaptive gallery vectors ($M \times 512$, $M \le 20$) | `root:root` | `0600` |
| `/var/lib/sentinel/users/<user>/meta.json` | Adaptive-gallery counters (last adaptation date, count today) | `root:root` | `0600` |
| `/var/lib/sentinel/blacklist/intrusion_*.jpg` | Intrusion photos ($640 \times 480$ JPEG); newest 20, at most 30 days old | `root:root` | `0600` |
| `/var/cache/sentinel/models/` | ONNX model binaries directory | `root:root` | `0755` |
| `/etc/sentinel/config.toml` | System runtime configuration file | `root:root` | `0644` |
| `/var/log/sentinel/` | System authentication audit logs | `root:root` | `0750` |

Gallery, metadata and config files are written to a temporary file and renamed into place, so a crash never leaves a half-written file.

Files from earlier versions that are no longer read and can be deleted: `/var/lib/sentinel/blacklist/embeddings.npy`, `/var/lib/sentinel/minifas_calib.json`.

---

## 2. Binary Embedding Storage Format

User biometric templates are stored as NumPy 1.2+ array binaries (`.npy`):

### Core Gallery (`gallery.npy`)
- **Shape**: $(N, 512)$ where $N = \text{poses} \times \text{subsamples}$.
  - Base Enrollment: $5 \text{ poses} \times 3 \text{ samples} = 15$ vectors.
  - Glasses Variant Enrollment: $10 \text{ poses} \times 3 \text{ samples} = 30$ vectors. Maximum 40.
- **DataType**: `float32` (IEEE 754 32-bit floating point).
- **L2-Norm Guarantee**: Each vector $\mathbf{v}$ satisfies $\|\mathbf{v}\|_2 = 1.0 \pm 10^{-6}$.
- **Immutability**: Core gallery vectors are written exclusively during enrollment and are never modified by automatic adaptive routines.

### Adaptive Gallery (`adaptive.npy`)
- **Shape**: $(M, 512)$ where $0 \le M \le 20$.
- **Eviction Policy**: First-In, First-Out (FIFO). When $M = 20$, the oldest adaptive vector (row 0) is discarded to make room for new Tier 1 adaptation entries.
- **Use at Inference**: a probe is matched against both files; the nearest template decides the tier. Learning a new template additionally requires a strong match in `gallery.npy` itself.
- **Reset on re-enrollment**: `FinishEnrollment` deletes `adaptive.npy` and `meta.json`.

---

## 3. User Metadata Schema (`meta.json`)

```json
{
  "last_adaptation_date": "2026-07-21",
  "today_count": 1,
  "total_count": 4
}
```

The `GetUserInfo` DBus method does not read this file for counts; it counts the vectors in `gallery.npy` and `adaptive.npy` directly.

---

## 4. ONNX Model Binaries Registry

Model files are cached at `/var/cache/sentinel/models/`:

| File Name | Upstream Source | Dimensions / Input Size | Purpose |
|---|---|---|---|
| `scrfd_500m_kps.onnx` | InsightFace Buffalo_SC | $1 \times 3 \times 640 \times 640$ | Face detection & 5 landmarks |
| `mobile_facenet.onnx` | InsightFace Model Zoo | $1 \times 3 \times 112 \times 112$ | 512-d feature extraction (Default) |
| `MiniFASNetV2.onnx` | Silent-Face-Anti-Spoofing (ONNX: yakhyo/face-anti-spoofing) | $1 \times 3 \times 80 \times 80$, crop scale 2.7 | Anti-spoofing (required) |
| `MiniFASNetV1SE.onnx` | same | $1 \times 3 \times 80 \times 80$, crop scale 4.0 | Anti-spoofing (fused when present) |

`setup.sh` and `scripts/download_models.sh` verify the SHA-256 of every model file.
