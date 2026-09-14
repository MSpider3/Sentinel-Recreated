#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthTier {
    Golden,    // d < 0.25
    Standard,  // 0.25 <= d < 0.42
    TwoFactor, // 0.42 <= d <= 0.50
    Denied,    // d > 0.50
}

pub fn cosine_distance(a: &[f32; 512], b: &[f32; 512]) -> f32 {
    let mut dot = 0.0f32;
    for i in 0..512 {
        dot += a[i] * b[i];
    }
    // Clamped cosine similarity to [-1.0, 1.0]
    let sim = dot.clamp(-1.0, 1.0);
    1.0 - sim
}

use crate::config::SecurityConfig;

pub fn decide_tier(distance: f32) -> AuthTier {
    decide_tier_with_config(distance, &SecurityConfig::default())
}

pub fn decide_tier_with_config(distance: f32, config: &SecurityConfig) -> AuthTier {
    if distance < config.golden_threshold {
        AuthTier::Golden
    } else if distance < config.standard_threshold {
        AuthTier::Standard
    } else if distance <= config.two_factor_threshold {
        AuthTier::TwoFactor
    } else {
        AuthTier::Denied
    }
}

pub fn match_gallery(query: &[f32; 512], gallery: &[[f32; 512]]) -> (f32, AuthTier) {
    match_gallery_with_config(query, gallery, &SecurityConfig::default())
}

pub fn match_gallery_with_config(
    query: &[f32; 512],
    gallery: &[[f32; 512]],
    config: &SecurityConfig,
) -> (f32, AuthTier) {
    if gallery.is_empty() {
        return (2.0, AuthTier::Denied);
    }
    let mut min_dist = 2.0f32;
    for vec in gallery {
        let dist = cosine_distance(query, vec);
        if dist < min_dist {
            min_dist = dist;
        }
    }
    let tier = decide_tier_with_config(min_dist, config);
    (min_dist, tier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identical_embeddings_distance_zero() {
        let mut v = [0.0f32; 512];
        v[0] = 1.0;
        let d = cosine_distance(&v, &v);
        assert!((d - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_tier_boundaries() {
        assert_eq!(decide_tier(0.24), AuthTier::Golden);
        assert_eq!(decide_tier(0.25), AuthTier::Standard);
        assert_eq!(decide_tier(0.41), AuthTier::Standard);
        assert_eq!(decide_tier(0.42), AuthTier::TwoFactor);
        assert_eq!(decide_tier(0.50), AuthTier::TwoFactor);
        assert_eq!(decide_tier(0.51), AuthTier::Denied);
    }
}
