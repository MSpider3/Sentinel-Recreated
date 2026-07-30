/// auth-test binary — uses SentinelAuthenticator state machine.
/// Mirrors authenticate.py exactly:
///   Waiting -> Recognized -> Success/Failure/Require2FA
///
/// Usage:
///   sudo cargo run --bin auth-test -- --user testuser [--preview] [--save-debug-frames]

use anyhow::Result;
use clap::Parser;
use sentinel_core::config::SentinelConfig;
use sentinel_core::gallery::GalleryStore;
use sentinel_core::pipeline::{
    ActiveTier, AuthState, DebugPreviewWindow, FrameCapture, MobileFaceNet, ScrfdDetector,
    SentinelAuthenticator, SpoofDetector, COLOR_CALIB, COLOR_TIER1, COLOR_TIER2, COLOR_TIER3,
    COLOR_TIER4,
};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Sentinel Recreated — Full Authentication Test (SentinelAuthenticator state machine)",
    long_about = None
)]
struct Args {
    /// Target username to authenticate.
    #[arg(short, long)]
    user: String,

    /// Path to sentinel config file.
    #[arg(short, long, default_value = "/etc/sentinel/config.toml")]
    config: String,

    /// Directory containing ONNX models.
    #[arg(short, long, default_value = "/var/cache/sentinel/models")]
    models_dir: String,

    /// Path for MiniFASNet calibration JSON.
    #[arg(long, default_value = "/var/lib/sentinel/minifas_calib.json")]
    calib_path: String,

    /// Camera device index (0 = first webcam).
    #[arg(short, long, default_value_t = 0)]
    device: u32,

    /// Show a live preview window (requires a display server).
    #[arg(short, long)]
    preview: bool,

    /// Save debug frames to /tmp/sentinel_debug/.
    #[arg(long)]
    save_debug_frames: bool,
}

