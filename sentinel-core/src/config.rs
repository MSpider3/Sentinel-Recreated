use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct SentinelConfig {
    #[serde(default)]
    pub camera: CameraConfig,
    #[serde(default)]
    pub detection: DetectionConfig,
    #[serde(default)]
    pub security: SecurityConfig,
    #[serde(default)]
    pub adaptive_policy: AdaptivePolicyConfig,
    #[serde(default)]
    pub hardware: HardwareConfig,
}

fn default_camera_source() -> String {
    "/dev/video0".to_string()
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct CameraConfig {
    #[serde(default = "default_camera_source")]
    pub source: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

impl Default for CameraConfig {
    fn default() -> Self {
        Self {
            source: "/dev/video0".to_string(),
            width: 640,
            height: 480,
            fps: 30,
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct DetectionConfig {
    #[serde(default = "default_scrfd_input_size")]
    pub scrfd_input_size: u32,
    pub score_threshold: f32,
    pub nms_threshold: f32,
    pub min_face_size_px: u32,
}

fn default_scrfd_input_size() -> u32 {
    320
}

impl Default for DetectionConfig {
    fn default() -> Self {
        Self {
            scrfd_input_size: 320,
            score_threshold: 0.50,
            nms_threshold: 0.30,
            min_face_size_px: 40,
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct SecurityConfig {
    pub golden_threshold: f32,
    pub standard_threshold: f32,
    pub two_factor_threshold: f32,
    #[serde(default = "default_recognition_threshold")]
    pub recognition_threshold: f32,
    pub spoof_threshold: f32,
    pub max_retries: u32,
    pub challenge_timeout_secs: f64,
    pub gallery_max_size: usize,
}

fn default_recognition_threshold() -> f32 {
    0.38
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            golden_threshold: 0.28,
            standard_threshold: 0.42,
            two_factor_threshold: 0.50,
            recognition_threshold: 0.38,
            spoof_threshold: 0.80,
            max_retries: 3,
            challenge_timeout_secs: 20.0,
            gallery_max_size: 20,
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct AdaptivePolicyConfig {
    pub adaptation_limit_per_day: u32,
}

impl Default for AdaptivePolicyConfig {
    fn default() -> Self {
        Self {
            adaptation_limit_per_day: 1,
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct HardwareConfig {
    pub execution_provider: String,
    pub onnx_num_threads: usize,
    pub frame_drop_threshold_ms: u64,
}

impl Default for HardwareConfig {
    fn default() -> Self {
        Self {
            execution_provider: "cpu".to_string(),
            onnx_num_threads: 2,
            frame_drop_threshold_ms: 200,
        }
    }
}

impl Default for SentinelConfig {
    fn default() -> Self {
        Self {
            camera: CameraConfig::default(),
            detection: DetectionConfig::default(),
            security: SecurityConfig::default(),
            adaptive_policy: AdaptivePolicyConfig::default(),
            hardware: HardwareConfig::default(),
        }
    }
}

impl SentinelConfig {
    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let p = path.as_ref();
        if !p.exists() {
            return Ok(Self::default());
        }
        let content = fs::read_to_string(p)
            .with_context(|| format!("Failed to read config file: {}", p.display()))?;
        let config: SentinelConfig = toml::from_str(&content)
            .with_context(|| format!("Failed to parse TOML config from: {}", p.display()))?;
        Ok(config)
    }
}
