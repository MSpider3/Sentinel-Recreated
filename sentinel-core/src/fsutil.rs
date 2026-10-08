use anyhow::{Context, Result};
use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Sibling path used while a file is being written.
pub fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

/// Replace `path` with `bytes` in one step: write a temporary file next to
/// it, flush it to disk, then rename. A crash or power cut leaves either the
/// old file or the new one, never a half-written one.
pub fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let tmp = tmp_path(path);
    let result = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(mode)
            .open(&tmp)
            .with_context(|| format!("Failed to create {}", tmp.display()))?;
        // `mode` only applies when the file is created; enforce it either way.
        file.set_permissions(fs::Permissions::from_mode(mode))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&tmp, path).with_context(|| format!("Failed to replace {}", path.display()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_atomic_replaces_and_sets_mode() {
        let dir = std::env::temp_dir().join(format!("sentinel_test_fsutil_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("data.json");

        write_atomic(&path, b"first", 0o600).unwrap();
        write_atomic(&path, b"second", 0o600).unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"second");
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(!tmp_path(&path).exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
