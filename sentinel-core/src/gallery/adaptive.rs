use anyhow::{Context, Result};
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::config::SentinelConfig;
use crate::gallery::store::GalleryStore;
use crate::pipeline::r#match::AuthTier;

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
        fs::write(p, json)
            .with_context(|| format!("Failed to write meta JSON: {}", p.display()))?;

        #[cfg(unix)]
        {
            if let Ok(m) = fs::metadata(p) {
                let mut perms = m.permissions();
                perms.set_mode(0o600);
                fs::set_permissions(p, perms).ok();
            }
        }
        Ok(())
    }

    pub fn load(username: &str) -> Result<Vec<[f32; 512]>> {
        let store = GalleryStore::new(username);
        store.load_adaptive()
    }

    pub fn save(username: &str, embeddings: &[[f32; 512]]) -> Result<()> {
        let store = GalleryStore::new(username);
        store.save_adaptive(embeddings)
    }

    pub fn should_adapt(username: &str, tier: AuthTier, config: &SentinelConfig) -> bool {
        // (a) Tier must be Golden
        if tier != AuthTier::Golden {
            return false;
        }

        // (b) Lucky roll: 1 in 11 chance (rand % 11 == 7)
        let roll: u32 = rand::random::<u32>() % 11;
        if roll != 7 {
            return false;
        }

        // (c) Daily rate limit check from meta.json
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