fn main() -> Result<()> {
    env_logger::init();
    let args = Args::parse();

    println!("=== Sentinel Recreated: Authentication Test ===");
    println!("Target User: {}", args.user);

    // ── Load gallery ─────────────────────────────────────────────────────────
    let store = GalleryStore::new(&args.user);
    let gallery = store.all_vectors()?;

    if gallery.is_empty() {
        eprintln!(
            "Error: No enrollment vectors for '{}'. Run enroll-test first.",
            args.user
        );
        std::process::exit(1);
    }
    println!("Loaded {} gallery vectors for '{}'.", gallery.len(), args.user);

    // ── Load config ──────────────────────────────────────────────────────────
    let config = SentinelConfig::load(&args.config).unwrap_or_default();
    let models = PathBuf::from(&args.models_dir);

    let scrfd_path = models.join("scrfd_500m_kps.onnx");
    let mfn_path   = models.join("mobile_facenet.onnx");
    let minifas    = models.join("MiniFASNetV2.onnx");

    if !scrfd_path.exists() || !mfn_path.exists() {
        eprintln!(
            "Error: Model files missing in {}. Run scripts/download_models.sh first.",
            models.display()
        );
        std::process::exit(1);
    }

    // ── Create models ────────────────────────────────────────────────────────
    let detector = ScrfdDetector::new(
        scrfd_path.to_str().unwrap(),
        config.detection.score_threshold,
        config.detection.nms_threshold,
        config.detection.min_face_size_px,
    )?;

    let embedder = MobileFaceNet::new(mfn_path.to_str().unwrap())?;

    let spoof = if minifas.exists() {
        println!("MiniFASNet anti-spoof loaded: {}", minifas.display());
        Some(SpoofDetector::new(
            minifas.to_str().unwrap(),
            &args.calib_path,
            config.security.spoof_threshold,
        )?)
    } else {
        println!("[Notice] MiniFASNet model not found — anti-spoofing bypassed.");
        None
    };

    // ── Build authenticator ──────────────────────────────────────────────────
    let mut auth = SentinelAuthenticator::new(
        detector,
        embedder,
        gallery,
        args.user.clone(),
        spoof,
    );

    // ── Camera ───────────────────────────────────────────────────────────────
    let source = if args.device != 0 {
        format!("/dev/video{}", args.device)
    } else {
        config.camera.source.clone()
    };
    let mut capture = FrameCapture::new(&source)?;
    capture.start()?;

    // ── Preview window ────────────────────────────────────────────────────────
    let mut preview: Option<DebugPreviewWindow> = if args.preview {
        println!("Preview window enabled (640×480, minifb).");
        Some(DebugPreviewWindow::new(
            "Sentinel Authentication Preview",
            640,
            480,
        )?)
    } else {
        None
    };

    if args.save_debug_frames {
        let _ = std::fs::create_dir_all("/tmp/sentinel_debug");
    }

    let mut frame_count = 0u64;
    let mut last_message = String::new();

    println!("Authentication started. Look at the camera...\n");

    // ── Main loop ─────────────────────────────────────────────────────────────
    loop {
        frame_count += 1;

        // Grab latest frame
        let captured = match capture.read_captured_frame() {
            Some(f) => f,
            None => {
                thread::sleep(Duration::from_millis(15));
                continue;
            }
        };

        let frame = &captured.image;

        // Process through state machine
        let result = match auth.process_frame(frame) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[Auth] process_frame error: {e}");
                thread::sleep(Duration::from_millis(33));
                continue;
            }
        };

        // Print status only when message changes (avoid spam)
        if result.message != last_message {
            let prefix = match result.state {
                AuthState::Waiting    => "[Waiting]",
                AuthState::Recognized => "[Recognized]",
                AuthState::Success    => "[SUCCESS]",
                AuthState::Failure    => "[FAILURE]",
                AuthState::Require2FA => "[2FA REQUIRED]",
            };
            println!("{} {}", prefix, result.message);
            if let (Some(user), Some(dist)) = (&result.matched_user, result.distance) {
                println!("  → User: {}  Distance: {:.4}", user, dist);
            }
            last_message = result.message.clone();
        }

        // ── Preview rendering ─────────────────────────────────────────────────
        if let Some(ref mut win) = preview {
            // Map state to bbox color
            let bbox_color = if result.face_box.is_some() {
                match result.state {
                    AuthState::Waiting => {
                        // During calibration show yellow, else default white/red
                        COLOR_CALIB
                    }
                    AuthState::Recognized => match result.active_tier {
                        Some(ActiveTier::Golden)    => COLOR_TIER1, // Green
                        Some(ActiveTier::Standard)  => COLOR_TIER2, // Cyan
                        Some(ActiveTier::TwoFactor) => COLOR_TIER3, // Orange
                        None                        => COLOR_TIER4,
                    },
                    AuthState::Success    => COLOR_TIER1, // Green
                    AuthState::Failure    => COLOR_TIER4, // Red
                    AuthState::Require2FA => COLOR_TIER3, // Orange
                }
            } else {
                COLOR_TIER4
            };

            let colored_bboxes: Vec<([f32; 4], u32)> = result
                .face_box
                .iter()
                .map(|b| (*b, bbox_color))
                .collect();

            if !win.draw_frame_colored(frame, &colored_bboxes) {
                println!("[Preview] Window closed or ESC pressed.");
                break;
            }
        }

        // ── Debug frame saving ────────────────────────────────────────────────
        if args.save_debug_frames && frame_count % 20 == 0 {
            let path = format!("/tmp/sentinel_debug/auth_{:05}.jpg", frame_count);
            let _ = frame.save(&path);
        }

        // ── Terminal handling ─────────────────────────────────────────────────
        match result.state {
            AuthState::Success => {
                println!("\n✓ ACCESS GRANTED");
                if let Some(dist) = result.distance {
                    let conf = ((1.0 - dist.min(1.0)) * 100.0) as u32;
                    println!("  Confidence: {}%  Distance: {:.4}", conf, dist);
                }
                thread::sleep(Duration::from_millis(1500));
                break;
            }
            AuthState::Failure => {
                eprintln!("\n✗ ACCESS DENIED");
                eprintln!("  Reason: {}", result.message);
                thread::sleep(Duration::from_millis(1500));
                capture.stop();
                std::process::exit(1);
            }
            AuthState::Require2FA => {
                println!("\n⚡ 2FA REQUIRED");
                println!("  Biometrics passed but 2FA is mandatory (Tier 3 match).");
                println!("  Exit code 2 — caller should prompt for 2nd factor.");
                thread::sleep(Duration::from_millis(1500));
                capture.stop();
                std::process::exit(2);
            }
            _ => {}
        }

        thread::sleep(Duration::from_millis(33)); // ~30 fps polling
    }

    capture.stop();
    println!("Session ended.");
    Ok(())
}
