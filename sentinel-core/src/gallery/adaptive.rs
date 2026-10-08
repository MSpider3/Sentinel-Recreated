use anyhow::{Context, Result};
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::{SecurityConfig, SentinelConfig};
use crate::gallery::store::GalleryStore;

/// A new template closer than this to one already stored adds nothing.
const MIN_NOVELTY: f32 = 0.10;

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
pub struct MetaJson {
    pub last_adaptation_date: String,
    pub today_count: u32,
    pub total_count: usize,
}

pub struct AdaptiveGallery;

impl AdaptiveGallery {
    pub fn meta_path(username: &str) -> PathBuf {
        PathBuf::from(format!("/var/lib/sentinel/users/{}/meta.json", username))
    }

    pub fn load_meta(username: &str) -> MetaJson {
        let p = Self::meta_path(username);
        Self::load_meta_from_path(&p)
    }

    pub fn load_meta_from_path(p: &Path) -> MetaJson {
        if p.exists() {
            if let Ok(content) = fs::read_to_string(p) {
                if let Ok(meta) = serde_json::from_str(&content) {
                    return meta;
                }
            }
        }
        MetaJson::default()
    }

    pub fn save_meta(username: &str, meta: &MetaJson) -> Result<()> {
        let p = Self::meta_path(username);
        Self::save_meta_to_path(&p, meta)
    }

    pub fn save_meta_to_path(p: &Path, meta: &MetaJson) -> Result<()> {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create meta dir: {}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(meta)?;
        crate::fsutil::write_atomic(p, json.as_bytes(), 0o600)
            .with_context(|| format!("Failed to write meta JSON: {}", p.display()))
    }

    pub fn load(username: &str) -> Result<Vec<[f32; 512]>> {
        let store = GalleryStore::new(username);
        store.load_adaptive()
    }

    pub fn save(username: &str, embeddings: &[[f32; 512]]) -> Result<()> {
        let store = GalleryStore::new(username);
        store.save_adaptive(embeddings)
    }

    /// Whether a just-granted face is worth keeping as a learned template.
    ///
    /// * `core_distance` — distance to the nearest *enrolled* template. The
    ///   face must strongly match what the user enrolled; a match against an
    ///   earlier learned template does not count, so learned templates can
    ///   never drift step by step away from the enrolled face.
    /// * `nearest_distance` — distance to the nearest stored template of any
    ///   kind; near-duplicates are not stored.
    /// * `spoof_score` — must be a clean "real" score.
    pub fn is_worth_learning(
        core_distance: f32,
        nearest_distance: f32,
        spoof_score: f32,
        security: &SecurityConfig,
    ) -> bool {
        core_distance < security.golden_threshold
            && nearest_distance >= MIN_NOVELTY
            && spoof_score >= security.spoof_threshold
    }

    pub fn should_adapt(
        username: &str,
        core_distance: f32,
        nearest_distance: f32,
        spoof_score: f32,
        config: &SentinelConfig,
    ) -> bool {
        // A limit of 0 switches learning off.
        if config.adaptive_policy.adaptation_limit_per_day == 0
            || !Self::is_worth_learning(core_distance, nearest_distance, spoof_score, &config.security)
        {
            return false;
        }

        // Daily rate limit check from meta.json
        let today = Local::now().format("%Y-%m-%d").to_string();
        let meta = Self::load_meta(username);
        if meta.last_adaptation_date == today
            && meta.today_count >= config.adaptive_policy.adaptation_limit_per_day
        {
            return false;
        }

        true
    }

    pub fn add_vector(username: &str, embedding: &[f32; 512], config: &SentinelConfig) -> Result<()> {
        let store = GalleryStore::new(username);
        let mut current = store.load_adaptive().unwrap_or_default();
        current.push(*embedding);

        let cap = config.security.gallery_max_size as usize; // default 20
        let bounded = if current.len() > cap {
            current[current.len() - cap..].to_vec()
        } else {
            current
        };

        store.save_adaptive(&bounded)?;

        // Update meta.json
        let today = Local::now().format("%Y-%m-%d").to_string();
        let mut meta = Self::load_meta(username);
        if meta.last_adaptation_date == today {
            meta.today_count += 1;
        } else {
            meta.last_adaptation_date = today;
            meta.today_count = 1;
        }
        meta.total_count = bounded.len();
        Self::save_meta(username, &meta)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_learning_rules() {
        let sec = SecurityConfig::default(); // golden 0.28, clean spoof 0.80
        // Strong match to an enrolled template, a bit different, clearly live.
        assert!(AdaptiveGallery::is_worth_learning(0.18, 0.15, 0.95, &sec));
        // Only matches an earlier learned template, not the enrolled face.
        assert!(!AdaptiveGallery::is_worth_learning(0.35, 0.15, 0.95, &sec));
        // Near-duplicate of something already stored.
        assert!(!AdaptiveGallery::is_worth_learning(0.18, 0.03, 0.95, &sec));
        // Anti-spoof score not clean.
        assert!(!AdaptiveGallery::is_worth_learning(0.18, 0.15, 0.60, &sec));
        assert!(!AdaptiveGallery::is_worth_learning(0.18, 0.15, f32::NAN, &sec));
    }

    #[test]
    fn test_meta_json_serialization() {
        let meta = MetaJson {
            last_adaptation_date: "2026-07-25".to_string(),
            today_count: 1,
            total_count: 5,
        };
        let tmp_dir = std::env::temp_dir().join("sentinel_test_adaptive");
        let meta_file = tmp_dir.join("meta.json");

        AdaptiveGallery::save_meta_to_path(&meta_file, &meta).unwrap();
        let loaded = AdaptiveGallery::load_meta_from_path(&meta_file);
        assert_eq!(loaded, meta);

        let _ = fs::remove_dir_all(&tmp_dir);
    }
}
