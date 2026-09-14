use anyhow::{anyhow, Result};
use gstreamer as gst;
use gstreamer_app as gst_app;
use gst::prelude::*;
use image::RgbImage;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct CapturedFrame {
    pub image: RgbImage,
    pub luma: f64,
    pub timestamp: Instant,
}

/// BT.601 luma mean for dark-frame detection. Frame is RGB.
pub fn bt601_luma_mean(frame: &RgbImage) -> f64 {
    let pixels = frame.pixels().count() as f64;
    if pixels < 1.0 {
        return 0.0;
    }
    let mut sum = 0.0f64;
    for pixel in frame.pixels() {
        let r = pixel[0] as f64;
        let g = pixel[1] as f64;
        let b = pixel[2] as f64;
        sum += 0.299 * r + 0.587 * g + 0.114 * b;
    }
    sum / pixels
}

pub fn apply_clahe(frame: &RgbImage) -> Result<RgbImage> {
    Ok(frame.clone())
}

struct FrameBuffer {
    captured: Option<CapturedFrame>,
}

pub struct FrameCapture {
    source: String,
    running: Arc<AtomicBool>,
    buffer: Arc<Mutex<FrameBuffer>>,
    handle: Option<JoinHandle<()>>,
}

impl FrameCapture {
    pub fn new(source: &str) -> Result<Self> {
        let src = if source.trim().chars().all(|c| c.is_ascii_digit()) {
            format!("/dev/video{}", source.trim())
        } else {
            source.to_string()
        };

        Ok(Self {
            source: src,
            running: Arc::new(AtomicBool::new(false)),
            buffer: Arc::new(Mutex::new(FrameBuffer { captured: None })),
            handle: None,
        })
    }

    pub fn start(&mut self) -> Result<()> {
        if self.running.load(Ordering::SeqCst) {
            return Ok(());
        }

        gst::init().map_err(|e| anyhow!("Failed to initialize GStreamer: {}", e))?;

        let pipeline_str = if self.source == "pipewiresrc" {
            "pipewiresrc ! videoconvert ! video/x-raw,format=RGB ! appsink name=sink drop=true max-buffers=1".to_string()
        } else if self.source.starts_with("/dev/video") {
            format!("v4l2src device={} ! videoconvert ! video/x-raw,format=RGB ! appsink name=sink drop=true max-buffers=1", self.source)
        } else {
            format!("v4l2src device={} ! videoconvert ! video/x-raw,format=RGB ! appsink name=sink drop=true max-buffers=1", self.source)
        };

        println!("[FrameCapture] Building GStreamer pipeline: {}", pipeline_str);

        let pipeline = gst::parse::launch(&pipeline_str)
            .map_err(|e| anyhow!("Failed to parse GStreamer pipeline '{}': {}", pipeline_str, e))?;

        let pipeline = pipeline
            .dynamic_cast::<gst::Pipeline>()
            .map_err(|_| anyhow!("Failed to cast to gst::Pipeline"))?;

        let sink_element = pipeline
            .by_name("sink")
            .ok_or_else(|| anyhow!("Failed to find 'sink' element in pipeline"))?;

        let appsink = sink_element
            .dynamic_cast::<gst_app::AppSink>()
            .map_err(|_| anyhow!("Failed to cast element to AppSink"))?;

        pipeline
            .set_state(gst::State::Playing)
            .map_err(|e| anyhow!("Failed to set pipeline state to Playing: {}", e))?;

        self.running.store(true, Ordering::SeqCst);
        let running = Arc::clone(&self.running);
        let buffer = Arc::clone(&self.buffer);
        let source_name = self.source.clone();

        let handle = thread::spawn(move || {
            let mut frame_count = 0u64;
            let mut last_log = Instant::now();

            while running.load(Ordering::SeqCst) {
                match appsink.try_pull_sample(gst::ClockTime::from_mseconds(50)) {
                    Some(sample) => {
                        if let Some(buf) = sample.buffer() {
                            if let Ok(map) = buf.map_readable() {
                                if let Some(caps) = sample.caps() {
                                    if let Some(structure) = caps.structure(0) {
                                        let width = structure.get::<i32>("width").unwrap_or(640) as u32;
                                        let height = structure.get::<i32>("height").unwrap_or(480) as u32;
                                        let bytes = map.as_slice();
                                        if bytes.len() == (width * height * 3) as usize {
                                            if let Some(mut rgb) = RgbImage::from_raw(width, height, bytes.to_vec()) {
                                                const MAX_WIDTH: u32 = 640;
                                                const MAX_HEIGHT: u32 = 480;
                                                if rgb.width() > MAX_WIDTH || rgb.height() > MAX_HEIGHT {
                                                    rgb = image::imageops::resize(
                                                        &rgb, MAX_WIDTH, MAX_HEIGHT,
                                                        image::imageops::FilterType::Triangle
                                                    );
                                                }
                                                let luma = bt601_luma_mean(&rgb);
                                                if let Ok(mut lock) = buffer.lock() {
                                                    lock.captured = Some(CapturedFrame {
                                                        image: rgb,
                                                        luma,
                                                        timestamp: Instant::now(),
                                                    });
                                                    frame_count += 1;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    None => {
                        if last_log.elapsed() > Duration::from_secs(5) {
                            eprintln!("[FrameCapture] Waiting for GStreamer frames on {}...", source_name);
                            last_log = Instant::now();
                        }
                    }
                }
            }

            let _ = pipeline.set_state(gst::State::Null);
            println!("[FrameCapture] GStreamer pipeline stopped. Total frames captured: {}", frame_count);
        });

        self.handle = Some(handle);
        Ok(())
    }

    pub fn read_captured_frame(&self) -> Option<CapturedFrame> {
        let lock = self.buffer.lock().ok()?;
        let cap = lock.captured.clone()?;
        if cap.timestamp.elapsed() > Duration::from_millis(500) {
            return None;
        }
        Some(cap)
    }

    pub fn read_frame(&self) -> Option<RgbImage> {
        self.read_captured_frame().map(|f| f.image)
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for FrameCapture {
    fn drop(&mut self) {
        self.stop();
    }
}
