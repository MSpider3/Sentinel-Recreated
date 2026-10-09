/// authenticator.rs
/// One authentication session: takes camera frames one at a time and decides
/// between Success, Failure, Spoof, NoFace and Timeout.
///
/// Per frame: warm-up → detect → quality gate → align → embed → match →
/// anti-spoof → `Decision` (see decision.rs for the grant rules).

use anyhow::{Context, Result};
use std::time::Instant;

use crate::audit::{AuditLogger, AuditRecord};
use crate::config::SentinelConfig;
use crate::gallery::{AdaptiveGallery, IntrusionLog};
use crate::pipeline::{
    align::align_face,
    capture::CapturedFrame,
    decision::{Decision, Verdict},
    models::Models,
    quality,
    r#match::{decide_tier_with_config, match_gallery_with_config, AuthTier},
};

/// Seconds without any face in view before the session ends as NoFace.
const NO_FACE_TIMEOUT_SECS: f64 = 3.0;

// ─── Camera warm-up ────────────────────────────────────────────────────────────
// A camera that has just been switched on needs a moment for auto-exposure to
// settle; its first frames are dark or washed out. Frames are skipped until
// the overall brightness stops changing.

/// Frames darker than this (mean luma, 0..255) are never considered settled.
const MIN_LUMA: f64 = 15.0;
/// Largest frame-to-frame change in mean luma that still counts as settled.
const LUMA_SETTLED_DELTA: f64 = 3.0;
/// Settled frames in a row required before recognition starts.
const LUMA_SETTLED_FRAMES: u32 = 2;
/// Recognition starts after this long even if brightness never settles; the
/// quality gate then decides frame by frame.
const MAX_WARMUP_SECS: f64 = 1.0;

struct Warmup {
    prev_luma: Option<f64>,
    settled_frames: u32,
    started: Option<Instant>,
    done: bool,
}

impl Warmup {
    fn new() -> Self {
        Self { prev_luma: None, settled_frames: 0, started: None, done: false }
    }

    /// Feed the mean luma of each new frame; true once frames may be used.
    fn ready(&mut self, luma: f64) -> bool {
        if self.done {
            return true;
        }
        let started = *self.started.get_or_insert_with(Instant::now);

        let settled = luma >= MIN_LUMA
            && self.prev_luma.map_or(false, |prev| (luma - prev).abs() <= LUMA_SETTLED_DELTA);
        self.settled_frames = if settled { self.settled_frames + 1 } else { 0 };
        self.prev_luma = Some(luma);

        self.done = self.settled_frames >= LUMA_SETTLED_FRAMES
            || started.elapsed().as_secs_f64() > MAX_WARMUP_SECS;
        if self.done {
            log::debug!(
                "[Auth] Camera warm-up done {} ms after its first frame (luma {:.0}, settled: {})",
                started.elapsed().as_millis(),
                luma,
                self.settled_frames >= LUMA_SETTLED_FRAMES
            );
        }
        self.done
    }
}

// ─── State Machine ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthState {
    /// Still looking; feed more frames.
    Waiting,
    /// Access granted.
    Success,
    /// Several frames in a row showed a clearly different person.
    Failure,
    /// The face matched but repeatedly looked like a photo or screen.
    Spoof,
    /// Nobody in front of the camera.
    NoFace,
    /// Time ran out without enough proof either way.
    Timeout,
}

/// Output of every `process_frame` call.
#[derive(Debug, Clone)]
pub struct AuthResult {
    pub state: AuthState,
    pub message: String,
    /// Active face bounding box [x1, y1, x2, y2], if any.
    pub face_box: Option<[f32; 4]>,
    /// Matched user name (set once access is granted).
    pub matched_user: Option<String>,
    /// Cosine distance to best gallery match on the latest evaluated frame.
    pub distance: Option<f32>,
    /// Match tier of the latest evaluated frame.
    pub active_tier: Option<AuthTier>,
    /// Anti-spoof "real" score of the latest matching frame.
    pub spoof_score: Option<f32>,
    /// True once at least one good-quality face was compared to the gallery.
    pub face_evaluated: bool,
}

// ─── SentinelAuthenticator ─────────────────────────────────────────────────────

pub struct SentinelAuthenticator {
    /// Enrolled templates (L2-normalised 512-d). Never modified here.
    pub core_gallery: Vec<[f32; 512]>,
    /// Templates learned after enrollment.
    pub adaptive_gallery: Vec<[f32; 512]>,
    pub target_user: String,
    pub config: SentinelConfig,

