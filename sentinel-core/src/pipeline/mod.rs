pub mod align;
pub mod authenticator;
pub mod capture;
pub mod decision;
pub mod detect;
pub mod embed;
pub mod r#match;
pub mod models;
pub mod preview;
pub mod quality;
pub mod spoof;

pub use align::align_face;
pub use authenticator::{AuthResult, AuthState, SentinelAuthenticator};
pub use capture::{FrameCapture, CapturedFrame};
pub use detect::{FaceDetection, ScrfdDetector, RawCandidate, ScrfdResult};
pub use embed::MobileFaceNet;
pub use models::{init_onnx_runtime, Models};
pub use r#match::{cosine_distance, decide_tier, decide_tier_with_config, match_gallery, match_gallery_with_config, AuthTier};
pub use preview::{
    DebugPreviewWindow,
    COLOR_TIER1, COLOR_TIER2, COLOR_TIER3, COLOR_TIER4, COLOR_CALIB,
};
pub use spoof::SpoofDetector;
