/// authenticator.rs
/// Full port of `SentinelAuthenticator` from biometric_processor.py
/// Exact state machine, tier thresholds, liveness challenges, blacklist checking,
/// adaptive template learning, and audit logging.

use anyhow::Result;
use image::RgbImage;
use std::time::Instant;

use crate::audit::{AuditLogger, AuditRecord};
use crate::config::SentinelConfig;
use crate::gallery::{AdaptiveGallery, BlacklistManager};
use crate::pipeline::{
    align::align_face,
    detect::ScrfdDetector,
    embed::MobileFaceNet,
    liveness::{BlinkDetector, HeadPoseChallenge, HeadPoseDetector},
    r#match::{match_gallery_with_config, AuthTier},
    spoof::SpoofDetector,
};

// ─── Thresholds (matching BiometricConfig in Python prototype) ─────────────────
#[allow(dead_code)]
const GOLDEN_THRESHOLD: f32 = 0.25;
#[allow(dead_code)]
const STANDARD_THRESHOLD: f32 = 0.42;
#[allow(dead_code)]
const TWO_FACTOR_THRESHOLD: f32 = 0.50;

const MAX_RETRIES: u32 = 3;
const CHALLENGE_TIMEOUT_SECS: f64 = 20.0;
const GLOBAL_SESSION_TIMEOUT_SECS: f64 = 120.0;
/// Frames without a face before a session reset (only applies when state != Waiting)
const SESSION_RESET_GRACE_PERIOD: u32 = 30;
/// Max pixel movement between frames before we lose face-lock
const MAX_MOVEMENT_THRESHOLD_SQ: f32 = 200.0 * 200.0;
/// Number of initial frames to skip to allow camera auto-exposure to stabilise.
/// Cold cameras often produce dark or blurry frames for the first few ticks.
const WARMUP_FRAMES: u32 = 5;

// ─── State Machine ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthState {
    Waiting,
    Recognized, // Doing head-pose challenge + blink
    Success,
    Failure,
    Require2FA,
}

/// Tier in use for the current session — mirrors Python `active_tier`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveTier {
    Golden,    // Tier 1: d < 0.25
    Standard,  // Tier 2: 0.25 <= d < 0.42
    TwoFactor, // Tier 3: 0.42 <= d <= 0.50
}

impl ActiveTier {
    fn from_auth_tier(t: &AuthTier) -> Option<Self> {
        match t {
            AuthTier::Golden => Some(Self::Golden),
            AuthTier::Standard => Some(Self::Standard),
            AuthTier::TwoFactor => Some(Self::TwoFactor),
            AuthTier::Denied => None,
        }
    }
}

/// Output of every `process_frame` call.
#[derive(Debug, Clone)]
pub struct AuthResult {
    pub state: AuthState,
    pub message: String,
    /// Active face bounding box [x1, y1, x2, y2], if any.
    pub face_box: Option<[f32; 4]>,
    /// Matched user name (set once recognised).
    pub matched_user: Option<String>,
    /// Cosine distance to best gallery match.
    pub distance: Option<f32>,
    /// Current active tier (set once recognised).
    pub active_tier: Option<ActiveTier>,
    /// Anti-spoof confidence score (if MiniFASNet ran).
    pub spoof_score: Option<f32>,
}

// ─── Liveness Checklist (mirrors LivenessValidator) ───────────────────────────

struct LivenessChecklist {
    spoof_ok: bool,
    challenge_ok: bool,
    blink_ok: bool,
    challenge_type: HeadPoseChallenge,
    challenge_start_time: Instant,
    /// Nose position when challenge first started for delta tracking.
    challenge_start_pos: Option<(f32, f32)>,
}

impl LivenessChecklist {
    fn new(challenge: HeadPoseChallenge) -> Self {
        Self {
            spoof_ok: false,
            challenge_ok: false,
            blink_ok: false,
            challenge_type: challenge,
            challenge_start_time: Instant::now(),
            challenge_start_pos: None,
        }
    }

    fn all_passed(&self) -> bool {
        self.spoof_ok && self.challenge_ok && self.blink_ok
    }

    fn timed_out(&self) -> bool {
        self.challenge_start_time.elapsed().as_secs_f64() > CHALLENGE_TIMEOUT_SECS
    }

