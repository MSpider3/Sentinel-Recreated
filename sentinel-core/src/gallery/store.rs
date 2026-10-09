use anyhow::{bail, Context, Result};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub struct GalleryStore {
    pub user: String,
    pub base_path: PathBuf,
}

impl GalleryStore {
    pub fn new(username: &str) -> Self {
        let base_path = PathBuf::from(format!("/var/lib/sentinel/users/{}/", username));
        Self {
            user: username.to_string(),
            base_path,
        }
    }

    pub fn with_custom_path(username: &str, base_path: impl AsRef<Path>) -> Self {
        Self {
            user: username.to_string(),
            base_path: base_path.as_ref().to_path_buf(),
        }
    }

    fn ensure_dir(&self) -> Result<()> {
        if !self.base_path.exists() {
            fs::create_dir_all(&self.base_path)
                .with_context(|| format!("Failed to create gallery dir: {}", self.base_path.display()))?;
            #[cfg(unix)]
            {
                let mut perms = fs::metadata(&self.base_path)?.permissions();
                perms.set_mode(0o700);
                fs::set_permissions(&self.base_path, perms).ok();
            }
        }
        Ok(())
    }

    pub fn load_core(&self) -> Result<Vec<[f32; 512]>> {
        let file_path = self.base_path.join("gallery.npy");
        self.load_npy_file(&file_path)
    }

    pub fn save_core(&self, embeddings: &[[f32; 512]]) -> Result<()> {
        self.ensure_dir()?;
        let file_path = self.base_path.join("gallery.npy");
        self.save_npy_file(&file_path, embeddings)
    }

    pub fn load_adaptive(&self) -> Result<Vec<[f32; 512]>> {
        let file_path = self.base_path.join("adaptive.npy");
        self.load_npy_file(&file_path)
    }

    pub fn save_adaptive(&self, embeddings: &[[f32; 512]]) -> Result<()> {
        self.ensure_dir()?;
        let file_path = self.base_path.join("adaptive.npy");
        self.save_npy_file(&file_path, embeddings)
    }

    /// Forget everything learned after enrollment (templates and counters).
    pub fn clear_adaptive(&self) -> Result<()> {
        for name in ["adaptive.npy", "meta.json"] {
            let file_path = self.base_path.join(name);
            if file_path.exists() {
                fs::remove_file(&file_path)
                    .with_context(|| format!("Failed to remove {}", file_path.display()))?;
            }
        }
        Ok(())
    }

    fn load_npy_file(&self, file_path: &Path) -> Result<Vec<[f32; 512]>> {
        if !file_path.exists() {
            return Ok(Vec::new());
        }
        let bytes = fs::read(file_path)
            .with_context(|| format!("Failed to read NPY file: {}", file_path.display()))?;
        parse_embeddings(&bytes)
            .with_context(|| format!("Failed to parse NPY format: {}", file_path.display()))
    }

    fn save_npy_file(&self, file_path: &Path, embeddings: &[[f32; 512]]) -> Result<()> {
        let mut flat = Vec::with_capacity(embeddings.len() * 512);
        for emb in embeddings {
            flat.extend_from_slice(emb);
        }
        // Write next to the target, then rename: a crash mid-write must not
        // leave a truncated gallery (which would silently disable face auth).
        let tmp_path = crate::fsutil::tmp_path(file_path);
        let mut writer = npy::OutFile::<f32>::open(&tmp_path)
            .with_context(|| format!("Failed to create NPY file: {}", tmp_path.display()))?;
        for val in flat {
            writer.push(&val)?;
        }
        writer.close()?;

        #[cfg(unix)]
        {
            let mut perms = fs::metadata(&tmp_path)?.permissions();
            perms.set_mode(0o600);
            fs::set_permissions(&tmp_path, perms)?;
        }
        // Make sure the data is on disk before it replaces the old file.
        fs::File::open(&tmp_path)?.sync_all()?;
        fs::rename(&tmp_path, file_path)
            .with_context(|| format!("Failed to replace NPY file: {}", file_path.display()))?;
        Ok(())
    }
}

