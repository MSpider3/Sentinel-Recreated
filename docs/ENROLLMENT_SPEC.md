# Enrollment Specification & Adaptive Gallery Policy — Sentinel Recreated

**Document**: `docs/ENROLLMENT_SPEC.md`  
**Subsystem**: `sentinel-py/enroll.py` & `sentinel-core/src/gallery/adaptive.rs`

---

## 1. Enrollment Subsystem Division & Camera Ownership

The face enrollment process is an interactive wizard:
- **Python Client (`sentinel_py/enroll.py`) — Camera Owner & UI**: opens the camera named in the daemon's config (`camera.source`), shows the live preview, guides the user through the poses and sends frames to the daemon with `SubmitEnrollmentFrameData`.
- **Rust Daemon (`sentinel-core`) — Judge & Store**: checks each frame and, only when asked to capture, turns it into a template. It never opens the camera during enrollment.

Two kinds of frame submission:
- **Preview** (`capture = false`, about 10 per second): the daemon reports whether the frame *would* be accepted. Nothing is stored.
- **Capture** (`capture = true`, when the user presses SPACE): an accepted frame becomes one template.

---

## 2. Mandatory Pose Sequences

### Base Pose Sequence (Default — 15 Embeddings)
1. **Center**: Face directly looking at camera lens. (3 sub-samples)
2. **Left**: Yaw head turn approximately $15^\circ$ to the left. (3 sub-samples)
3. **Right**: Yaw head turn approximately $15^\circ$ to the right. (3 sub-samples)
4. **Up**: Pitch head tilt approximately $10^\circ$ upwards. (3 sub-samples)
5. **Down**: Pitch head tilt approximately $10^\circ$ downwards. (3 sub-samples)

### Glasses Wearer Variant (30 Embeddings)
`sentinel enroll` asks *"Do you wear glasses, even only sometimes?"* before the camera opens (`--glasses` / `--no-glasses` answer it in advance). If the answer is yes:
1. Complete Base 5-Pose Sequence **WITH glasses** $\rightarrow 15\text{ embeddings}$.
2. Interactive Pause Prompt: *"Please remove your glasses and press ENTER."*
3. Complete Base 5-Pose Sequence **WITHOUT glasses** $\rightarrow 15\text{ embeddings}$.
4. Total Core Gallery Size: $30\text{ embeddings}$.

---

## 3. Frame Validation

Each submitted frame must pass, in order:
1. **Single Identity Gate**: exactly one face (`MULTIPLE_FACES` / `NO_FACE` otherwise), at least `min_face_size_px` wide and tall.
2. **Quality Gate** — the same one authentication uses: landmarks inside the frame (`OUT_OF_FRAME`), roughly frontal (`NOT_FRONTAL`), not too dark or bright (`TOO_DARK` / `TOO_BRIGHT`), not blurred (`BLURRY`). The pose prompts therefore mean a *slight* turn; a strong turn is refused.
3. **On capture only**:
   - the template is the sum of the embeddings of the face and its mirror image, normalised;
   - a capture within cosine distance 0.01 of one already collected is the same picture again (`TOO_SIMILAR`);
   - at most 40 templates per enrollment (`FULL`); at least 15 are needed to finish.

`FinishEnrollment` writes `gallery.npy` and **deletes the user's learned templates** (`adaptive.npy`, `meta.json`): a new enrollment replaces the identity.

*Anti-spoofing is not run during enrollment; starting an enrollment requires PolicyKit administrator authentication.*

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
1. **Strong match to an enrolled template**: distance to the nearest *enrolled* template $<$ `golden_threshold`, on a frame that was granted on its own. Matching only an earlier learned template is not enough, so learned templates cannot drift away from the enrolled face step by step.
2. **Clean anti-spoof score**: at least `spoof_threshold`.
3. **Something new**: at least 0.10 away from every stored template (near-duplicates are not kept).
4. **Daily Rate Limit**: Maximum **1 adaptation vector saved per calendar day** per user (`adaptation_limit_per_day`).
5. **Capacity Cap**: at most `gallery_max_size` (20) vectors. FIFO eviction drops the oldest learned vector. Enrolled templates are never evicted or changed.