    /// Check head-pose challenge via nose landmark movement.
    fn update_motion_challenge(&mut self, face_box: &[f32; 4], nose: (f32, f32)) -> bool {
        if self.challenge_ok {
            return true;
        }
        let w = face_box[2] - face_box[0];
        let motion_threshold = w * 0.08;

        let start = match self.challenge_start_pos {
            Some(p) => p,
            None => {
                self.challenge_start_pos = Some(nose);
                return false;
            }
        };

        let delta_x = nose.0 - start.0;
        let delta_y = nose.1 - start.1;

        println!(
            "[Challenge {:?}] nose=({:.1},{:.1}) start=({:.1},{:.1}) delta=({:.1},{:.1}) thresh={:.1}",
            self.challenge_type, nose.0, nose.1, start.0, start.1, delta_x, delta_y, motion_threshold
        );

        let done = match self.challenge_type {
            HeadPoseChallenge::TurnLeft => delta_x < -motion_threshold,
            HeadPoseChallenge::TurnRight => delta_x > motion_threshold,
            HeadPoseChallenge::TiltUp => delta_y < -motion_threshold,
            HeadPoseChallenge::TiltDown => delta_y > motion_threshold,
        };
        if done {
            self.challenge_ok = true;
        }
        done
    }

    fn challenge_name(&self) -> &'static str {
        match self.challenge_type {
            HeadPoseChallenge::TurnLeft => "Turn LEFT",
            HeadPoseChallenge::TurnRight => "Turn RIGHT",
            HeadPoseChallenge::TiltUp => "Tilt UP",
            HeadPoseChallenge::TiltDown => "Tilt DOWN",
        }
    }
}

// ─── SentinelAuthenticator ─────────────────────────────────────────────────────

/// Full authentication engine — port of Python SentinelAuthenticator.
pub struct SentinelAuthenticator {
    // Models (mandatory)
    pub detector: ScrfdDetector,
    pub embedder: MobileFaceNet,
    pub head_pose: HeadPoseDetector,
    // Optional spoof model
    pub spoof: Option<SpoofDetector>,
    // Gallery: list of L2-normalised 512-d embeddings
    pub gallery: Vec<[f32; 512]>,
    pub target_user: String,
    pub config: SentinelConfig,

    // State machine
    state: AuthState,
    message: String,

    // Face tracking
    locked_face_center: Option<(f32, f32)>,

    // Recognized-phase data
    matched_user: Option<String>,
    last_distance: Option<f32>,
    active_tier: Option<ActiveTier>,
    last_spoof_score: Option<f32>,

    // Liveness
    liveness: Option<LivenessChecklist>,
    blink_detector: BlinkDetector,
    frames_no_face: u32,

    // Retry / timeout
    retry_count: u32,
    session_start: Instant,

    // Random challenge sequence
    challenge_rng_idx: usize,

    // Camera warmup: counts frames elapsed since start/reset.
    // Face detection is skipped until this reaches WARMUP_FRAMES so that
    // auto-exposure has time to stabilise on cold camera start.
    warmup_frames_elapsed: u32,

    // Audit logger
    audit_logger: AuditLogger,
    blacklist_mgr: BlacklistManager,
}

fn random_challenge(rng_idx: usize) -> HeadPoseChallenge {
    match rng_idx % 4 {
        0 => HeadPoseChallenge::TurnLeft,
        1 => HeadPoseChallenge::TurnRight,
        2 => HeadPoseChallenge::TiltUp,
        _ => HeadPoseChallenge::TiltDown,
    }
}

impl SentinelAuthenticator {
    pub fn new(
        detector: ScrfdDetector,
        embedder: MobileFaceNet,
        gallery: Vec<[f32; 512]>,
        target_user: String,
        spoof: Option<SpoofDetector>,
    ) -> Self {
        Self::new_with_config(
            detector,
            embedder,
            gallery,
            target_user,
            spoof,
            SentinelConfig::default(),
        )
    }

    pub fn new_with_config(
        detector: ScrfdDetector,
        embedder: MobileFaceNet,
        gallery: Vec<[f32; 512]>,
        target_user: String,
        spoof: Option<SpoofDetector>,
        config: SentinelConfig,
    ) -> Self {
        let rng_idx = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos() as usize)
            % 4;

