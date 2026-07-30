# Enrollment Specification & Adaptive Gallery Policy — Sentinel Recreated

**Document**: `docs/ENROLLMENT_SPEC.md`  
**Subsystem**: `sentinel-py/enroll.py` & `sentinel-core/src/gallery/adaptive.rs`

---

## 1. Enrollment Subsystem Division & Camera Ownership

The face enrollment process is an interactive wizard. Responsibility and hardware ownership are strictly partitioned:
- **Rust Daemon (`sentinel-core`) — Camera Owner**: The daemon strictly owns the camera hardware device (`/dev/video*`). When `SubmitEnrollmentFrame(session_id)` is invoked via DBus, the daemon captures high-quality frames directly from the hardware pipeline, validates them against quality gates, executes SCRFD detection and 5-point alignment, extracts 512-d embeddings, and writes binary numpy array data (`gallery.npy`). This design (Option A) eliminates transmitting raw frame byte buffers over DBus and guarantees security by preventing client frame tampering.
- **Python Client (`sentinel-py/enroll.py`) — Session Orchestrator**: Renders the UI preview window, overlays status messages, guides the user through pose prompt state machine transitions, and triggers DBus control calls (`StartEnrollment`, `SubmitEnrollmentFrame`, `FinishEnrollment`).

---

## 2. Mandatory Pose Sequences

### Base Pose Sequence (Default — 15 Embeddings)
1. **Center**: Face directly looking at camera lens. (3 sub-samples)
2. **Left**: Yaw head turn approximately $15^\circ$ to the left. (3 sub-samples)
3. **Right**: Yaw head turn approximately $15^\circ$ to the right. (3 sub-samples)
4. **Up**: Pitch head tilt approximately $10^\circ$ upwards. (3 sub-samples)
5. **Down**: Pitch head tilt approximately $10^\circ$ downwards. (3 sub-samples)

### Glasses Wearer Variant (30 Embeddings)
If the user indicates they wear glasses during enrollment setup:
1. Complete Base 5-Pose Sequence **WITH glasses** $\rightarrow 15\text{ embeddings}$.
2. Interactive Pause Prompt: *"Please remove your glasses and press ENTER."*
3. Complete Base 5-Pose Sequence **WITHOUT glasses** $\rightarrow 15\text{ embeddings}$.
4. Total Core Gallery Size: $30\text{ embeddings}$.

---

## 3. Daemon Camera Sampling & Quality Gates (Enrollment Frame Validation)

When `SubmitEnrollmentFrame(session_id)` is invoked via DBus, the daemon directly samples the current camera frame and evaluates:
1. **Detection Gate**: Face MUST be detected by SCRFD with confidence $\ge 0.60$.
2. **Dimension Gate**: Face bounding box height and width MUST be $\ge 25\%$ of the shorter frame dimension ($120\text{ px}$ for $640\times 480$).
3. **Single Identity Gate**: Frame MUST contain **exactly one face**. Multi-face frames return `MULTIPLE_FACES`.
4. **Landmark Stability Gate**: 5 facial landmarks must be extracted and successfully fit the affine transformation matrix without mathematical singularity.
5. *Note: Anti-spoofing and active liveness checks are disabled during enrollment, as physical user session initiation is verified by PolicyKit administrative password escalation.*

---

## 4. Client UI State Machine (`enroll.py`)

```
   ┌──────────────┐
   │   INSTRUCT   │ Instruct user on pose (3s delay)
   └──────┬───────┘
          │
          ▼
   ┌──────────────┐
   │  DETECTING   │ Live camera preview, wait for quality gates to pass
   └──────┬───────┘
          │ Quality Gates Passed
          ▼
   ┌──────────────┐
   │  CAPTURING   │ Freeze frame, send to daemon, aggregate sub-samples
   └──────┬───────┘
          │ 3 Sub-samples Captured
          ▼
   ┌──────────────┐
   │   SUCCESS    │ Green feedback overlay (2s delay)
   └──────┬───────┘
          │ More Poses Remaining?
          ├── Yes ──► Transition to INSTRUCT for next pose index
          └── No  ──► Call FinishEnrollment() ──► DONE
```

---

## 5. Adaptive Gallery Policy (Post-Enrollment Learning)

To adapt seamlessly to gradual biological changes (aging, facial hair, lighting variations), the daemon maintains an adaptive FIFO gallery (`adaptive.npy`).

### Update Eligibility Criteria:
1. **Tier 1 (Golden Zone) Only**: Cosine distance $d < 0.25$.
2. **Probabilistic Roll ($p \approx 0.09$)**: 1 in 11 Tier 1 authentication sessions triggers adaptive save. Prevents rapid overfitting.
3. **Daily Rate Limit**: Maximum **1 adaptation vector saved per calendar day** per user.
4. **Capacity Cap**: Maximum 20 vectors ($20 \times 512$). FIFO eviction drops the oldest adaptive vector when capacity is reached.
5. **Clean Vector Generation**: The adaptive embedding is computed from a fresh aligned crop of the current authentication frame rather than reusing temporary pipeline arrays.
