# FRS Pipeline Specification — Sentinel Recreated

**Document**: `docs/FRS_PIPELINE.md`  
**Subsystem**: `sentinel-core/src/pipeline/`

---

## 1. Prototype Failure Analysis & Paradigm Shift

The legacy prototype failed in real-world authentication scenarios due to three critical architectural flaws:
1. **Unaligned Feature Extraction**: The prototype used raw bounding box crops directly passed to feature extractors. Facial tilt or yaw variations produced distinct vector representations for the exact same identity.
2. **Suboptimal Model Selection**: SFace (128-dimensional output) lacks sufficient angular discrimination compared to modern 512-dimensional ArcFace/MobileFaceNet embeddings.
3. **Unreliable Active Challenges**: The prototype's blink and head-turn challenges were slow, could be passed by sliding a photo, and added seconds to every login.

**Sentinel Recreated** fixes these defects by establishing a mandatory 5-point similarity transformation stage, standardizing on 512-d embeddings, and replacing active challenges with passive anti-spoofing plus multi-frame evidence.

---

## 2. The Authentication Pipeline

Every new camera frame goes through the same steps. All of them run on one frame, so the face that matched is always the face that was checked for spoofing.

```
Capture ─► Warm-up ─► Detect ─► Quality gate ─► Align ─► Embed ─► Match ─► Anti-spoof ─► Decision
```

Source: `sentinel-core/src/pipeline/authenticator.rs` (per-frame flow) and `decision.rs` (grant rules).

### Step 1 — Frame Capture (`capture.rs`)
- **Source**: V4L2 device via GStreamer (`v4l2src`), or `pipewiresrc`.
- **Format**: the configured `camera.width` × `camera.height` (default 640×480) at up to `camera.fps`. If the camera does not offer that mode, capture falls back to whatever it does offer and centre-crops to the configured aspect ratio (never stretches).
- **Frame numbering**: every frame carries a sequence number. The pipeline waits for a *new* frame; no frame is processed twice.
- **Errors**: a camera error (busy device, unplugged) ends the scan at once with `NO_FACE`, so the password prompt appears without waiting for the timeout.

### Step 2 — Warm-up
A camera that was just switched on needs a moment for auto-exposure. Frames are skipped until the mean brightness is above 15 and changes by at most 3 between two consecutive frames, or 1 s has passed.

### Step 3 — Face Detection (`detect.rs`)
- **Model**: `scrfd_500m_kps.onnx`, input `scrfd_input_size` (default 320).
- **Letterbox**: the frame is shrunk keeping its aspect ratio and padded with black, as SCRFD expects.
- **Dark frames**: when the frame is dim (mean pixel value < 90) the detector's copy is brightened. Recognition and anti-spoofing always use the untouched frame.
- **Parameters**: score threshold `0.50`, NMS IoU `0.30`, minimum face size `min_face_size_px` (default 80 px).
- The largest face is the one being authenticated.

### Step 4 — Quality Gate (`quality.rs`)
A frame that fails is skipped. It is never counted as a failed match.
- All five landmarks inside the frame.
- Roughly frontal: sideways turn, up/down tilt and roll within limits, measured from the five landmarks.
- Aligned face not too dark, not too bright, and not blurred (variance of the Laplacian).

### Step 5 — 5-Point Alignment (`align.rs`)
Similarity transform mapping the detected landmarks onto the ArcFace 112×112 template:

- Left Eye: $(38.2946, 51.6963)$ · Right Eye: $(73.5318, 51.5014)$ · Nose Tip: $(56.0252, 71.7366)$
- Left Mouth Corner: $(41.5493, 92.3655)$ · Right Mouth Corner: $(70.7299, 92.2041)$

*Enrollment and authentication use the same alignment code.*

### Step 6 — Embedding (`embed.rs`)
`mobile_facenet.onnx` → 512-d vector, L2-normalised. Enrollment additionally embeds the mirrored face and sums the two before normalising.

### Step 7 — Matching (`match.rs`)
Cosine distance $d = 1 - \mathbf{e}_A \cdot \mathbf{e}_B$ to the nearest template of the target user (enrolled + learned). The distance picks a tier:

| Tier | Distance | Meaning |
|---|---|---|
| **Golden** | $d <$ `golden_threshold` (0.28) | strong match |
| **Standard** | $d <$ `standard_threshold` (0.42) | normal match |
| **TwoFactor** | $d \le$ `two_factor_threshold` (0.50) | not sure |
| **Denied** | above | no match |

### Step 8 — Anti-Spoofing (`spoof.rs`)
- Runs only for Golden and Standard frames.
- **Models**: `MiniFASNetV2.onnx` (face crop 2.7× the face box) and, when installed, `MiniFASNetV1SE.onnx` (4.0×). Their "real" probabilities are averaged.
- **Input**: 80×80, **BGR**, raw 0–255 values, plain bilinear resize; the crop keeps the face box's aspect ratio and is shifted to stay inside the frame — exactly as the models were trained.
- Each model is run once on a blank input at start-up as a self-check.
- **Fail closed**: if no anti-spoof model is loaded, the daemon refuses every request (password still works).

### Step 9 — Decision (`decision.rs`)
The tier decides how much proof is needed. There are no blink or head-turn challenges.

| Frame | Effect |
|---|---|
| Golden, anti-spoof ≥ `spoof_threshold` (0.80) | **Granted at once** |
| Golden or Standard, anti-spoof ≥ `spoof_threshold_standard` (0.70) | counts towards **3 such frames in a row → Granted** |
| Golden or Standard, anti-spoof below that | spoof strike; `max_retries` (3) strikes → **SPOOF**. Strikes are never forgiven within a session |
| TwoFactor | resets the streaks; keep looking |
| Denied, distance below `two_factor_threshold` + 0.20 | no match *yet* (the owner at an awkward angle scores 0.50–0.60): keep looking |
| Denied, distance at or above that (0.70) | clearly someone else; 10 in a row → **DENIED** and an intrusion photo is saved |

Session limits: no face for 3 s → `NO_FACE`; `global_session_timeout` (default and maximum 7 s) → `TIMEOUT`.

Every result except `GRANTED` makes `pam_sentinel.so` step aside for the password prompt.

---

## 3. Hardware Performance Budget (Intel i3-10100U Target)

| Pipeline Step | Target | Verified Mean (i3 10th Gen) | CPU Utilization |
|---|---|---|---|
| Frame Capture | $< 5\text{ ms}$ | — | 1 Dedicated Thread |
| SCRFD-500M Detection (320×320) | $< 20\text{ ms}$ | **12.5 ms** | shared 2-thread pool |
| Affine 5-Point Alignment | $< 1\text{ ms}$ | **0.94 ms** | Single Thread CPU |
| MobileFaceNet Embedding | $< 20\text{ ms}$ | **19 ms** | shared 2-thread pool |
| MiniFASNet Anti-Spoof | $< 10\text{ ms}$ | **3.4 ms** (one model) / **~8 ms** (two) | shared 2-thread pool |
| Cosine Match (30 vectors) | $< 1\text{ ms}$ | **0.02 ms** | Single Thread CPU |
| **Whole pipeline per frame** | **$< 50\text{ ms}$** | **~43 ms (i3-1005G1, release build)** | **2 threads** |
