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
        let bounded = if embeddings.len() > 20 {
            &embeddings[embeddings.len() - 20..]
        } else {
            embeddings
        };
        let file_path = self.base_path.join("adaptive.npy");
        self.save_npy_file(&file_path, bounded)
    }

    pub fn all_vectors(&self) -> Result<Vec<[f32; 512]>> {
        let mut all = self.load_core().unwrap_or_default();
        let adaptive = self.load_adaptive().unwrap_or_default();
        all.extend(adaptive);
        Ok(all)
    }

    fn load_npy_file(&self, file_path: &Path) -> Result<Vec<[f32; 512]>> {
        if !file_path.exists() {
            return Ok(Vec::new());
        }
        let bytes = fs::read(file_path)
            .with_context(|| format!("Failed to read NPY file: {}", file_path.display()))?;
        let data: npy::NpyData<f32> = npy::NpyData::from_bytes(&bytes)
            .with_context(|| format!("Failed to parse NPY format: {}", file_path.display()))?;
        let raw: Vec<f32> = data.to_vec();
        if raw.len() % 512 != 0 {
            bail!("Invalid NPY array size {}, not a multiple of 512", raw.len());
        }
        let mut vectors = Vec::with_capacity(raw.len() / 512);
        for chunk in raw.chunks_exact(512) {
            let mut vec = [0.0f32; 512];
            vec.copy_from_slice(chunk);
            vectors.push(vec);
        }
        Ok(vectors)
    }

    fn save_npy_file(&self, file_path: &Path, embeddings: &[[f32; 512]]) -> Result<()> {
        let mut flat = Vec::with_capacity(embeddings.len() * 512);
        for emb in embeddings {
            flat.extend_from_slice(emb);
        }
        let mut writer = npy::OutFile::<f32>::open(file_path)
            .with_context(|| format!("Failed to create NPY file: {}", file_path.display()))?;
        for val in flat {
            writer.push(&val)?;
        }
        writer.close()?;

        #[cfg(unix)]
        {
            let mut perms = fs::metadata(file_path)?.permissions();
            perms.set_mode(0o600);
            fs::set_permissions(file_path, perms).ok();
        }
        Ok(())
    }
}
