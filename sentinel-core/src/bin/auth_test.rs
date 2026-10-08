/// auth-test binary — runs the same SentinelAuthenticator the daemon uses,
/// without DBus or PAM:
///   Waiting -> Success / Failure / Spoof / NoFace / Timeout
///
/// Usage:
///   sudo cargo run --bin auth-test -- --user testuser [--preview] [--save-debug-frames]

use anyhow::Result;
use clap::Parser;
use sentinel_core::config::SentinelConfig;
use sentinel_core::gallery::GalleryStore;
use sentinel_core::pipeline::{
    AuthState, AuthTier, DebugPreviewWindow, FrameCapture, Models, SentinelAuthenticator,
    COLOR_CALIB, COLOR_TIER1, COLOR_TIER2, COLOR_TIER3, COLOR_TIER4,
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
    let core_gallery = store.load_core()?;
    let adaptive_gallery = store.load_adaptive().unwrap_or_default();

    if core_gallery.is_empty() {
        eprintln!(
            "Error: No enrollment vectors for '{}'. Run enroll-test first.",
            args.user
        );
        std::process::exit(1);
    }
    println!(
        "Loaded {} enrolled + {} learned vectors for '{}'.",
        core_gallery.len(),
        adaptive_gallery.len(),
        args.user
    );

    // ── Load config ──────────────────────────────────────────────────────────
    let config = SentinelConfig::load(&args.config).unwrap_or_else(|e| {
        eprintln!("Warning: {:#} — using defaults.", e);
        SentinelConfig::default()
    });

    // ── Load models ──────────────────────────────────────────────────────────
    let mut models = Models::load(&PathBuf::from(&args.models_dir), &config.detection)?;
    if models.spoof.is_none() {
        eprintln!("Error: anti-spoof model missing — authentication cannot grant access.");
        std::process::exit(1);
    }

    // ── Build authenticator ──────────────────────────────────────────────────
    let mut auth = SentinelAuthenticator::new(
        core_gallery,
        adaptive_gallery,
        args.user.clone(),
        config.clone(),
    );

    // ── Camera ───────────────────────────────────────────────────────────────
    let source = if args.device != 0 {
        format!("/dev/video{}", args.device)
    } else {
        config.camera.source.clone()
    };
    let mut capture =
        FrameCapture::with_format(&source, config.camera.width, config.camera.height, config.camera.fps)?;
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
    let mut last_seq = 0u64;
    let mut last_message = String::new();

    println!("Authentication started. Look at the camera...\n");

    // ── Main loop ─────────────────────────────────────────────────────────────
    loop {
        frame_count += 1;

        // Wait for the next new frame (each frame is processed once)
        if capture.failed() {
            eprintln!("Camera error — see log output.");
            std::process::exit(1);
        }
        let captured = match capture.wait_new_frame(last_seq, Duration::from_millis(100)) {
            Some(f) => f,
            None => {
                if auth.expire_if_overdue() {
                    eprintln!("\n✗ TIMED OUT (camera delivered no usable frames)");
                    capture.stop();
                    std::process::exit(1);
                }
                continue;
            }
        };
        last_seq = captured.seq;

        let frame = &captured.image;

        // Process through state machine
        let result = match auth.process_frame(&mut models, &captured) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[Auth] frame skipped: {e:#}");
                continue;
            }
        };

        // Print status only when message changes (avoid spam)
        if result.message != last_message {
            let prefix = match result.state {
                AuthState::Waiting    => "[Waiting]",
                AuthState::Success    => "[SUCCESS]",
                AuthState::Failure    => "[DENIED]",
                AuthState::Spoof      => "[SPOOF]",
                AuthState::NoFace     => "[NO FACE]",
                AuthState::Timeout    => "[TIMEOUT]",
            };
            println!("{} {}", prefix, result.message);
            if let Some(dist) = result.distance {
                println!(
                    "  → Distance: {:.4}  Tier: {:?}  Spoof score: {}",
                    dist,
                    result.active_tier,
                    result.spoof_score.map_or("n/a".to_string(), |s| format!("{:.3}", s))
                );
            }
            last_message = result.message.clone();
        }

        // ── Preview rendering ─────────────────────────────────────────────────
        if let Some(ref mut win) = preview {
            // Map state to bbox color
            let bbox_color = match (&result.state, result.active_tier) {
                (AuthState::Success, _) => COLOR_TIER1,
                (AuthState::Failure | AuthState::Spoof | AuthState::Timeout | AuthState::NoFace, _) => COLOR_TIER4,
                (AuthState::Waiting, Some(AuthTier::Golden)) => COLOR_TIER1,
                (AuthState::Waiting, Some(AuthTier::Standard)) => COLOR_TIER2,
                (AuthState::Waiting, Some(AuthTier::TwoFactor)) => COLOR_TIER3,
                (AuthState::Waiting, Some(AuthTier::Denied)) => COLOR_TIER4,
                (AuthState::Waiting, None) => COLOR_CALIB,
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
            AuthState::Failure | AuthState::Spoof | AuthState::Timeout | AuthState::NoFace => {
                eprintln!("\n✗ NOT GRANTED ({:?})", result.state);
                eprintln!("  Reason: {}", result.message);
                thread::sleep(Duration::from_millis(1500));
                capture.stop();
                std::process::exit(1);
            }
            AuthState::Waiting => {}
        }
    }

    capture.stop();
    println!("Session ended.");
    Ok(())
}
