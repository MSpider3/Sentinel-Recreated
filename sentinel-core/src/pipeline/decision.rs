//! The grant / deny decision, kept free of cameras and models so every rule
//! can be unit-tested.
//!
//! Each good-quality frame yields one observation: how well the face matched
//! (its tier) and, for matching faces, the anti-spoof "real" score. The tier
//! decides how much proof is needed:
//!
//! | Tier        | Meaning           | Proof needed                                   |
//! |-------------|-------------------|------------------------------------------------|
//! | Golden      | strong match      | one frame with a clean anti-spoof score         |
//! | Standard    | normal match      | `MATCH_FRAMES` matching live frames in a row    |
//! | TwoFactor   | not sure          | none possible — keep looking until the timeout  |
//! | Denied      | no match          | keep looking, unless the face is *clearly* someone |
//! |             |                   | else for `DENY_FRAMES` in a row → DENIED        |
//!
//! "Clearly someone else" is deliberately far from the match thresholds. The
//! enrolled user seen from an unusual angle or in poor light scores just past
//! `two_factor_threshold` (0.50-0.60 measured), while a different person
//! scores around 0.8-1.0. Ending the session early for the first group would
//! refuse the owner in under a second; they get the full session instead.

use crate::config::SecurityConfig;
use crate::pipeline::r#match::AuthTier;

/// Matching, live frames in a row needed when no single frame is conclusive.
pub const MATCH_FRAMES: u32 = 3;
/// Clearly-different frames in a row before the session ends as DENIED.
pub const DENY_FRAMES: u32 = 10;
/// How far past `two_factor_threshold` a distance must be to count as
/// "clearly a different person" rather than "no match yet".
pub const CLEARLY_DIFFERENT_MARGIN: f32 = 0.20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Not enough proof either way yet.
    Continue,
    /// Access granted. `golden` is true only for a single-frame strong match
    /// with a clean anti-spoof score.
    Grant { golden: bool },
    /// The face matches but repeatedly looks like a photo or screen.
    Spoof,
    /// Many frames in a row show a clearly different person.
    Denied,
}

pub struct Decision {
    spoof_threshold: f32,
    spoof_threshold_standard: f32,
    clearly_different_distance: f32,
    max_spoof_strikes: u32,
    match_streak: u32,
    different_streak: u32,
    spoof_strikes: u32,
}

impl Decision {
    pub fn new(config: &SecurityConfig) -> Self {
        Self {
            spoof_threshold: config.spoof_threshold,
            spoof_threshold_standard: config.spoof_threshold_standard,
            clearly_different_distance: config.two_factor_threshold + CLEARLY_DIFFERENT_MARGIN,
            max_spoof_strikes: config.max_retries.max(1),
            match_streak: 0,
            different_streak: 0,
            spoof_strikes: 0,
        }
    }