    state: AuthState,
    message: String,
    decision: Decision,
    warmup: Warmup,
    session_start: Instant,
    no_face_start: Option<Instant>,

    last_distance: Option<f32>,
    last_tier: Option<AuthTier>,
    last_spoof_score: Option<f32>,
    face_evaluated: bool,

    audit_logger: AuditLogger,
    intrusion_log: IntrusionLog,
}

impl SentinelAuthenticator {
    pub fn new(
        core_gallery: Vec<[f32; 512]>,
        adaptive_gallery: Vec<[f32; 512]>,
        target_user: String,
        config: SentinelConfig,
    ) -> Self {
        Self {
            core_gallery,
            adaptive_gallery,
            target_user,
            decision: Decision::new(&config.security),
            config,
            state: AuthState::Waiting,
            message: "Initialising camera...".to_string(),
            warmup: Warmup::new(),
            session_start: Instant::now(),
            no_face_start: None,
            last_distance: None,
            last_tier: None,
            last_spoof_score: None,
            face_evaluated: false,
            audit_logger: AuditLogger::new(),
            intrusion_log: IntrusionLog::new(),
        }
    }

    fn make_result(&self, face_box: Option<[f32; 4]>) -> AuthResult {
        AuthResult {
            state: self.state.clone(),
            message: self.message.clone(),
            face_box,
            matched_user: (self.state == AuthState::Success).then(|| self.target_user.clone()),
            distance: self.last_distance,
            active_tier: self.last_tier,
            spoof_score: self.last_spoof_score,
            face_evaluated: self.face_evaluated,
        }
    }

    /// End the session in `state`, write the audit record and return the result.
    fn finish(&mut self, state: AuthState, message: &str, face_box: Option<[f32; 4]>) -> AuthResult {
        let result = match state {
            AuthState::Success => "GRANTED",
            AuthState::Spoof => "SPOOF",
            AuthState::Timeout => "TIMEOUT",
            _ => "DENIED",
        };
        let tier = match self.last_tier {
            Some(AuthTier::Golden) => 1,
            Some(AuthTier::Standard) => 2,
            Some(AuthTier::TwoFactor) => 3,
            Some(AuthTier::Denied) | None => 4,
        };
        let user = if state == AuthState::Success { self.target_user.as_str() } else { "unknown" };
        let liveness = if self.last_spoof_score.is_some() { "PASSIVE" } else { "SKIPPED" };
        let record = AuditRecord::new_now(
            user,
            result,
            self.last_distance,
            tier,
            liveness,
            self.last_spoof_score,
            self.session_start.elapsed().as_millis() as u64,
        );
        let _ = self.audit_logger.log(&record);

        self.state = state;
        self.message = message.to_string();
        self.make_result(face_box)
    }

    /// End a still-running session because time ran out (for callers that
    /// are waiting on a camera which has stopped delivering frames).
    pub fn expire(&mut self) -> AuthResult {
        if self.state != AuthState::Waiting {
            return self.make_result(None);
        }
        self.finish(AuthState::Timeout, "Session timed out.", None)
    }

    /// `expire()` the session if its time is up. Returns true when it did.
    pub fn expire_if_overdue(&mut self) -> bool {
        let overdue = self.state == AuthState::Waiting
            && self.session_start.elapsed().as_secs_f64() > self.config.security.global_session_timeout;
        if overdue {
            self.expire();
        }
        overdue
    }

