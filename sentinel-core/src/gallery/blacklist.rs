use anyhow::{bail, Context, Result};
use chrono::Local;
use image::RgbImage;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::pipeline::r#match::cosine_distance;

pub struct BlacklistManager {
    pub dir: PathBuf,
}

impl Default for BlacklistManager {
    fn default() -> Self {
        Self::new()
    }
}

impl BlacklistManager {
    pub fn new() -> Self {
        Self {
            dir: PathBuf::from("/var/lib/sentinel/blacklist"),
        }
    }

    pub fn with_custom_path(dir: impl AsRef<Path>) -> Self {
        Self {
            dir: dir.as_ref().to_path_buf(),
        }
    }

    fn ensure_dir(&self) -> Result<()> {
        if !self.dir.exists() {
            fs::create_dir_all(&self.dir)
                .with_context(|| format!("Failed to create blacklist dir: {}", self.dir.display()))?;
            #[cfg(unix)]
            {
                let mut perms = fs::metadata(&self.dir)?.permissions();
                perms.set_mode(0o700);
                fs::set_permissions(&self.dir, perms).ok();
            }
        }
        Ok(())
    }

    pub fn load_vectors(&self) -> Result<Vec<[f32; 512]>> {
        let npy_path = self.dir.join("embeddings.npy");
        if !npy_path.exists() {
            return Ok(Vec::new());
        }
        let bytes = fs::read(&npy_path)
            .with_context(|| format!("Failed to read blacklist NPY: {}", npy_path.display()))?;
        let data: npy::NpyData<f32> = npy::NpyData::from_bytes(&bytes)
            .with_context(|| format!("Failed to parse blacklist NPY: {}", npy_path.display()))?;
        let raw: Vec<f32> = data.to_vec();
        if raw.len() % 512 != 0 {
            bail!("Invalid blacklist NPY array size {}, not a multiple of 512", raw.len());
        }
        let mut vectors = Vec::with_capacity(raw.len() / 512);
        for chunk in raw.chunks_exact(512) {
            let mut vec = [0.0f32; 512];
            vec.copy_from_slice(chunk);
            vectors.push(vec);
        }
        Ok(vectors)
    }

    /// Check if candidate embedding matches any blacklisted face (distance < 0.25).
    pub fn check(&self, embedding: &[f32; 512]) -> bool {
        let vectors = self.load_vectors().unwrap_or_default();
        for vec in &vectors {
            let d = cosine_distance(embedding, vec);
            if d < 0.25 {
                return true;
            }
        }
        false
    }

    /// Add an intruder embedding and screenshot to the blacklist directory.
    pub fn add_intruder(&self, embedding: &[f32; 512], frame: &RgbImage) -> Result<()> {
        self.ensure_dir()?;

        // 1. Append embedding vector to embeddings.npy
        let mut current = self.load_vectors().unwrap_or_default();
        current.push(*embedding);

        let npy_path = self.dir.join("embeddings.npy");
        let mut flat = Vec::with_capacity(current.len() * 512);
        for emb in &current {
            flat.extend_from_slice(emb);
        }
        let mut writer = npy::OutFile::<f32>::open(&npy_path)
            .with_context(|| format!("Failed to create blacklist NPY: {}", npy_path.display()))?;
        for val in flat {
            writer.push(&val)?;
        }
        writer.close()?;

        #[cfg(unix)]
        {
            if let Ok(m) = fs::metadata(&npy_path) {
                let mut perms = m.permissions();
                perms.set_mode(0o600);
                fs::set_permissions(&npy_path, perms).ok();
            }
        }

        // 2. Save JPEG screenshot intrusion_YYYYMMDD_HHMMSS.jpg
        let timestamp = Local::now().format("%Y%m%d_%H%M%S").to_string();
        let jpg_path = self.dir.join(format!("intrusion_{}.jpg", timestamp));
        frame.save(&jpg_path)
            .with_context(|| format!("Failed to save intrusion JPG: {}", jpg_path.display()))?;

        #[cfg(unix)]
        {
            if let Ok(m) = fs::metadata(&jpg_path) {
                let mut perms = m.permissions();
                perms.set_mode(0o600);
                fs::set_permissions(&jpg_path, perms).ok();
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blacklist_add_and_check() {
        let tmp_dir = std::env::temp_dir().join("sentinel_test_blacklist");
        let manager = BlacklistManager::with_custom_path(&tmp_dir);

        let mut dummy_emb = [0.0f32; 512];
        dummy_emb[0] = 1.0;

        let frame = RgbImage::new(100, 100);

        assert!(!manager.check(&dummy_emb));
        manager.add_intruder(&dummy_emb, &frame).unwrap();
        assert!(manager.check(&dummy_emb));

        let _ = fs::remove_dir_all(&tmp_dir);
    }
}
