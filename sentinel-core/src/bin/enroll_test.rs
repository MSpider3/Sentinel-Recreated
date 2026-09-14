use anyhow::Result;
use clap::Parser;
use image::RgbImage;
use sentinel_core::config::SentinelConfig;
use sentinel_core::gallery::GalleryStore;
use sentinel_core::pipeline::{
    align_face, DebugPreviewWindow, FaceDetection, FrameCapture, MobileFaceNet, RawCandidate, ScrfdDetector,
};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Parser, Debug)]
#[command(author, version, about = "Sentinel Recreated — Standalone Face Enrollment Test Binary", long_about = None)]
struct Args {
    #[arg(short, long)]
    user: String,

    #[arg(short, long, default_value = "/etc/sentinel/config.toml")]
    config: String,

    #[arg(short, long, default_value = "/var/cache/sentinel/models")]
    models_dir: String,

    #[arg(short, long)]
    glasses: bool,

    #[arg(short, long, default_value_t = 0)]
    device: u32,

    #[arg(short, long)]
    preview: bool,

    #[arg(long)]
    save_debug_frames: bool,
}

struct PoseConfig {
    name: &'static str,
    instruction: &'static str,
}

fn main() -> Result<()> {
    env_logger::init();
    let args = Args::parse();

    println!("=== Sentinel Recreated: Standalone Face Enrollment Test ===");
    println!("Target User: {}", args.user);
    if args.preview {
        println!("Debug preview window enabled (minifb 640x480).");
    }
    if args.save_debug_frames {
        println!("Debug frame saving enabled -> /tmp/sentinel_debug/");
        let _ = std::fs::create_dir_all("/tmp/sentinel_debug");
    }

    let config = SentinelConfig::load(&args.config).unwrap_or_default();
    let models_path = PathBuf::from(&args.models_dir);

    let scrfd_model = models_path.join("scrfd_500m_kps.onnx");
    let mobilefacenet_model = models_path.join("mobile_facenet.onnx");

    if !scrfd_model.exists() || !mobilefacenet_model.exists() {
        eprintln!(
            "Error: Model files not found in {}. Please run scripts/download_models.sh first.",
            models_path.display()
        );
        std::process::exit(1);
    }

    let mut detector = ScrfdDetector::new(
        scrfd_model.to_str().unwrap(),
        config.detection.score_threshold,
        config.detection.nms_threshold,
        config.detection.min_face_size_px,
    )?;

    let mut embedder = MobileFaceNet::new(mobilefacenet_model.to_str().unwrap())?;

    let source = if args.device != 0 {
        format!("/dev/video{}", args.device)
    } else {
        config.camera.source.clone()
    };
    println!("Opening camera source {}...", source);
    let mut capture = FrameCapture::new(&source)?;
    capture.start()?;

    let mut preview_window = if args.preview {
        Some(DebugPreviewWindow::new("Sentinel Debug Preview", 640, 480)?)
    } else {
        None
    };

    let base_poses = vec![
        PoseConfig { name: "Center", instruction: "Look directly at the camera" },
        PoseConfig { name: "Left", instruction: "Slowly turn your head to the LEFT" },
        PoseConfig { name: "Right", instruction: "Slowly turn your head to the RIGHT" },
        PoseConfig { name: "Up", instruction: "Slowly tilt your head UP" },
        PoseConfig { name: "Down", instruction: "Slowly tilt your head DOWN" },
    ];

    let passes = if args.glasses { 2 } else { 1 };
    let mut collected_embeddings = Vec::<[f32; 512]>::new();
    let mut frame_count = 0u64;

    for pass in 0..passes {
        let pass_label = if args.glasses {
            if pass == 0 { " (With Glasses)" } else { " (Without Glasses)" }
        } else {
            ""
        };

        if pass == 1 {
            println!("\n=======================================================");
            println!("[PASSTHROUGH] Please REMOVE your glasses and press ENTER to continue...");
            println!("=======================================================");
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
        }

        for (pose_idx, pose) in base_poses.iter().enumerate() {
            let full_pose_name = format!("{}{}", pose.name, pass_label);

            println!("\n=======================================================");
            println!(
                "Pose {}/{}: [{}]",
                pose_idx + 1 + (pass * base_poses.len()),
                base_poses.len() * passes,
                full_pose_name
            );
            println!("Instruction: {}", pose.instruction);
            println!("Get ready... Position your face in camera view.");
            println!("=======================================================");

            // STATE 1: INSTRUCT (Pause 2 seconds for user to read instruction)
            let instruct_start = Instant::now();
            while instruct_start.elapsed() < Duration::from_millis(2000) {
                frame_count += 1;
                if let Some(captured) = capture.read_captured_frame() {
                    let frame = &captured.image;
                    if let Some(ref mut win) = preview_window {
                        let _ = win.draw_frame(frame, &[]);
                    }
                }
                thread::sleep(Duration::from_millis(30));
            }

            // STATE 2: DETECTING (Enforce single face & steady hold)
            let mut pose_samples = Vec::<[f32; 512]>::new();
            let mut last_status_print = Instant::now();

            while pose_samples.len() < 3 {
                frame_count += 1;

                if let Some(captured) = capture.read_captured_frame() {
                    let frame = &captured.image;
                    let luma = captured.luma;

                    if luma < 15.0 {
                        if last_status_print.elapsed() > Duration::from_millis(1000) {
                            println!(
                                "[Frame {}] luma={:.1} — Dark frame skipped (luma={:.1} < 15.0 threshold)",
                                frame_count, luma, luma
                            );
                            last_status_print = Instant::now();
                        }
                        if let Some(ref mut win) = preview_window {
                            if !win.draw_frame(frame, &[]) {
                                println!("[Preview] Window closed or ESC pressed.");
                                capture.stop();
                                return Ok(());
                            }
                        }
                        handle_save_debug_frames(args.save_debug_frames, frame_count, frame, &[], &[], true);
                        thread::sleep(Duration::from_millis(10));
                        continue;
                    }

                    let res = detector.detect_detailed(frame)?;

                    // Draw ONLY valid NMS detections in preview & debug frames
                    let detection_bboxes: Vec<[f32; 4]> = res.detections.iter().map(|c| c.bbox).collect();

                    if let Some(ref mut win) = preview_window {
                        if !win.draw_frame(frame, &detection_bboxes) {
                            println!("[Preview] Window closed or ESC pressed.");
                            capture.stop();
                            return Ok(());
                        }
                    }

                    // Enforce Single Face Constraint
                    if res.detections.len() == 1 {
                        let det = &res.detections[0];
                        let bw = (det.bbox[2] - det.bbox[0]).max(0.0);

                        println!(
                            "[Frame {}] luma={:.1} — Face accepted (score={:.2}, bbox={:.0}px) — capturing sub-sample {}/3 for [{}]",
                            frame_count, luma, det.score, bw, pose_samples.len() + 1, full_pose_name
                        );

                        handle_save_debug_frames(
                            args.save_debug_frames,
                            frame_count,
                            frame,
                            &res.detections,
                            &res.raw_candidates,
                            false,
                        );

                        if let Ok(aligned) = align_face(frame, &det.landmarks) {
                            if let Ok(emb) = embedder.embed(&aligned) {
                                pose_samples.push(emb);
                                thread::sleep(Duration::from_millis(300));
                            }
                        }
                    } else if res.detections.len() > 1 {
                        if last_status_print.elapsed() > Duration::from_millis(1000) {
                            println!(
                                "[Frame {}] luma={:.1} — Multiple faces detected ({} faces) — please ensure ONLY ONE person is in camera view",
                                frame_count, luma, res.detections.len()
                            );
                            last_status_print = Instant::now();
                        }
                        handle_save_debug_frames(args.save_debug_frames, frame_count, frame, &[], &res.raw_candidates, false);
                    } else {
                        if last_status_print.elapsed() > Duration::from_millis(1000) {
                            if res.raw_candidates.is_empty() {
                                println!(
                                    "[Frame {}] luma={:.1} — No face detected by SCRFD",
                                    frame_count, luma
                                );
                            } else {
                                let top = &res.raw_candidates[0];
                                let min_dim = top.bw.min(top.bh);
                                if top.score < config.detection.score_threshold {
                                    println!(
                                        "[Frame {}] luma={:.1} — Face detected but filtered (score={:.2} < {:.2} threshold, bbox={:.0}px)",
                                        frame_count, luma, top.score, config.detection.score_threshold, min_dim
                                    );
                                } else if min_dim < config.detection.min_face_size_px as f32 {
                                    println!(
                                        "[Frame {}] luma={:.1} — Face detected but filtered (score={:.2}, bbox={:.0}px < {}px threshold)",
                                        frame_count, luma, top.score, min_dim, config.detection.min_face_size_px
                                    );
                                }
                            }
                            last_status_print = Instant::now();
                        }
                        handle_save_debug_frames(args.save_debug_frames, frame_count, frame, &[], &res.raw_candidates, false);
                    }
                } else {
                    if last_status_print.elapsed() > Duration::from_millis(1000) {
                        println!("[Frame {}] Camera Waiting — No frame stream from camera device", frame_count);
                        last_status_print = Instant::now();
                    }
                    thread::sleep(Duration::from_millis(10));
                }
            }

            collected_embeddings.extend(pose_samples);

            // STATE 3: SUCCESS (Show pose complete notice & brief pause)
            println!("\n[SUCCESS] Pose [{}] captured successfully!", full_pose_name);
            let success_start = Instant::now();
            while success_start.elapsed() < Duration::from_millis(1500) {
                frame_count += 1;
                if let Some(captured) = capture.read_captured_frame() {
                    let frame = &captured.image;
                    if let Some(ref mut win) = preview_window {
                        let _ = win.draw_frame(frame, &[]);
                    }
                }
                thread::sleep(Duration::from_millis(30));
            }
        }
    }

    capture.stop();

    println!("\nSaving embeddings to gallery...");
    let store = GalleryStore::new(&args.user);
    store.save_core(&collected_embeddings)?;

    println!(
        "\nSUCCESS: Enrolled {} vectors for user '{}'",
        collected_embeddings.len(),
        args.user
    );

    Ok(())
}