    /// Process one new camera frame. An `Err` means this frame could not be
    /// used; it never grants access and the caller simply feeds the next one.
    pub fn process_frame(&mut self, models: &mut Models, captured: &CapturedFrame) -> Result<AuthResult> {
        if self.state != AuthState::Waiting {
            return Ok(self.make_result(None));
        }

        if self.session_start.elapsed().as_secs_f64() > self.config.security.global_session_timeout {
            return Ok(self.finish(AuthState::Timeout, "Session timed out.", None));
        }

        if !self.warmup.ready(captured.luma) {
            self.message = "Initialising camera...".to_string();
            return Ok(self.make_result(None));
        }

        // ── Face detection: the largest face is the one being authenticated ──
        let frame = &captured.image;
        let detections = models.detector.detect(frame)?;
        let area = |b: &[f32; 4]| (b[2] - b[0]) * (b[3] - b[1]);
        let face = match detections.iter().max_by(|a, b| area(&a.bbox).total_cmp(&area(&b.bbox))) {
            Some(face) => face,
            None => {
                let since = *self.no_face_start.get_or_insert_with(Instant::now);
                if since.elapsed().as_secs_f64() > NO_FACE_TIMEOUT_SECS {
                    self.state = AuthState::NoFace;
                    self.message = "No face detected.".to_string();
                } else {
                    self.message = "No face detected. Look at camera.".to_string();
                }
                return Ok(self.make_result(None));
            }
        };
        self.no_face_start = None;
        let bbox = face.bbox;

        // ── Quality gate: a poor frame is skipped, never counted as a mismatch ──
        if let Err(issue) = quality::check_pose(&face.landmarks, frame.width(), frame.height()) {
            self.message = issue.hint().to_string();
            return Ok(self.make_result(Some(bbox)));
        }
        let aligned = align_face(frame, &face.landmarks)?;
        if let Err(issue) = quality::check_aligned_face(&aligned) {
            self.message = issue.hint().to_string();
            return Ok(self.make_result(Some(bbox)));
        }

        // ── Match ────────────────────────────────────────────────────────────
        let embedding = models.embedder.embed(&aligned)?;
        let security = &self.config.security;
        let (core_distance, _) = match_gallery_with_config(&embedding, &self.core_gallery, security);
        let (adaptive_distance, _) = match_gallery_with_config(&embedding, &self.adaptive_gallery, security);
        let distance = core_distance.min(adaptive_distance);
        let tier = decide_tier_with_config(distance, security);
        self.last_distance = Some(distance);
        self.last_tier = Some(tier);
        self.face_evaluated = true;

        // ── Anti-spoof, on the same frame that matched ───────────────────────
        // Not run for non-matching faces: they cannot be granted anyway.
        let spoof_score = match tier {
            AuthTier::Golden | AuthTier::Standard => {
                let spoof = models.spoof.as_mut().context("Anti-spoof model unavailable")?;
                let score = spoof.predict(frame, bbox)?;
                self.last_spoof_score = Some(score);
                score
            }
            AuthTier::TwoFactor | AuthTier::Denied => 0.0,
        };
        log::debug!("[Auth] distance={:.4} tier={:?} spoof={:.3}", distance, tier, spoof_score);

        // ── Decision ─────────────────────────────────────────────────────────
        match self.decision.observe(tier, distance, spoof_score) {
            Verdict::Continue => {
                self.message = match tier {
                    AuthTier::Golden | AuthTier::Standard => "Verifying...",
                    AuthTier::TwoFactor | AuthTier::Denied => "Not recognised yet. Look straight at the camera.",
                }
                .to_string();
                Ok(self.make_result(Some(bbox)))
            }
            Verdict::Grant { golden } => {
                if golden
                    && AdaptiveGallery::should_adapt(
                        &self.target_user,
                        core_distance,
                        distance,
                        spoof_score,
                        &self.config,
                    )
                {
                    match AdaptiveGallery::add_vector(&self.target_user, &embedding, &self.config) {
                        Ok(()) => log::info!("[Auth] Learned a new template for {}.", self.target_user),
                        Err(e) => log::warn!("[Auth] Could not save learned template: {e}"),
                    }
                }
                let message = format!("Access Granted: {}", self.target_user);
                Ok(self.finish(AuthState::Success, &message, Some(bbox)))
            }
            Verdict::Spoof => Ok(self.finish(AuthState::Spoof, "Spoof detected.", Some(bbox))),
            Verdict::Denied => {
                if let Err(e) = self.intrusion_log.save(frame) {
                    log::warn!("[Auth] Could not save intrusion photo: {e}");
                }
                Ok(self.finish(AuthState::Failure, "Access Denied.", Some(bbox)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_warmup_waits_for_brightness_to_settle() {
        let mut w = Warmup::new();
        // Auto-exposure ramping up: brightness still changing.
        assert!(!w.ready(5.0));
        assert!(!w.ready(40.0));
        assert!(!w.ready(80.0));
        // Two frames in a row within the settled delta.
        assert!(!w.ready(81.0));
        assert!(w.ready(82.0));
        // Once ready it stays ready, whatever the brightness does.
        assert!(w.ready(10.0));
    }

    #[test]
    fn test_warmup_ignores_a_stable_but_black_picture() {
        let mut w = Warmup::new();
        assert!(!w.ready(3.0));
        assert!(!w.ready(3.0));
        assert!(!w.ready(3.0));
    }

    #[test]
    fn test_warmup_gives_up_waiting_after_a_while() {
        let mut w = Warmup::new();
        w.started = Some(Instant::now() - std::time::Duration::from_secs(2));
        assert!(w.ready(3.0));
    }
}
