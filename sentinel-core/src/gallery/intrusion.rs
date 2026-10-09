use anyhow::{Context, Result};
use chrono::Local;
use image::RgbImage;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Newest photos kept; older ones are deleted.
const MAX_PHOTOS: usize = 20;
/// Photos older than this are deleted.
const MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);

/// Photo log of people who were clearly not the enrolled user.
///
/// This is a record for the owner to look at, nothing more: the photos are
/// never used to decide an authentication.
pub struct IntrusionLog {
    pub dir: PathBuf,
}

impl Default for IntrusionLog {
    fn default() -> Self {
        Self::new()
    }
}

impl IntrusionLog {
    pub fn new() -> Self {
        Self {
            // Directory name kept from earlier versions so existing photos stay listed.
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
                .with_context(|| format!("Failed to create intrusion dir: {}", self.dir.display()))?;
            let mut perms = fs::metadata(&self.dir)?.permissions();
            perms.set_mode(0o700);
            fs::set_permissions(&self.dir, perms).ok();
        }
        Ok(())
    }

    /// Photo file names, oldest first (the timestamp in the name sorts them).
    fn photos(&self) -> Vec<PathBuf> {
        let mut photos: Vec<PathBuf> = fs::read_dir(&self.dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| {
                        p.file_name()
                            .and_then(|n| n.to_str())
                            .map_or(false, |n| n.starts_with("intrusion_") && n.ends_with(".jpg"))
                    })
                    .collect()
            })
            .unwrap_or_default();
        photos.sort();
        photos
    }

    fn prune(&self) {
        let photos = self.photos();
        let excess = photos.len().saturating_sub(MAX_PHOTOS);
        for (i, photo) in photos.iter().enumerate() {
            let too_old = fs::metadata(photo)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| SystemTime::now().duration_since(t).ok())
                .map_or(false, |age| age > MAX_AGE);
            if i < excess || too_old {
                let _ = fs::remove_file(photo);
            }
        }
    }

    /// Save a photo as intrusion_YYYYMMDD_HHMMSS.jpg and drop old ones.
    pub fn save(&self, frame: &RgbImage) -> Result<PathBuf> {
        self.ensure_dir()?;

        let timestamp = Local::now().format("%Y%m%d_%H%M%S").to_string();
        let jpg_path = self.dir.join(format!("intrusion_{}.jpg", timestamp));
        frame
            .save(&jpg_path)
            .with_context(|| format!("Failed to save intrusion JPG: {}", jpg_path.display()))?;

        if let Ok(m) = fs::metadata(&jpg_path) {
            let mut perms = m.permissions();
            perms.set_mode(0o600);
            fs::set_permissions(&jpg_path, perms).ok();
        }

        self.prune();
        Ok(jpg_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_save_and_keep_only_newest() {
        let tmp_dir = std::env::temp_dir().join(format!("sentinel_test_intrusion_{}", std::process::id()));
        let log = IntrusionLog::with_custom_path(&tmp_dir);
        let frame = RgbImage::new(64, 48);

        let saved = log.save(&frame).unwrap();
        assert!(saved.exists());

        // Older photos beyond the cap are removed, newest are kept.
        for i in 0..MAX_PHOTOS + 5 {
            fs::write(tmp_dir.join(format!("intrusion_20200101_{:06}.jpg", i)), b"x").unwrap();
        }
        fs::write(tmp_dir.join("notes.txt"), b"keep me").unwrap();
        log.prune();

        let left = log.photos();
        assert_eq!(left.len(), MAX_PHOTOS);
        assert!(left.contains(&saved));
        assert!(!tmp_dir.join("intrusion_20200101_000000.jpg").exists());
        assert!(tmp_dir.join("notes.txt").exists());

        let _ = fs::remove_dir_all(&tmp_dir);
    }
}