        Self {
            detector,
            embedder,
            head_pose: HeadPoseDetector::new(),
            spoof,
            gallery,
            target_user,
            config,
            state: AuthState::Waiting,
            message: "Initialising camera...".to_string(),
            locked_face_center: None,
            matched_user: None,
            last_distance: None,
            active_tier: None,
            last_spoof_score: None,
            liveness: None,
            blink_detector: BlinkDetector::new(),
            frames_no_face: 0,
            retry_count: 0,
            session_start: Instant::now(),
            challenge_rng_idx: rng_idx,
            warmup_frames_elapsed: 0,
            audit_logger: AuditLogger::new(),
            blacklist_mgr: BlacklistManager::new(),
        }
    }

    fn active_tier_num(&self) -> u32 {
        match self.active_tier {
            Some(ActiveTier::Golden) => 1,
            Some(ActiveTier::Standard) => 2,
            Some(ActiveTier::TwoFactor) => 3,
            None => 4,
        }
    }

    fn log_audit(&self, result: &str, tier: u32, liveness_status: &str) {
        let duration_ms = self.session_start.elapsed().as_millis() as u64;
        let user_str = self
            .matched_user
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let record = AuditRecord::new_now(
            user_str,
            result,
            self.last_distance,
            tier,
            liveness_status,
            self.last_spoof_score,
            duration_ms,
        );
        let _ = self.audit_logger.log(&record);
    }

    fn center_of(bbox: &[f32; 4]) -> (f32, f32) {
        ((bbox[0] + bbox[2]) / 2.0, (bbox[1] + bbox[3]) / 2.0)
    }

    fn dist_sq(a: (f32, f32), b: (f32, f32)) -> f32 {
        (a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)
    }

    fn reset(&mut self, full_reset: bool) {
        self.state = AuthState::Waiting;
        self.locked_face_center = None;
        self.liveness = None;
        self.frames_no_face = 0;
        self.blink_detector = BlinkDetector::new();
        // Reset warmup so that if the camera is re-opened or the pipeline is
        // restarted we skip the first few frames again.
        if full_reset {
            self.warmup_frames_elapsed = 0;
            self.matched_user = None;
            self.last_distance = None;
            self.active_tier = None;
        }
    }

    fn make_result(&self, face_box: Option<[f32; 4]>) -> AuthResult {
        AuthResult {
            state: self.state.clone(),
            message: self.message.clone(),
            face_box,
            matched_user: self.matched_user.clone(),
            distance: self.last_distance,
            active_tier: self.active_tier,
            spoof_score: self.last_spoof_score,
        }
    }

    /// Process one camera frame through the full authentication pipeline.
    pub fn process_frame(&mut self, frame: &RgbImage) -> Result<AuthResult> {
        // ── Global timeout ──────────────────────────────────────────────────
        if self.session_start.elapsed().as_secs_f64() > GLOBAL_SESSION_TIMEOUT_SECS {
            self.state = AuthState::Failure;
            self.message = "Session timed out.".to_string();
            self.log_audit("TIMEOUT", 4, "SKIPPED");
            return Ok(self.make_result(None));
        }

        // ── Max retries lockout ─────────────────────────────────────────────
        if self.retry_count >= MAX_RETRIES {
            self.state = AuthState::Failure;
            self.message = "Maximum attempts reached.".to_string();
            self.log_audit("TIMEOUT", 4, "CHALLENGE_TIMEOUT");
            return Ok(self.make_result(None));
        }

        // ── Camera warmup ───────────────────────────────────────────────────
        // Skip the first WARMUP_FRAMES frames so the camera's automatic
        // exposure / white-balance can stabilise. Dark or blurry warmup frames
        // would produce spurious "no face" results; silently dropping them is
        // safer than treating them as detection failures.
        if self.warmup_frames_elapsed < WARMUP_FRAMES {
            self.warmup_frames_elapsed += 1;
            self.message = format!(
                "Initialising camera... ({}/{})",
                self.warmup_frames_elapsed, WARMUP_FRAMES
            );
            return Ok(self.make_result(None));
        }

        // ── Face detection ──────────────────────────────────────────────────
        let detections = self.detector.detect(frame)?;

        let active_face: Option<[f32; 4]> = if detections.is_empty() {
            None
        } else if let Some(locked) = self.locked_face_center {
            let best = detections
                .iter()
                .min_by(|a, b| {
                    let da = Self::dist_sq(Self::center_of(&a.bbox), locked);
                    let db = Self::dist_sq(Self::center_of(&b.bbox), locked);
                    da.partial_cmp(&db).unwrap()
                })
                .unwrap();
            let center = Self::center_of(&best.bbox);
            if Self::dist_sq(center, locked) < MAX_MOVEMENT_THRESHOLD_SQ {
                Some(best.bbox)
            } else {
                None
            }
        } else {
            let largest = detections
                .iter()
                .max_by(|a, b| {
                    let wa = a.bbox[2] - a.bbox[0];
                    let ha = a.bbox[3] - a.bbox[1];
                    let wb = b.bbox[2] - b.bbox[0];
                    let hb = b.bbox[3] - b.bbox[1];
                    (wa * ha).partial_cmp(&(wb * hb)).unwrap()
                })
                .unwrap();
            Some(largest.bbox)
        };

        // ── Face lost handling ───────────────────────────────────────────────
        if active_face.is_none() {
            self.frames_no_face += 1;
            if self.frames_no_face > SESSION_RESET_GRACE_PERIOD && self.state != AuthState::Waiting {
                println!("[Auth] Face lost — resetting session.");
                self.reset(false);
                self.message = "Face lost. Please re-center.".to_string();
            } else {
                self.message = "No face detected. Look at camera.".to_string();
            }
            return Ok(self.make_result(None));
        }

        self.frames_no_face = 0;
        let bbox = active_face.unwrap();
        self.locked_face_center = Some(Self::center_of(&bbox));

        // ── Spoof check ──────────────────────────────────────────────────────
        if let Some(ref mut spoof) = self.spoof {
            if spoof.is_calibrating() {
                if let Ok(crop) = SpoofDetector::square_crop(frame, bbox, 1.5) {
                    spoof.calibrate_tick(&crop);
                    let n = spoof.calib_samples_len();
                    self.message = format!("Calibrating anti-spoof... ({}/80)", n);
                    return Ok(self.make_result(Some(bbox)));
                }
            } else {
                if let Ok(crop) = SpoofDetector::square_crop(frame, bbox, 1.5) {
                    match spoof.predict(&crop) {
                        Ok((is_real, confidence)) => {
                            self.last_spoof_score = Some(confidence);
                            if !is_real {
                                self.retry_count += 1;
                                let remaining = MAX_RETRIES.saturating_sub(self.retry_count);
                                println!(
                                    "[Auth] Spoof detected (conf={:.2}). Retries left: {}",
                                    confidence, remaining
                                );
                                self.reset(true);
                                self.message = format!(
                                    "Spoof detected! Attempts left: {}",
                                    remaining
                                );
                                return Ok(self.make_result(Some(bbox)));
                            }
                            if let Some(ref mut lv) = self.liveness {
                                lv.spoof_ok = true;
                            }
                        }
                        Err(e) => {
                            println!("[Auth] Spoof error: {e} — skipping");
                        }
                    }
                }
            }
        } else {
            if let Some(ref mut lv) = self.liveness {
                lv.spoof_ok = true;
            }
        }

        // ─────────────────────────────────────────────────────────────────────
        // STATE: WAITING — scan, embed, blacklist check, match, classify tier
        // ─────────────────────────────────────────────────────────────────────
        if self.state == AuthState::Waiting {
            self.message = "Scanning face...".to_string();

            let det_with_kps = detections.iter().find(|d| {
                (d.bbox[0] - bbox[0]).abs() < 5.0 && (d.bbox[1] - bbox[1]).abs() < 5.0
            });

            let aligned = if let Some(det) = det_with_kps {
                align_face(frame, &det.landmarks).ok()
            } else {
                None
            };

            if let Some(aligned_img) = aligned {
                match self.embedder.embed(&aligned_img) {
                    Ok(embedding) => {
                        // 1. Blacklist Check (BEFORE tier decision!)
                        if self.blacklist_mgr.check(&embedding) {
                            println!("[Auth] Intrusion matching blacklist (d < 0.25) — access denied immediately.");
                            self.state = AuthState::Failure;
                            self.message = "Access Denied: Restricted Identity".to_string();
                            self.log_audit("DENIED", 4, "SKIPPED");
                            return Ok(self.make_result(Some(bbox)));
                        }

                        if self.gallery.is_empty() {
                            self.state = AuthState::Failure;
                            self.message = "No gallery loaded.".to_string();
                            self.log_audit("DENIED", 4, "SKIPPED");
                            return Ok(self.make_result(Some(bbox)));
                        }

                        // 2. Identification / Tier matching
                        let (dist, tier) = match_gallery_with_config(&embedding, &self.gallery, &self.config.security);
                        self.last_distance = Some(dist);
                        println!("[Auth] Distance={:.4}  Tier={:?}", dist, tier);

                        match ActiveTier::from_auth_tier(&tier) {
                            None => {
                                // Tier 4: Denied
                                println!("[Auth] Unknown face — access denied.");
                                self.state = AuthState::Failure;
                                self.message = "Access Denied.".to_string();

                                // Save intruder screenshot + embedding to blacklist
                                if let Err(e) = self.blacklist_mgr.add_intruder(&embedding, frame) {
                                    println!("[Auth] Warning: Failed to log intruder to blacklist: {e}");
                                } else {
                                    println!("[Auth] Intruder logged to blacklist.");
                                }

                                self.log_audit("DENIED", 4, "SKIPPED");
                                return Ok(self.make_result(Some(bbox)));
                            }
                            Some(active_tier) => {
                                self.active_tier = Some(active_tier);
                                self.matched_user = Some(self.target_user.clone());

                                // Tier 1 (Golden: d < 0.25): Skip head pose and blink challenges, grant access immediately.
                                if active_tier == ActiveTier::Golden {
                                    println!(
                                        "[Auth] GOLDEN match (d={:.4}) — granting access immediately after spoof check.",
                                        dist
                                    );
                                    self.state = AuthState::Success;
                                    self.message = format!(
                                        "Access Granted: {} (Golden Match)",
                                        self.target_user
                                    );

                                    // Trigger Adaptive Gallery learning if eligible
                                    if AdaptiveGallery::should_adapt(
                                        &self.target_user,
                                        AuthTier::Golden,
                                        &self.config,
                                    ) {
                                        if let Err(e) = AdaptiveGallery::add_vector(
                                            &self.target_user,
                                            &embedding,
                                            &self.config,
                                        ) {
                                            println!("[Auth] Warning: Adaptive template save failed: {e}");
                                        } else {
                                            println!("[Auth] Adaptive gallery template saved for {}.", self.target_user);
                                        }
                                    }

                                    self.log_audit("GRANTED", 1, "SKIPPED");
                                    return Ok(self.make_result(Some(bbox)));
                                }

                                // Tier 2 (Standard) & Tier 3 (2FA): Require head pose challenge -> blink detection
                                let challenge = random_challenge(self.challenge_rng_idx);
                                self.challenge_rng_idx = (self.challenge_rng_idx + 1) % 4;
                                println!(
                                    "[Auth] Recognized (Tier {:?}, d={:.4}). Starting challenge: {:?}",
                                    active_tier, dist, challenge
                                );
                                self.state = AuthState::Recognized;
                                let mut lv = LivenessChecklist::new(challenge);
                                if self.spoof.is_none() {
                                    lv.spoof_ok = true;
                                }
                                self.liveness = Some(lv);
                                self.message = format!(
                                    "Hi {}! Please: {}",
                                    self.target_user,
                                    self.liveness.as_ref().unwrap().challenge_name()
                                );
                            }
                        }
                    }
                    Err(e) => {
                        self.message = format!("Embed error: {e}");
                    }
                }
            } else {
                self.message = "Aligning face...".to_string();
            }

            return Ok(self.make_result(Some(bbox)));
        }

        // ─────────────────────────────────────────────────────────────────────
        // STATE: RECOGNIZED — head-pose challenge then blink
        // ─────────────────────────────────────────────────────────────────────
        if self.state == AuthState::Recognized {
            let lv = match self.liveness.as_mut() {
                Some(l) => l,
                None => {
                    self.reset(false);
                    return Ok(self.make_result(Some(bbox)));
                }
            };

            // Challenge timeout check
            if lv.timed_out() {
                self.retry_count += 1;
                let remaining = MAX_RETRIES.saturating_sub(self.retry_count);
                println!("[Auth] Challenge timed out. Retries left: {}", remaining);

                let liveness_status = if lv.challenge_ok {
                    "BLINK_TIMEOUT"
                } else {
                    "CHALLENGE_TIMEOUT"
                };
                self.log_audit("TIMEOUT", self.active_tier_num(), liveness_status);

                self.reset(true);
                self.message = format!("Too slow! Attempts left: {}", remaining);
                return Ok(self.make_result(Some(bbox)));
            }

            let nose: (f32, f32) = detections
                .iter()
                .find(|d| {
                    (d.bbox[0] - bbox[0]).abs() < 15.0 && (d.bbox[1] - bbox[1]).abs() < 15.0
                })
                .map(|d| (d.landmarks[2][0], d.landmarks[2][1]))
                .unwrap_or_else(|| Self::center_of(&bbox));

            if !lv.challenge_ok {
                // Stage 2a: head-pose motion challenge
                let challenge_name = lv.challenge_name();
                if lv.update_motion_challenge(&bbox, nose) {
                    println!("[Auth] Head pose challenge passed!");
                    self.message = "Good! Now please blink.".to_string();
                } else {
                    self.message = format!(
                        "Hi {}! {}",
                        self.matched_user.as_deref().unwrap_or("?"),
                        challenge_name
                    );
                }
            } else {
                // Stage 2b: blink detection (only after challenge)
                self.message = "Please blink now...".to_string();

                let ear_val = self.compute_ear_from_detection(frame, &bbox, &detections);
                if let Some(ear) = ear_val {
                    let blinked = self.blink_detector.update(ear);
                    if blinked {
                        println!("[Auth] Blink detected!");
                        if let Some(ref mut lv2) = self.liveness {
                            lv2.blink_ok = true;
                        }
                    }
                }
            }

            // Check if all checks passed
            let all_passed = self.liveness.as_ref().map(|l| l.all_passed()).unwrap_or(false);
            if all_passed {
                match self.active_tier {
                    Some(ActiveTier::TwoFactor) => {
                        self.state = AuthState::Require2FA;
                        self.message = format!(
                            "2FA Required: {}",
                            self.matched_user.as_deref().unwrap_or("?")
                        );
                        println!("[Auth] Biometrics passed — 2FA required.");
                        self.log_audit("REQUIRE_2FA", 3, "BLINK_PASSED");
                    }
                    _ => {
                        self.state = AuthState::Success;
                        self.message = format!(
                            "Access Granted: {}",
                            self.matched_user.as_deref().unwrap_or("?")
                        );
                        println!("[Auth] Access GRANTED.");
                        let tier_num = self.active_tier_num();
                        self.log_audit("GRANTED", tier_num, "BLINK_PASSED");
                    }
                }
            }
        }

        Ok(self.make_result(Some(bbox)))
    }

    fn compute_ear_from_detection(
        &self,
        _frame: &RgbImage,
        bbox: &[f32; 4],
        detections: &[crate::pipeline::detect::FaceDetection],
    ) -> Option<f32> {
        let det = detections.iter().find(|d| {
            (d.bbox[0] - bbox[0]).abs() < 10.0 && (d.bbox[1] - bbox[1]).abs() < 10.0
        })?;

        let kps = det.landmarks;
        let left_eye = kps[0];
        let right_eye = kps[1];
        let nose = kps[2];
        let l_mouth = kps[3];
        let r_mouth = kps[4];

        let _eye_cx = (left_eye[0] + right_eye[0]) / 2.0;
        let eye_cy = (left_eye[1] + right_eye[1]) / 2.0;
        let eye_dist =
            ((left_eye[0] - right_eye[0]).powi(2) + (left_eye[1] - right_eye[1]).powi(2)).sqrt();

        let mouth_cy = (l_mouth[1] + r_mouth[1]) / 2.0;
        let face_h = mouth_cy - eye_cy;

        if eye_dist < 1e-3 || face_h < 1e-3 {
            return None;
        }

        let eye_to_nose_y = (nose[1] - eye_cy).abs();
        let ear_proxy = eye_to_nose_y / (face_h + 1.0);

        let ear = (ear_proxy * 0.5).clamp(0.0, 1.0);
        Some(ear)
    }
}
