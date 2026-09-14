use anyhow::{Context, Result};
use clap::Parser;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use sentinel_core::pipeline::{
    align_face, match_gallery, FrameCapture, MobileFaceNet, ScrfdDetector, SpoofDetector,
};

#[derive(Parser, Debug)]
#[command(author, version, about = "Sentinel FRS Pipeline Latency Benchmark")]
struct Args {
    /// Directory containing .jpg/.jpeg/.png debug frames
    #[arg(long, default_value = "/tmp/sentinel_debug")]
    frames_dir: String,

    /// Camera device node if live capture is needed
    #[arg(long, default_value = "/dev/video0")]
    camera: String,

    /// SCRFD input resolution (320 or 640)
    #[arg(long, default_value_t = 320)]
    input_size: u32,
}

struct FrameTimings {
    file_name: String,
    scrfd_ms: f64,
    align_ms: f64,
    embed_ms: f64,
    match_ms: f64,
    spoof_ms: f64,
    total_ms: f64,
}

fn calculate_p95(mut values: Vec<f64>) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((values.len() as f64) * 0.95).ceil() as usize - 1;
    let clamped_idx = idx.min(values.len() - 1);
    values[clamped_idx]
}

fn calculate_stats(values: &[f64]) -> (f64, f64, f64, f64) {
    if values.is_empty() {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let min_val = values.iter().cloned().fold(f64::INFINITY, f64::min);
    let max_val = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let sum_val: f64 = values.iter().sum();
    let mean_val = sum_val / (values.len() as f64);
    let p95_val = calculate_p95(values.to_vec());
    (min_val, mean_val, max_val, p95_val)
}

fn main() -> Result<()> {
    env_logger::init();
    let args = Args::parse();
    let frames_dir = PathBuf::from(&args.frames_dir);

    println!("=== Sentinel Pipeline Latency Benchmark ===");
    println!("Target Frames Directory: {}", frames_dir.display());

    // 1. Ensure frames directory has at least 10 frames, else capture 30 fresh frames
    if !frames_dir.exists() {
        fs::create_dir_all(&frames_dir)?;
    }

    let mut frame_paths: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = fs::read_dir(&frames_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                let lower = ext.to_lowercase();
                if lower == "jpg" || lower == "jpeg" || lower == "png" {
                    frame_paths.push(path);
                }
            }
        }
    }
    frame_paths.sort();

    if frame_paths.len() < 10 {
        println!(
            "[bench-pipeline] Found {} existing frames (< 10). Capturing 30 fresh frames from camera ({}) ...",
            frame_paths.len(),
            args.camera
        );

        let mut capture = FrameCapture::new(&args.camera)
            .context("Failed to initialize FrameCapture")?;
        capture.start().context("Failed to start camera capture")?;

        let mut captured_count = 0usize;
        let start_wait = Instant::now();

        while captured_count < 30 && start_wait.elapsed() < Duration::from_secs(10) {
            if let Some(cap) = capture.read_captured_frame() {
                if cap.luma >= 15.0 {
                    let out_file = frames_dir.join(format!("bench_frame_{:04}.jpg", captured_count));
                    if cap.image.save(&out_file).is_ok() {
                        frame_paths.push(out_file);
                        captured_count += 1;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        capture.stop();
        frame_paths.sort();
        println!("[bench-pipeline] Captured {} frames into {}", captured_count, frames_dir.display());
    }

    if frame_paths.is_empty() {
        anyhow::bail!("No frames available in {} even after capture attempt.", frames_dir.display());
    }

    println!("[bench-pipeline] Benchmarking across {} frames...", frame_paths.len());

    // 2. Initialize Models
    let candidate_dirs = [
        PathBuf::from("/var/cache/sentinel/models"),
        PathBuf::from("/tmp/sentinel_models"),
        PathBuf::from("models"),
    ];

    let model_dir = candidate_dirs
        .iter()
        .find(|d| d.join("scrfd_500m_kps.onnx").exists() && d.join("mobile_facenet.onnx").exists())
        .cloned()
        .unwrap_or_else(|| PathBuf::from("/var/cache/sentinel/models"));

    let scrfd_path = model_dir.join("scrfd_500m_kps.onnx");
    let mfn_path = model_dir.join("mobile_facenet.onnx");
    let minifas_path = model_dir.join("MiniFASNetV2.onnx");

    if !scrfd_path.exists() || !mfn_path.exists() {
        anyhow::bail!(
            "Models missing in {}. Please ensure scrfd_500m_kps.onnx and mobile_facenet.onnx are present.",
            model_dir.display()
        );
    }
    println!("[bench-pipeline] Using ONNX models from: {}", model_dir.display());

    let mut detector = ScrfdDetector::new_with_input_size(
        scrfd_path.to_str().unwrap(),
        0.50,
        0.30,
        60,
        args.input_size,
    ).context("Failed to load SCRFD detector")?;

    let mut embedder = MobileFaceNet::new(
        mfn_path.to_str().unwrap()
    ).context("Failed to load MobileFaceNet embedder")?;

    let mut spoof_detector = if minifas_path.exists() {
        SpoofDetector::new(
            minifas_path.to_str().unwrap(),
            "/var/lib/sentinel/minifas_calib.json",
            0.85,
        ).ok()
    } else {
        None
    };

    // 3. Create synthetic 30-vector gallery (each 512-d unit vector)
    let mut gallery: Vec<[f32; 512]> = Vec::with_capacity(30);
    for i in 0..30 {
        let mut v = [0.0f32; 512];
        v[i % 512] = 1.0;
        gallery.push(v);
    }

    // Warmup step (run 1 frame to ensure ORN thread pools / execution graphs initialized)
    if let Ok(warmup_img) = image::open(&frame_paths[0]) {
        let rgb = warmup_img.to_rgb8();
        let _ = detector.detect_detailed(&rgb);
    }

    let mut results: Vec<FrameTimings> = Vec::new();

    // Default fallback landmarks/bbox if no face detected in frame
    let default_landmarks = [
        [200.0, 180.0],
        [350.0, 180.0],
        [275.0, 250.0],
        [220.0, 320.0],
        [330.0, 320.0],
    ];
    let default_bbox = [150.0, 120.0, 400.0, 380.0];

    for path in &frame_paths {
        let file_name = path.file_name().unwrap().to_string_lossy().to_string();
        let frame_img = match image::open(path) {
            Ok(img) => img.to_rgb8(),
            Err(_) => continue,
        };

        // Stage 1: SCRFD Detection
        let t_scrfd_start = Instant::now();
        let det_res = detector.detect_detailed(&frame_img)?;
        let scrfd_ms = t_scrfd_start.elapsed().as_secs_f64() * 1000.0;

        let (landmarks, bbox) = if !det_res.detections.is_empty() {
            (det_res.detections[0].landmarks, det_res.detections[0].bbox)
        } else {
            (default_landmarks, default_bbox)
        };

        // Stage 2: Affine Alignment
        let t_align_start = Instant::now();
        let aligned = align_face(&frame_img, &landmarks)?;
        let align_ms = t_align_start.elapsed().as_secs_f64() * 1000.0;

        // Stage 3: MobileFaceNet Embedding
        let t_embed_start = Instant::now();
        let embedding = embedder.embed(&aligned)?;
        let embed_ms = t_embed_start.elapsed().as_secs_f64() * 1000.0;

        // Stage 4: Cosine match against 30-vector gallery
        let t_match_start = Instant::now();
        let (_dist, _tier) = match_gallery(&embedding, &gallery);
        let match_ms = t_match_start.elapsed().as_secs_f64() * 1000.0;

        // Stage 5: MiniFASNet Spoof Check
        let spoof_ms = if let Some(ref mut sd) = spoof_detector {
            let t_spoof_start = Instant::now();
            if let Ok(crop) = SpoofDetector::square_crop(&frame_img, bbox, 2.7) {
                let _ = sd.predict(&crop);
            }
            t_spoof_start.elapsed().as_secs_f64() * 1000.0
        } else {
            0.0
        };

        let total_ms = scrfd_ms + align_ms + embed_ms + match_ms + spoof_ms;

        results.push(FrameTimings {
            file_name,
            scrfd_ms,
            align_ms,
            embed_ms,
            match_ms,
            spoof_ms,
            total_ms,
        });
    }

    // 4. Print Detailed Benchmark Table & Summary Statistics
    let scrfd_vals: Vec<f64> = results.iter().map(|r| r.scrfd_ms).collect();
    let align_vals: Vec<f64> = results.iter().map(|r| r.align_ms).collect();
    let embed_vals: Vec<f64> = results.iter().map(|r| r.embed_ms).collect();
    let match_vals: Vec<f64> = results.iter().map(|r| r.match_ms).collect();
    let spoof_vals: Vec<f64> = results.iter().map(|r| r.spoof_ms).collect();
    let total_vals: Vec<f64> = results.iter().map(|r| r.total_ms).collect();

    let (s_min, s_mean, s_max, s_p95) = calculate_stats(&scrfd_vals);
    let (a_min, a_mean, a_max, a_p95) = calculate_stats(&align_vals);
    let (e_min, e_mean, e_max, e_p95) = calculate_stats(&embed_vals);
    let (m_min, m_mean, m_max, m_p95) = calculate_stats(&match_vals);
    let (p_min, p_mean, p_max, p_p95) = calculate_stats(&spoof_vals);
    let (t_min, t_mean, t_max, t_p95) = calculate_stats(&total_vals);

    println!("\n{:<32} {:>8} {:>8} {:>8} {:>8} {:>10}", "Pipeline Stage", "Min(ms)", "Mean(ms)", "Max(ms)", "P95(ms)", "Target Spec");
    println!("{}", "-".repeat(82));
    println!("{:<32} {:>8.2} {:>8.2} {:>8.2} {:>8.2} {:>10}", "1. SCRFD Detection", s_min, s_mean, s_max, s_p95, "< 12.0 ms");
    println!("{:<32} {:>8.2} {:>8.2} {:>8.2} {:>8.2} {:>10}", "2. Affine Alignment", a_min, a_mean, a_max, a_p95, "< 1.0 ms");
    println!("{:<32} {:>8.2} {:>8.2} {:>8.2} {:>8.2} {:>10}", "3. MobileFaceNet Embedding", e_min, e_mean, e_max, e_p95, "< 15.0 ms");
    println!("{:<32} {:>8.2} {:>8.2} {:>8.2} {:>8.2} {:>10}", "4. Cosine Match (30 vectors)", m_min, m_mean, m_max, m_p95, "< 1.0 ms");
    println!("{:<32} {:>8.2} {:>8.2} {:>8.2} {:>8.2} {:>10}", "5. MiniFASNet Spoof Check", p_min, p_mean, p_max, p_p95, "< 10.0 ms");
    println!("{}", "-".repeat(82));
    println!("{:<32} {:>8.2} {:>8.2} {:>8.2} {:>8.2} {:>10}", "TOTAL PIPELINE", t_min, t_mean, t_max, t_p95, "< 43.0 ms");

    println!("\nOver Budget Analysis (> 100ms):");
    let over_budget: Vec<&FrameTimings> = results.iter().filter(|r| r.total_ms > 100.0).collect();
    if over_budget.is_empty() {
        println!("  None! All {} frames processed under 100ms budget.", results.len());
    } else {
        for ob in over_budget {
            println!("  [OVER BUDGET] {} => {:.2} ms", ob.file_name, ob.total_ms);
        }
    }

    let est_fps = if t_mean > 0.0 { 1000.0 / t_mean } else { 0.0 };
    println!("\nEstimated Pipeline Throughput: {:.2} FPS", est_fps);

    if t_mean <= 43.0 && t_p95 <= 100.0 {
        println!("\n>>> RESULT: PASS (Mean total {:.2}ms <= 43ms, P95 {:.2}ms <= 100ms) <<<", t_mean, t_p95);
    } else {
        println!("\n>>> RESULT: WARNING / EXCEEDS BUDGET (Mean total {:.2}ms, P95 {:.2}ms) <<<", t_mean, t_p95);
    }

    Ok(())
}
