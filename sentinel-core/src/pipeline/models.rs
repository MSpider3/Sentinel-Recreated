use anyhow::{Context, Result};
use image::RgbImage;
use ort::environment::GlobalThreadPoolOptions;
use std::path::Path;

use crate::config::DetectionConfig;
use crate::pipeline::{MobileFaceNet, ScrfdDetector, SpoofDetector};

/// Anti-spoof models and the crop scale each one was trained with. The first
/// is required for anti-spoofing to be available; the second is used when present.
const SPOOF_MODELS: [(&str, f32); 2] = [("MiniFASNetV2.onnx", 2.7), ("MiniFASNetV1SE.onnx", 4.0)];

/// Set up ONNX Runtime with one thread pool shared by all models. Threads do
/// not busy-wait between runs, so an idle daemon uses no CPU. Call once,
/// before any model is loaded; without it ONNX Runtime uses its defaults.
pub fn init_onnx_runtime(num_threads: usize) -> Result<()> {
    let pool = GlobalThreadPoolOptions::default()
        .with_intra_threads(num_threads)
        .and_then(|pool| pool.with_spin_control(false))
        .map_err(|e| anyhow::anyhow!("Failed to configure ONNX Runtime thread pool: {}", e))?;
    ort::init().with_global_thread_pool(pool).commit();
    Ok(())
}

/// All neural networks, loaded once and reused for every authentication.
pub struct Models {
    pub detector: ScrfdDetector,
    pub embedder: MobileFaceNet,
    /// `None` when no anti-spoof model could be loaded. Authentication must
    /// then refuse to grant access rather than run without the check.
    pub spoof: Option<SpoofDetector>,
}

impl Models {
    pub fn load(models_dir: &Path, detection: &DetectionConfig) -> Result<Self> {
        let path = |name: &str| models_dir.join(name).to_string_lossy().into_owned();

        let mut detector = ScrfdDetector::new_with_input_size(
            &path("scrfd_500m_kps.onnx"),
            detection.score_threshold,
            detection.nms_threshold,
            detection.min_face_size_px,
            detection.scrfd_input_size,
        )?;
        let mut embedder = MobileFaceNet::new(&path("mobile_facenet.onnx"))?;

        let spoof_paths: Vec<(String, f32)> = SPOOF_MODELS
            .iter()
            .filter(|(name, _)| models_dir.join(name).exists())
            .map(|(name, scale)| (path(name), *scale))
            .collect();
        let spoof_refs: Vec<(&str, f32)> = spoof_paths.iter().map(|(p, s)| (p.as_str(), *s)).collect();
        let spoof = if models_dir.join(SPOOF_MODELS[0].0).exists() {
            match SpoofDetector::new(&spoof_refs) {
                Ok(s) => {
                    log::info!("Anti-spoof ready with {} model(s).", spoof_refs.len());
                    Some(s)
                }
                Err(e) => {
                    log::error!("Anti-spoof model failed to load: {:#}", e);
                    None
                }
            }
        } else {
            log::error!("Anti-spoof model {} is missing from {}.", SPOOF_MODELS[0].0, models_dir.display());
            None
        };

        // One dummy run per model: the first inference allocates buffers and
        // starts the thread pool, which should not happen during a login.
        detector.detect(&RgbImage::new(640, 480)).context("SCRFD warm-up run failed")?;
        embedder.embed(&RgbImage::from_pixel(112, 112, image::Rgb([128, 96, 64]))).ok();

        Ok(Self { detector, embedder, spoof })
    }
}
