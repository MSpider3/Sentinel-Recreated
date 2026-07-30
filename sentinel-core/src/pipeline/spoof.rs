use anyhow::{Context, Result};
use image::{imageops, RgbImage};
use ort::execution_providers::{CPUExecutionProvider, OpenVINOExecutionProvider};
use ort::{session::Session, value::Tensor};
use serde::{Deserialize, Serialize};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CalibConfig {
    pub use_rgb: bool,
    pub live_idx: usize,
    pub calibrated: bool,
}

impl Default for CalibConfig {
    fn default() -> Self {
        Self {
            use_rgb: true,
            live_idx: 1,
            calibrated: false,
        }
    }
}

pub fn softmax(logits: &[f32]) -> Vec<f32> {
    if logits.is_empty() {
        return Vec::new();
    }
    let max_val = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let exps: Vec<f32> = logits.iter().map(|x| (x - max_val).exp()).collect();
    let sum_exp: f32 = exps.iter().sum();
    if sum_exp < 1e-10 {
        return vec![1.0 / (logits.len() as f32); logits.len()];
    }
    exps.into_iter().map(|x| x / sum_exp).collect()
}

pub struct SpoofDetector {
    session: Session,
    calib_path: PathBuf,
    calib_config: CalibConfig,
    threshold: f32,
    calib_samples: Vec<Vec<f32>>,
}

impl SpoofDetector {
    pub fn new(model_path: &str, calib_path: &str, threshold: f32) -> Result<Self> {
        let session = Session::builder()
            .map_err(|e| anyhow::anyhow!("{:?}", e))?
            .with_execution_providers([
                OpenVINOExecutionProvider::default().build(),
                CPUExecutionProvider::default().build(),
            ])
            .map_err(|e| anyhow::anyhow!("{:?}", e))?
            .with_intra_threads(2)
            .map_err(|e| anyhow::anyhow!("{:?}", e))?
            .commit_from_file(model_path)
            .with_context(|| format!("Failed to load MiniFASNet model from: {}", model_path))?;

        let calib_p = PathBuf::from(calib_path);
        let calib_config = if calib_p.exists() {
            let content = fs::read_to_string(&calib_p).unwrap_or_default();
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            CalibConfig::default()
        };

        Ok(Self {
            session,
            calib_path: calib_p,
            calib_config,
            threshold,
            calib_samples: Vec::new(),
        })
    }

    pub fn is_calibrating(&self) -> bool {
        !self.calib_config.calibrated
    }

    /// Number of calibration samples collected so far (0..=80).
    pub fn calib_samples_len(&self) -> usize {
        self.calib_samples.len()
    }

    /// Square-crop the face region with `scale` expansion, resize to 80x80.
    pub fn square_crop(frame: &RgbImage, bbox: [f32; 4], scale: f32) -> Result<RgbImage> {
        let (fw, fh) = frame.dimensions();
        let x1 = bbox[0];
        let y1 = bbox[1];
        let x2 = bbox[2];
        let y2 = bbox[3];

        let w = x2 - x1;
        let h = y2 - y1;
        let cx = x1 + w / 2.0;
        let cy = y1 + h / 2.0;

        let max_side = w.max(h) * scale;
        let new_x1 = ((cx - max_side / 2.0).max(0.0) as u32).min(fw.saturating_sub(1));
        let new_y1 = ((cy - max_side / 2.0).max(0.0) as u32).min(fh.saturating_sub(1));
        let new_w = (max_side as u32).min(fw - new_x1).max(1);
        let new_h = (max_side as u32).min(fh - new_y1).max(1);

        let crop = imageops::crop_imm(frame, new_x1, new_y1, new_w, new_h).to_image();
        let resized = imageops::resize(&crop, 80, 80, imageops::FilterType::Triangle);
        Ok(resized)
    }

    fn extract_logits(&mut self, crop_80x80: &RgbImage, use_rgb: bool) -> Result<Vec<f32>> {
        let mut flat = Vec::with_capacity(1 * 3 * 80 * 80);
        for ch in 0..3usize {
            for r in 0..80u32 {
                for c in 0..80u32 {
                    let pixel = crop_80x80.get_pixel(c, r);
                    // pixel is RGB; ch 0=R,1=G,2=B
                    let val = if use_rgb {
                        pixel[ch] as f32
                    } else {
                        // BGR order: flip R(0) and B(2)
                        let bgr_ch = [2usize, 1, 0][ch];
                        pixel[bgr_ch] as f32
                    };
                    flat.push(val);
                }
            }
        }

        let input_tensor = Tensor::<f32>::from_array(([1usize, 3, 80, 80], flat.into_boxed_slice()))?;
        let outputs = self.session.run(ort::inputs![input_tensor])?;

        let output_val = outputs.values().next().context("No output from MiniFASNet")?;
        let (_shape, slice) = output_val.try_extract_tensor::<f32>()?;
        Ok(slice.to_vec())
    }

    pub fn calibrate_tick(&mut self, crop_80x80: &RgbImage) {
        if self.calib_config.calibrated {
            return;
        }
        if let Ok(logits) = self.extract_logits(crop_80x80, true) {
            self.calib_samples.push(logits);
        }
        if self.calib_samples.len() >= 80 {
            self.finish_calibration();
        }
    }

    fn finish_calibration(&mut self) {
        self.calib_config = CalibConfig {
            use_rgb: true,
            live_idx: 1,
            calibrated: true,
        };

        if let Some(parent) = self.calib_path.parent() {
            fs::create_dir_all(parent).ok();
        }

        if let Ok(json) = serde_json::to_string_pretty(&self.calib_config) {
            if fs::write(&self.calib_path, json).is_ok() {
                #[cfg(unix)]
                {
                    if let Ok(meta) = fs::metadata(&self.calib_path) {
                        let mut perms = meta.permissions();
                        perms.set_mode(0o600);
                        fs::set_permissions(&self.calib_path, perms).ok();
                    }
                }
            }
        }
    }

    pub fn predict(&mut self, crop_80x80: &RgbImage) -> Result<(bool, f32)> {
        let logits = self.extract_logits(crop_80x80, self.calib_config.use_rgb)?;
        let probs = softmax(&logits);
        let live_score = if self.calib_config.live_idx < probs.len() {
            probs[self.calib_config.live_idx]
        } else {
            probs[0]
        };
        let is_real = live_score >= self.threshold;
        Ok((is_real, live_score))
    }
}
