use anyhow::{anyhow, bail, Result};
use gstreamer as gst;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use gst::prelude::*;
use gst_video::prelude::*;
use image::RgbImage;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Condvar, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct CapturedFrame {
    pub image: RgbImage,
    pub luma: f64,
    pub timestamp: Instant,
    /// Counts up by one for every frame the camera delivers, so a consumer
    /// can tell a new frame from a second read of the same one.
    pub seq: u64,
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

/// The camera source ends up inside a GStreamer pipeline description, so only
/// a plain V4L2 device node (or the PipeWire source) is accepted.
pub fn is_valid_source(source: &str) -> bool {
    source == "pipewiresrc"
        || source.strip_prefix("/dev/video").map_or(false, |n| {
            !n.is_empty() && n.len() <= 3 && n.bytes().all(|b| b.is_ascii_digit())
        })
}

struct Shared {
    frame: Mutex<Option<CapturedFrame>>,
    new_frame: Condvar,
}

pub struct FrameCapture {
    source: String,
    width: u32,
    height: u32,
    fps: u32,
    running: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
    shared: Arc<Shared>,
    handle: Option<JoinHandle<()>>,
}

fn build_pipeline(source: &str, format_caps: Option<&str>) -> Result<(gst::Pipeline, gst_app::AppSink)> {
    let src = if source == "pipewiresrc" {
        "pipewiresrc".to_string()
    } else {
        format!("v4l2src device={}", source)
    };
    let caps = format_caps.map(|c| format!(" ! {}", c)).unwrap_or_default();
    let pipeline_str = format!(
        "{}{} ! videoconvert ! video/x-raw,format=RGB ! appsink name=sink drop=true max-buffers=1",
        src, caps
    );
    log::debug!("[FrameCapture] Building GStreamer pipeline: {}", pipeline_str);

    let pipeline = gst::parse::launch(&pipeline_str)
        .map_err(|e| anyhow!("Failed to parse GStreamer pipeline '{}': {}", pipeline_str, e))?
        .dynamic_cast::<gst::Pipeline>()
        .map_err(|_| anyhow!("Failed to cast to gst::Pipeline"))?;

    let appsink = pipeline
        .by_name("sink")
        .ok_or_else(|| anyhow!("Failed to find 'sink' element in pipeline"))?
        .dynamic_cast::<gst_app::AppSink>()
        .map_err(|_| anyhow!("Failed to cast element to AppSink"))?;

    if let Err(e) = pipeline.set_state(gst::State::Playing) {
        let _ = pipeline.set_state(gst::State::Null);
        bail!("Failed to set pipeline state to Playing: {}", e);
    }
    Ok((pipeline, appsink))
}

/// Copy one RGB sample out of GStreamer, honouring the row stride (rows are
/// padded to 4 bytes, so `width * 3` is not always the row length).
fn sample_to_rgb(sample: &gst::Sample) -> Option<RgbImage> {
    let info = gst_video::VideoInfo::from_caps(sample.caps()?).ok()?;
    let frame = gst_video::VideoFrameRef::from_buffer_ref_readable(sample.buffer()?, &info).ok()?;
    let (width, height) = (info.width() as usize, info.height() as usize);
    let stride = frame.plane_stride()[0] as usize;
    let data = frame.plane_data(0).ok()?;

    let row_len = width * 3;
    let mut rgb = Vec::with_capacity(row_len * height);
    for row in 0..height {
        rgb.extend_from_slice(data.get(row * stride..row * stride + row_len)?);
    }
    RgbImage::from_raw(width as u32, height as u32, rgb)
}

/// Bring a frame to the requested size without distorting it: centre-crop to
/// the target aspect ratio first, then scale.
fn fit_frame(rgb: RgbImage, width: u32, height: u32) -> RgbImage {
    if rgb.dimensions() == (width, height) {
        return rgb;
    }
    let (w, h) = rgb.dimensions();
    let crop_w = w.min((h as u64 * width as u64 / height as u64) as u32).max(1);
    let crop_h = h.min((w as u64 * height as u64 / width as u64) as u32).max(1);
    let cropped =
        image::imageops::crop_imm(&rgb, (w - crop_w) / 2, (h - crop_h) / 2, crop_w, crop_h).to_image();
    image::imageops::resize(&cropped, width, height, image::imageops::FilterType::Triangle)
}

impl FrameCapture {
    pub fn new(source: &str) -> Result<Self> {
        Self::with_format(source, 640, 480, 30)
    }

    pub fn with_format(source: &str, width: u32, height: u32, fps: u32) -> Result<Self> {
        let src = if source.trim().chars().all(|c| c.is_ascii_digit()) && !source.trim().is_empty() {
            format!("/dev/video{}", source.trim())
        } else {
            source.to_string()
        };
        if !is_valid_source(&src) {
            bail!("Invalid camera source '{}': expected /dev/videoN or pipewiresrc", src);
        }
        if width == 0 || height == 0 || fps == 0 {
            bail!("Invalid camera format {}x{}@{}", width, height, fps);
        }

        Ok(Self {
            source: src,
            width,
            height,
            fps,
            running: Arc::new(AtomicBool::new(false)),
            failed: Arc::new(AtomicBool::new(false)),
            shared: Arc::new(Shared { frame: Mutex::new(None), new_frame: Condvar::new() }),
            handle: None,
        })
    }

    pub fn start(&mut self) -> Result<()> {
        if self.running.load(Ordering::SeqCst) {
            return Ok(());
        }

        gst::init().map_err(|e| anyhow!("Failed to initialize GStreamer: {}", e))?;

        // Ask the camera for the configured mode. Without this GStreamer picks
        // the camera's largest mode, which on many webcams is slow (10 fps).
        let format_caps = format!(
            "video/x-raw,width={},height={},framerate=[1/1,{}/1]",
            self.width, self.height, self.fps
        );
        let (mut pipeline, mut appsink) = build_pipeline(&self.source, Some(&format_caps))?;

        self.running.store(true, Ordering::SeqCst);
        self.failed.store(false, Ordering::SeqCst);
        let running = Arc::clone(&self.running);
        let failed = Arc::clone(&self.failed);
        let shared = Arc::clone(&self.shared);
        let source = self.source.clone();
        let (width, height) = (self.width, self.height);

        let handle = thread::spawn(move || {
            let mut seq = 0u64;
            let mut tried_any_format = false;

            while running.load(Ordering::SeqCst) {
                if let Some(sample) = appsink.try_pull_sample(gst::ClockTime::from_mseconds(50)) {
                    if let Some(rgb) = sample_to_rgb(&sample) {
                        let image = fit_frame(rgb, width, height);
                        let luma = bt601_luma_mean(&image);
                        seq += 1;
                        if let Ok(mut slot) = shared.frame.lock() {
                            *slot = Some(CapturedFrame { image, luma, timestamp: Instant::now(), seq });
                            shared.new_frame.notify_all();
                        }
                    }
                    continue;
                }

                // No frame: see whether the camera reported an error.
                let error = pipeline
                    .bus()
                    .and_then(|bus| bus.pop_filtered(&[gst::MessageType::Error]));
                if let Some(msg) = error {
                    let _ = pipeline.set_state(gst::State::Null);
                    // The camera may simply not offer the requested mode:
                    // retry once with whatever it does offer.
                    if seq == 0 && !tried_any_format {
                        tried_any_format = true;
                        if let Ok((p, s)) = build_pipeline(&source, None) {
                            pipeline = p;
                            appsink = s;
                            continue;
                        }
                    }
                    if let gst::MessageView::Error(e) = msg.view() {
                        log::error!("[FrameCapture] Camera error on {}: {}", source, e.error());
                    }
                    failed.store(true, Ordering::SeqCst);
                    return;
                }
            }

            let _ = pipeline.set_state(gst::State::Null);
            log::debug!("[FrameCapture] GStreamer pipeline stopped. Total frames captured: {}", seq);
        });

        self.handle = Some(handle);
        Ok(())
    }

    /// True once the camera has reported an error and capture has stopped.
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::SeqCst)
    }

    /// Block until the camera delivers a frame newer than `last_seq`, or the
    /// timeout passes. Each frame is handed out at most once per `last_seq`.
    pub fn wait_new_frame(&self, last_seq: u64, timeout: Duration) -> Option<CapturedFrame> {
        let slot = self.shared.frame.lock().ok()?;
        let (slot, _) = self
            .shared
            .new_frame
            .wait_timeout_while(slot, timeout, |f| f.as_ref().map_or(true, |f| f.seq <= last_seq))
            .ok()?;
        slot.as_ref().filter(|f| f.seq > last_seq).cloned()
    }

    pub fn read_captured_frame(&self) -> Option<CapturedFrame> {
        let lock = self.shared.frame.lock().ok()?;
        let cap = lock.clone()?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_camera_source_validation() {
        assert!(is_valid_source("/dev/video0"));
        assert!(is_valid_source("/dev/video12"));
        assert!(is_valid_source("pipewiresrc"));

        assert!(!is_valid_source(""));
        assert!(!is_valid_source("/dev/video"));
        assert!(!is_valid_source("/dev/video0 ! filesink location=/etc/passwd"));
        assert!(!is_valid_source("/dev/sda"));
        assert!(!is_valid_source("videotestsrc"));

        assert!(FrameCapture::new("0").is_ok());
        assert!(FrameCapture::new("/dev/video0 ! fakesink").is_err());
    }

    #[test]
    fn test_fit_frame_keeps_aspect() {
        // 16:9 into 4:3: the sides are cropped, nothing is squashed.
        let wide = RgbImage::new(1280, 720);
        assert_eq!(fit_frame(wide, 640, 480).dimensions(), (640, 480));
        let same = RgbImage::new(640, 480);
        assert_eq!(fit_frame(same, 640, 480).dimensions(), (640, 480));
    }
}