    /// Feed one good-quality frame: its match tier, the cosine `distance`
    /// behind that tier, and the anti-spoof "real" probability (only looked
    /// at for Golden and Standard frames).
    pub fn observe(&mut self, tier: AuthTier, distance: f32, spoof_score: f32) -> Verdict {
        match tier {
            AuthTier::Golden | AuthTier::Standard => {
                self.different_streak = 0;

                // NaN compares false, so a broken score counts as a strike.
                if !(spoof_score >= self.spoof_threshold_standard) {
                    self.match_streak = 0;
                    self.spoof_strikes += 1;
                    if self.spoof_strikes >= self.max_spoof_strikes {
                        return Verdict::Spoof;
                    }
                    return Verdict::Continue;
                }

                if tier == AuthTier::Golden && spoof_score >= self.spoof_threshold {
                    return Verdict::Grant { golden: true };
                }

                self.match_streak += 1;
                if self.match_streak >= MATCH_FRAMES {
                    return Verdict::Grant { golden: false };
                }
                Verdict::Continue
            }
            AuthTier::TwoFactor => {
                self.match_streak = 0;
                self.different_streak = 0;
                Verdict::Continue
            }
            AuthTier::Denied => {
                self.match_streak = 0;
                // NaN compares false: an unusable distance is "no match yet".
                if distance >= self.clearly_different_distance {
                    self.different_streak += 1;
                    if self.different_streak >= DENY_FRAMES {
                        return Verdict::Denied;
                    }
                } else {
                    self.different_streak = 0;
                }
                Verdict::Continue
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Defaults: golden 0.28, standard 0.42, two_factor 0.50 (so "clearly
    // different" starts at 0.70), spoof_threshold 0.80 (clean),
    // spoof_threshold_standard 0.70, max_retries 3.
    fn decision() -> Decision {
        Decision::new(&SecurityConfig::default())
    }

    // Shorthands: a frame of each kind with a typical distance.
    fn golden(d: &mut Decision, spoof: f32) -> Verdict {
        d.observe(AuthTier::Golden, 0.18, spoof)
    }
    fn standard(d: &mut Decision, spoof: f32) -> Verdict {
        d.observe(AuthTier::Standard, 0.36, spoof)
    }
    fn unsure(d: &mut Decision) -> Verdict {
        d.observe(AuthTier::TwoFactor, 0.46, 0.0)
    }
    /// The owner at an awkward angle: past the match thresholds, but close.
    fn no_match_yet(d: &mut Decision) -> Verdict {
        d.observe(AuthTier::Denied, 0.55, 0.0)
    }
    fn stranger(d: &mut Decision) -> Verdict {
        d.observe(AuthTier::Denied, 0.95, 0.0)
    }

    #[test]
    fn test_clean_golden_frame_grants_at_once() {
        assert_eq!(golden(&mut decision(), 0.95), Verdict::Grant { golden: true });
    }

    #[test]
    fn test_standard_match_needs_three_frames_in_a_row() {
        let mut d = decision();
        assert_eq!(standard(&mut d, 0.9), Verdict::Continue);
        assert_eq!(standard(&mut d, 0.9), Verdict::Continue);
        assert_eq!(standard(&mut d, 0.9), Verdict::Grant { golden: false });
    }

    #[test]
    fn test_golden_with_middling_spoof_score_is_voted_not_instant() {
        let mut d = decision();
        assert_eq!(golden(&mut d, 0.75), Verdict::Continue);
        assert_eq!(golden(&mut d, 0.75), Verdict::Continue);
        assert_eq!(golden(&mut d, 0.75), Verdict::Grant { golden: false });
    }

    #[test]
    fn test_photo_with_borderline_scores_is_refused() {
        // Measured: a photo of the owner held to the webcam matched strongly
        // (distance 0.20) and scored 0.58-0.64 on the anti-spoof model for a
        // whole session. With the earlier 0.55 voting threshold that was
        // granted; it must end as SPOOF.
        let mut d = decision();
        assert_eq!(golden(&mut d, 0.584), Verdict::Continue);
        assert_eq!(golden(&mut d, 0.610), Verdict::Continue);
        assert_eq!(golden(&mut d, 0.637), Verdict::Spoof);
    }

    #[test]
    fn test_any_non_matching_frame_breaks_the_streak() {
        for breaker in [unsure, no_match_yet, stranger] {
            let mut d = decision();
            assert_eq!(standard(&mut d, 0.9), Verdict::Continue);
            assert_eq!(standard(&mut d, 0.9), Verdict::Continue);
            assert_eq!(breaker(&mut d), Verdict::Continue);
            assert_eq!(standard(&mut d, 0.9), Verdict::Continue);
            assert_eq!(standard(&mut d, 0.9), Verdict::Continue);
            assert_eq!(standard(&mut d, 0.9), Verdict::Grant { golden: false });
        }
    }

    #[test]
    fn test_matching_face_that_looks_fake_ends_as_spoof() {
        let mut d = decision();
        assert_eq!(golden(&mut d, 0.10), Verdict::Continue);
        assert_eq!(golden(&mut d, 0.20), Verdict::Continue);
        assert_eq!(golden(&mut d, 0.30), Verdict::Spoof);
    }

    #[test]
    fn test_spoof_strikes_are_not_forgiven_by_good_frames() {
        // An attacker must not be able to keep presenting until one frame scores high.
        let mut d = decision();
        assert_eq!(standard(&mut d, 0.10), Verdict::Continue);
        assert_eq!(standard(&mut d, 0.90), Verdict::Continue);
        assert_eq!(standard(&mut d, 0.10), Verdict::Continue);
        assert_eq!(standard(&mut d, 0.90), Verdict::Continue);
        assert_eq!(standard(&mut d, 0.10), Verdict::Spoof);
    }

    #[test]
    fn test_low_spoof_frame_resets_the_match_streak() {
        let mut d = decision();
        assert_eq!(standard(&mut d, 0.9), Verdict::Continue);
        assert_eq!(standard(&mut d, 0.9), Verdict::Continue);
        assert_eq!(standard(&mut d, 0.1), Verdict::Continue);
        assert_eq!(standard(&mut d, 0.9), Verdict::Continue);
        assert_eq!(standard(&mut d, 0.9), Verdict::Continue);
        assert_eq!(standard(&mut d, 0.9), Verdict::Grant { golden: false });
    }

    #[test]
    fn test_nan_spoof_score_never_grants() {
        let mut d = decision();
        assert_eq!(golden(&mut d, f32::NAN), Verdict::Continue);
        assert_eq!(golden(&mut d, f32::NAN), Verdict::Continue);
        assert_eq!(golden(&mut d, f32::NAN), Verdict::Spoof);
    }

    #[test]
    fn test_owner_at_a_bad_angle_is_never_denied_early() {
        // Measured on the target machine: the enrolled user, turned a little,
        // scored 0.53-0.56 for a whole second. That must keep scanning (the
        // session timeout ends it), not end as DENIED.
        let mut d = decision();
        for _ in 0..500 {
            assert_eq!(no_match_yet(&mut d), Verdict::Continue);
        }
        assert_eq!(d.observe(AuthTier::Denied, f32::NAN, 0.0), Verdict::Continue);
        // ...and once they face the camera they get in.
        assert_eq!(golden(&mut d, 0.95), Verdict::Grant { golden: true });
    }

    #[test]
    fn test_clearly_different_face_is_denied_after_many_frames() {
        let mut d = decision();
        for _ in 0..DENY_FRAMES - 1 {
            assert_eq!(stranger(&mut d), Verdict::Continue);
        }
        assert_eq!(stranger(&mut d), Verdict::Denied);
    }

    #[test]
    fn test_a_closer_frame_resets_the_different_streak() {
        for closer in [unsure, no_match_yet] {
            let mut d = decision();
            for _ in 0..DENY_FRAMES - 1 {
                assert_eq!(stranger(&mut d), Verdict::Continue);
            }
            assert_eq!(closer(&mut d), Verdict::Continue);
            for _ in 0..DENY_FRAMES - 1 {
                assert_eq!(stranger(&mut d), Verdict::Continue);
            }
        }
    }

    #[test]
    fn test_unsure_frames_never_grant_or_deny() {
        let mut d = decision();
        for _ in 0..200 {
            assert_eq!(d.observe(AuthTier::TwoFactor, 0.46, 0.99), Verdict::Continue);
        }
    }
}