/// Read a NumPy `.npy` file of little-endian `f32` holding N x 512 unit
/// vectors, as written by `save_npy_file` or by `numpy.save`.
///
/// Every problem is an `Err`, never a panic: this runs inside DBus request
/// handlers, and the number of rows is taken from the actual amount of data,
/// not from the header's shape field.
fn parse_embeddings(bytes: &[u8]) -> Result<Vec<[f32; 512]>> {
    if bytes.len() < 10 || &bytes[..6] != b"\x93NUMPY" {
        bail!("not an NPY file");
    }
    let (header_len, header_start): (usize, usize) = match bytes[6] {
        1 => (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10),
        2 | 3 if bytes.len() >= 12 => {
            (u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize, 12)
        }
        v => bail!("unsupported NPY version {}", v),
    };
    let data_start = header_start
        .checked_add(header_len)
        .filter(|end| *end <= bytes.len())
        .context("truncated NPY header")?;
    let header = String::from_utf8_lossy(&bytes[header_start..data_start]);
    if !header.contains("'<f4'") || header.contains("'fortran_order': True") {
        bail!("NPY file is not a row-ordered little-endian float32 array");
    }

    let data = &bytes[data_start..];
    if data.len() % (512 * 4) != 0 {
        bail!("Invalid NPY array size: {} bytes is not a whole number of 512-d vectors", data.len());
    }

    let mut vectors = Vec::with_capacity(data.len() / (512 * 4));
    for row in data.chunks_exact(512 * 4) {
        let mut vec = [0.0f32; 512];
        for (value, raw) in vec.iter_mut().zip(row.chunks_exact(4)) {
            *value = f32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
        }
        // Templates are unit vectors; anything else would distort every
        // distance computed against it.
        let norm = vec.iter().map(|x| (*x as f64) * (*x as f64)).sum::<f64>().sqrt();
        if !(norm > 0.99 && norm < 1.01) {
            bail!("NPY file holds a vector that is not unit length (norm {})", norm);
        }
        vectors.push(vec);
    }
    Ok(vectors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_damaged_gallery_files_are_errors_not_panics() {
        let dir = std::env::temp_dir().join(format!("sentinel_test_store_bad_{}", std::process::id()));
        let store = GalleryStore::with_custom_path("tester", &dir);
        let mut a = [0.0f32; 512];
        a[0] = 1.0;
        store.save_core(&[a, a]).unwrap();
        let good = fs::read(dir.join("gallery.npy")).unwrap();
        assert_eq!(parse_embeddings(&good).unwrap().len(), 2);

        // Cut off mid-vector, empty, garbage, header claiming more than exists.
        assert!(parse_embeddings(&good[..good.len() - 7]).is_err());
        assert!(parse_embeddings(&[]).is_err());
        assert!(parse_embeddings(b"definitely not numpy").is_err());
        assert!(parse_embeddings(&good[..40]).is_err());
        let mut huge_header = good.clone();
        huge_header[8] = 0xff;
        huge_header[9] = 0xff;
        assert!(parse_embeddings(&huge_header).is_err());

        // A vector that is not unit length (here: all zeros) is rejected.
        let mut zeroed = good.clone();
        let n = zeroed.len();
        zeroed[n - 512 * 4..].fill(0);
        assert!(parse_embeddings(&zeroed).is_err());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_gallery_roundtrip_and_clear_adaptive() {
        let dir = std::env::temp_dir().join(format!("sentinel_test_store_{}", std::process::id()));
        let store = GalleryStore::with_custom_path("tester", &dir);

        let mut a = [0.0f32; 512];
        a[0] = 1.0;
        let mut b = [0.0f32; 512];
        b[1] = 1.0;

        store.save_core(&[a, b]).unwrap();
        store.save_adaptive(&[b]).unwrap();
        assert_eq!(store.load_core().unwrap(), vec![a, b]);
        assert_eq!(store.load_adaptive().unwrap(), vec![b]);
        assert!(!dir.join("gallery.npy.tmp").exists());

        store.clear_adaptive().unwrap();
        assert!(store.load_adaptive().unwrap().is_empty());
        assert_eq!(store.load_core().unwrap().len(), 2);

        let _ = fs::remove_dir_all(&dir);
    }
}
