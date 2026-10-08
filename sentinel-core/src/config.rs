use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// Longest a face scan may run. `pam_sentinel.so` waits 8 s for the daemon's
/// reply, so the daemon must always give up first.
pub const MAX_SESSION_TIMEOUT_SECS: f64 = 7.0;

// Every section uses `#[serde(default)]`, so a config file may set only the
// keys it cares about; the rest keep their defaults.

/// Write an `f32` setting as the short decimal a person typed (0.4), not its
/// exact binary value (0.4000000059604645).
fn short_decimal<S: serde::Serializer>(value: &f32, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_f64((*value as f64 * 10_000.0).round() / 10_000.0)
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(default)]
pub struct SentinelConfig {
    pub camera: CameraConfig,
    pub detection: DetectionConfig,
    pub security: SecurityConfig,
    pub adaptive_policy: AdaptivePolicyConfig,
    pub hardware: HardwareConfig,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(default)]
pub struct CameraConfig {
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
#[serde(default)]
pub struct DetectionConfig {
    pub scrfd_input_size: u32,
    #[serde(serialize_with = "short_decimal")]
    pub score_threshold: f32,
    #[serde(serialize_with = "short_decimal")]
    pub nms_threshold: f32,
    pub min_face_size_px: u32,
}

impl Default for DetectionConfig {
    fn default() -> Self {
        Self {
            scrfd_input_size: 320,
            score_threshold: 0.50,
            nms_threshold: 0.30,
            min_face_size_px: 80,
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(default)]
pub struct SecurityConfig {
    /// Cosine distance below which a match is "strong" (one frame is enough).
    #[serde(serialize_with = "short_decimal")]
    pub golden_threshold: f32,
    /// Cosine distance below which a match is "normal" (several frames needed).
    #[serde(serialize_with = "short_decimal")]
    pub standard_threshold: f32,
    /// Above this distance the face is treated as a clearly different person.
    #[serde(serialize_with = "short_decimal")]
    pub two_factor_threshold: f32,
    /// Anti-spoof "real" score a strong match needs to be granted on one frame.
    #[serde(serialize_with = "short_decimal")]
    pub spoof_threshold: f32,
    /// Anti-spoof "real" score every frame of a voted grant must reach.
    /// A matching frame below it counts as a spoof strike.
    #[serde(serialize_with = "short_decimal")]
    pub spoof_threshold_standard: f32,
    /// Spoof strikes allowed before the session ends as SPOOF.
    pub max_retries: u32,
    /// Seconds before a face scan gives up and the password prompt takes over.
    pub global_session_timeout: f64,
    /// Maximum number of learned (adaptive) templates kept per user.
    pub gallery_max_size: usize,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            golden_threshold: 0.28,
            standard_threshold: 0.42,
            two_factor_threshold: 0.50,
            // Measured on the target laptop webcam: the live owner scored
            // 0.84-1.00; a photo and a phone video of the owner scored
            // 0.00-0.24, except one photo session at 0.58-0.64. Both
            // thresholds sit between those groups.
            spoof_threshold: 0.80,
            spoof_threshold_standard: 0.70,
            max_retries: 3,
            global_session_timeout: MAX_SESSION_TIMEOUT_SECS,
            gallery_max_size: 20,
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(default)]
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
#[serde(default)]
pub struct HardwareConfig {
    /// Threads for the shared ONNX Runtime pool. Use the physical core count.
    pub onnx_num_threads: usize,
}

impl Default for HardwareConfig {
    fn default() -> Self {
        Self { onnx_num_threads: 2 }
    }
}

impl SentinelConfig {
    /// Load and validate the config file. A missing file means defaults; a
    /// file that does not parse or holds unsafe values is an error.
    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let p = path.as_ref();
        if !p.exists() {
            return Ok(Self::default());
        }
        let content = fs::read_to_string(p)
            .with_context(|| format!("Failed to read config file: {}", p.display()))?;
        Self::from_toml(&content).with_context(|| format!("Invalid config file: {}", p.display()))
    }

    pub fn from_toml(content: &str) -> anyhow::Result<Self> {
        let mut config: SentinelConfig = toml::from_str(content).context("Failed to parse TOML")?;
        for key in unknown_keys(content) {
            log::warn!("Config key '{}' is not used by this version and is ignored — check for a typo.", key);
        }
        // Configs written by earlier versions carry a 25 s timeout; cap it
        // rather than rejecting the whole file.
        if config.security.global_session_timeout > MAX_SESSION_TIMEOUT_SECS {
            log::warn!(
                "security.global_session_timeout capped from {} to {} seconds",
                config.security.global_session_timeout,
                MAX_SESSION_TIMEOUT_SECS
            );
            config.security.global_session_timeout = MAX_SESSION_TIMEOUT_SECS;
        }
        config.validate()?;
        Ok(config)
    }

    /// Reject values that would make authentication unsafe or unusable.
    pub fn validate(&self) -> anyhow::Result<()> {
        fn check<T: PartialOrd + std::fmt::Display>(name: &str, value: T, min: T, max: T) -> anyhow::Result<()> {
            // Written so that NaN (which compares false) is rejected too.
            if !(value >= min && value <= max) {
                bail!("{} = {} is outside the allowed range {}..={}", name, value, min, max);
            }
            Ok(())
        }

        if !crate::pipeline::capture::is_valid_source(&self.camera.source) {
            bail!("camera.source = {:?} must be /dev/videoN or pipewiresrc", self.camera.source);
        }
        check("camera.width", self.camera.width, 160, 1920)?;
        check("camera.height", self.camera.height, 120, 1080)?;
        check("camera.fps", self.camera.fps, 1, 60)?;

        check("detection.scrfd_input_size", self.detection.scrfd_input_size, 160, 1280)?;
        check("detection.score_threshold", self.detection.score_threshold, 0.10, 0.99)?;
        check("detection.nms_threshold", self.detection.nms_threshold, 0.05, 0.95)?;
        check("detection.min_face_size_px", self.detection.min_face_size_px, 20, 400)?;

        let s = &self.security;
        check("security.golden_threshold", s.golden_threshold, 0.05, 0.50)?;
        check("security.standard_threshold", s.standard_threshold, 0.05, 0.50)?;
        check("security.two_factor_threshold", s.two_factor_threshold, 0.05, 0.70)?;
        if !(s.golden_threshold < s.standard_threshold && s.standard_threshold < s.two_factor_threshold) {
            bail!("security thresholds must satisfy golden < standard < two_factor");
        }
        check("security.spoof_threshold", s.spoof_threshold, 0.30, 0.999)?;
        check("security.spoof_threshold_standard", s.spoof_threshold_standard, 0.30, 0.999)?;
        if s.spoof_threshold_standard > s.spoof_threshold {
            bail!("security.spoof_threshold_standard must not exceed security.spoof_threshold");
        }
        check("security.max_retries", s.max_retries, 1, 10)?;
        check("security.global_session_timeout", s.global_session_timeout, 2.0, MAX_SESSION_TIMEOUT_SECS)?;
        check("security.gallery_max_size", s.gallery_max_size, 1, 100)?;

        check("adaptive_policy.adaptation_limit_per_day", self.adaptive_policy.adaptation_limit_per_day, 0, 10)?;
        check("hardware.onnx_num_threads", self.hardware.onnx_num_threads, 1, 16)?;
        Ok(())
    }
}

/// `section.key` names present in `content` that this version does not know.
fn unknown_keys(content: &str) -> Vec<String> {
    let known = toml::Value::try_from(SentinelConfig::default()).ok();
    let given: Option<toml::Value> = toml::from_str(content).ok();
    let (Some(toml::Value::Table(known)), Some(toml::Value::Table(given))) = (known, given) else {
        return Vec::new();
    };

    let mut unknown = Vec::new();
    for (section, value) in &given {
        match (known.get(section), value) {
            (Some(toml::Value::Table(known_keys)), toml::Value::Table(given_keys)) => {
                for key in given_keys.keys().filter(|k| !known_keys.contains_key(*k)) {
                    unknown.push(format!("{}.{}", section, key));
                }
            }
            (None, _) => unknown.push(section.clone()),
            _ => {}
        }
    }
    unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unknown_keys_are_reported() {
        let keys = unknown_keys("[security]\nspoof_treshold = 0.9\ngolden_threshold = 0.2\n\n[storage]\ndata_dir = \"/x\"\n");
        assert_eq!(keys, vec!["security.spoof_treshold".to_string(), "storage".to_string()]);
        assert!(unknown_keys("[security]\ngolden_threshold = 0.2\n").is_empty());
    }

    #[test]
    fn test_config_is_written_with_short_decimals() {
        let text = toml::to_string(&SentinelConfig::default()).unwrap();
        assert!(text.contains("golden_threshold = 0.28\n"), "{}", text);
        assert!(text.contains("nms_threshold = 0.3\n"), "{}", text);
        // ...and what is written loads back unchanged.
        let back = SentinelConfig::from_toml(&text).unwrap();
        assert_eq!(back.security.spoof_threshold, SentinelConfig::default().security.spoof_threshold);
    }

    #[test]
    fn test_defaults_are_valid() {
        SentinelConfig::default().validate().unwrap();
    }

    #[test]
    fn test_partial_sections_keep_other_defaults() {
        let parsed = SentinelConfig::from_toml("[security]\ngolden_threshold = 0.25\n\n[camera]\nfps = 15\n").unwrap();
        assert_eq!(parsed.security.golden_threshold, 0.25);
        assert_eq!(parsed.security.standard_threshold, 0.42);
        assert_eq!(parsed.camera.fps, 15);
        assert_eq!(parsed.camera.source, "/dev/video0");
    }

    #[test]
    fn test_old_config_keys_are_ignored() {
        // Keys from earlier versions must not stop an existing config from loading.
        let parsed = SentinelConfig::from_toml(
            "[security]\nrecognition_threshold = 0.38\nchallenge_timeout_secs = 20.0\nrequire_liveness = true\n\n[hardware]\nexecution_provider = \"cpu\"\n",
        );
        assert!(parsed.is_ok());
    }

    #[test]
    fn test_long_timeout_from_old_config_is_capped() {
        let parsed = SentinelConfig::from_toml("[security]\nglobal_session_timeout = 25.0\n").unwrap();
        assert_eq!(parsed.security.global_session_timeout, MAX_SESSION_TIMEOUT_SECS);
    }

    #[test]
    fn test_unsafe_values_are_rejected() {
        // A golden threshold this loose would match almost anyone.
        assert!(SentinelConfig::from_toml("[security]\ngolden_threshold = 2.0\n").is_err());
        assert!(SentinelConfig::from_toml("[security]\ngolden_threshold = 0.45\n").is_err()); // above standard
        assert!(SentinelConfig::from_toml("[security]\nspoof_threshold = 0.0\n").is_err());
        assert!(SentinelConfig::from_toml("[security]\nglobal_session_timeout = 0.5\n").is_err());
        assert!(SentinelConfig::from_toml("[security]\nstandard_threshold = nan\n").is_err());
        assert!(SentinelConfig::from_toml("[camera]\nsource = \"/dev/video0 ! filesink location=/x\"\n").is_err());
        assert!(SentinelConfig::from_toml("not toml at all").is_err());
    }
}