fn handle_save_debug_frames(
    save_debug: bool,
    frame_count: u64,
    frame: &RgbImage,
    detections: &[FaceDetection],
    _raw_candidates: &[RawCandidate],
    is_dark: bool,
) {
    if !save_debug {
        return;
    }

    let dir = std::path::Path::new("/tmp/sentinel_debug");

    if frame_count % 10 == 0 {
        let path = dir.join(format!("frame_{:04}.jpg", frame_count));
        if frame.save(&path).is_ok() {
            println!("[Debug] Saved frame to {}", path.display());
        }
    }

    if !detections.is_empty() {
        let det = &detections[0];
        let x = det.bbox[0] as i32;
        let y = det.bbox[1] as i32;
        let w = (det.bbox[2] - det.bbox[0]) as i32;
        let h = (det.bbox[3] - det.bbox[1]) as i32;
        let path = dir.join(format!(
            "detection_{:04}_bbox_{}_{}_{}_{}.jpg",
            frame_count, x, y, w, h
        ));

        let mut annotated = frame.clone();
        draw_red_bbox(&mut annotated, &det.bbox);
        if annotated.save(&path).is_ok() {
            println!("[Debug] Saved detection frame to {}", path.display());
        }
    } else if !is_dark {
        let path = dir.join(format!("noface_{:04}.jpg", frame_count));
        if frame.save(&path).is_ok() {
            println!("[Debug] Saved frame to {}", path.display());
        }
    }
}

fn draw_red_bbox(img: &mut RgbImage, bbox: &[f32; 4]) {
    let w = img.width() as i32;
    let h = img.height() as i32;
    let x1 = (bbox[0] as i32).clamp(0, w - 1);
    let y1 = (bbox[1] as i32).clamp(0, h - 1);
    let x2 = (bbox[2] as i32).clamp(0, w - 1);
    let y2 = (bbox[3] as i32).clamp(0, h - 1);
    let thickness = 3;

    for t in 0..thickness {
        for x in x1..=x2 {
            if y1 + t < h {
                img.put_pixel(x as u32, (y1 + t) as u32, image::Rgb([255, 0, 0]));
            }
            if y2 - t >= 0 && y2 - t < h {
                img.put_pixel(x as u32, (y2 - t) as u32, image::Rgb([255, 0, 0]));
            }
        }
        for y in y1..=y2 {
            if x1 + t < w {
                img.put_pixel((x1 + t) as u32, y as u32, image::Rgb([255, 0, 0]));
            }
            if x2 - t >= 0 && x2 - t < w {
                img.put_pixel((x2 - t) as u32, y as u32, image::Rgb([255, 0, 0]));
            }
        }
    }
}
