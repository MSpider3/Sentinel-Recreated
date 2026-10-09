use anyhow::{bail, Context, Result};
use image::{imageops, RgbImage};
use ort::{session::Session, value::Tensor};

use crate::config::DetectionConfig;

/// Frames with a mean pixel value below this get brightened for detection.
const DARK_FRAME_MEAN: f32 = 90.0;
/// Mean pixel value a dark frame is brightened to.
const BRIGHTEN_TO_MEAN: f32 = 110.0;
const MAX_BRIGHTEN_GAIN: f32 = 3.0;

#[derive(Debug, Clone)]
pub struct FaceDetection {
    pub bbox: [f32; 4],           // [x1, y1, x2, y2]
    pub landmarks: [[f32; 2]; 5], // [left_eye, right_eye, nose, left_mouth, right_mouth]
    pub score: f32,
}

#[derive(Debug, Clone)]
pub struct RawCandidate {
    pub bbox: [f32; 4],           // [x1, y1, x2, y2]
    pub landmarks: [[f32; 2]; 5],
    pub score: f32,
    pub bw: f32,
    pub bh: f32,
    pub filter_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ScrfdResult {
    pub detections: Vec<FaceDetection>,
    pub raw_candidates: Vec<RawCandidate>,
}

pub struct ScrfdDetector {
    session: Session,
    output_names: Vec<String>,
    score_threshold: f32,
    nms_threshold: f32,
    min_face_size_px: u32,
    input_size: u32,
}

impl ScrfdDetector {
    pub fn new(
        model_path: &str,
        score_threshold: f32,
        nms_threshold: f32,
        min_face_size_px: u32,
    ) -> Result<Self> {
        Self::new_with_input_size(model_path, score_threshold, nms_threshold, min_face_size_px, 320)
    }

    pub fn new_with_input_size(
        model_path: &str,
        score_threshold: f32,
        nms_threshold: f32,
        min_face_size_px: u32,
        input_size: u32,
    ) -> Result<Self> {
        let session = Session::builder()
            .map_err(|e| anyhow::anyhow!("{:?}", e))?
            .commit_from_file(model_path)
            .with_context(|| format!("Failed to load SCRFD model from: {}", model_path))?;

        let output_names = session.outputs().iter().map(|o| o.name().to_string()).collect();

        Ok(Self {
            session,
            output_names,
            score_threshold,
            nms_threshold,
            min_face_size_px,
            input_size: Self::valid_input_size(input_size),
        })
    }

    /// SCRFD strides go up to 32, so the input side must be a multiple of 32.
    fn valid_input_size(input_size: u32) -> u32 {
        (input_size.clamp(160, 1280) / 32) * 32
    }

    /// Apply detection settings from the config (the session itself is reused).
    pub fn configure(&mut self, config: &DetectionConfig) {
        self.score_threshold = config.score_threshold;
        self.nms_threshold = config.nms_threshold;
        self.min_face_size_px = config.min_face_size_px;
        self.input_size = Self::valid_input_size(config.scrfd_input_size);
    }

    pub fn detect(&mut self, frame: &RgbImage) -> Result<Vec<FaceDetection>> {
        Ok(self.run(frame, false)?.detections)
    }

    /// Like `detect`, but also returns every raw candidate with the reason it
    /// was filtered. Diagnostics only — not for the authentication hot path.
    pub fn detect_detailed(&mut self, frame: &RgbImage) -> Result<ScrfdResult> {
        self.run(frame, true)
    }

    pub fn is_valid_frame_size(width: u32, height: u32) -> bool {
        const MAX_DIM: u32 = 8192;
        width >= 1 && height >= 1 && width <= MAX_DIM && height <= MAX_DIM
    }

    fn run(&mut self, frame: &RgbImage, collect_raw: bool) -> Result<ScrfdResult> {
        let orig_width = frame.width() as f32;
        let orig_height = frame.height() as f32;

        // Reject frames whose dimensions can amplify the separable resize
        // intermediate (source_width * input_size * 16 bytes, Rgba32F) beyond
        // a bounded budget. Legitimate camera frames are <= 4K-class; the
        // 8192 cap bounds the intermediate at ~84 MB.
        const MAX_DIM: u32 = 8192;
        if frame.width() > MAX_DIM || frame.height() > MAX_DIM {
            return Ok(ScrfdResult {
                detections: Vec::new(),
                raw_candidates: Vec::new(),
            });
        }

        if orig_width < 1.0 || orig_height < 1.0 {
            return Ok(ScrfdResult {
                detections: Vec::new(),
                raw_candidates: Vec::new(),
            });
        }

        let input_size_f = self.input_size as f32;
        let input_size_u = self.input_size as u32;

        // Letterbox: shrink keeping the aspect ratio, place top-left, pad the
        // rest with black. SCRFD is trained this way; stretching a 4:3 frame
        // into a square distorts the face and its landmarks.
        let scale = input_size_f / orig_width.max(orig_height);
        let new_w = ((orig_width * scale).round() as u32).clamp(1, input_size_u);
        let new_h = ((orig_height * scale).round() as u32).clamp(1, input_size_u);

        let resized = imageops::resize(frame, new_w, new_h, imageops::FilterType::Triangle);
        let raw_pixels = resized.as_raw();

        // In a dim room the detector misses faces it finds easily once the
        // picture is brightened. The gain is applied to the detector's copy
        // only; recognition and anti-spoof always see the untouched frame.
        let mean = raw_pixels.iter().map(|&v| v as u64).sum::<u64>() as f32 / raw_pixels.len().max(1) as f32;
        let gain = if mean < DARK_FRAME_MEAN {
            (BRIGHTEN_TO_MEAN / mean.max(1.0)).min(MAX_BRIGHTEN_GAIN)
        } else {
            1.0
        };

        let plane_size = (input_size_u * input_size_u) as usize;
        let black = -127.5 / 128.0;
        let mut flat = vec![black; 3 * plane_size];

        for y in 0..new_h as usize {
            for x in 0..new_w as usize {
                let src = (y * new_w as usize + x) * 3;
                let dst = y * input_size_u as usize + x;
                for ch in 0..3 {
                    let value = (raw_pixels[src + ch] as f32 * gain).min(255.0);
                    flat[plane_size * ch + dst] = (value - 127.5) / 128.0;
                }
            }
        }

        let input_tensor = Tensor::<f32>::from_array((
            [1usize, 3, self.input_size as usize, self.input_size as usize],
            flat.into_boxed_slice(),
        ))?;
        let outputs = self.session.run(ort::inputs![input_tensor])?;

        let mut raw_candidates = Vec::<RawCandidate>::new();
        let mut valid_candidates = Vec::<FaceDetection>::new();

        let strides = [8u32, 16, 32];
        for (s_idx, &stride) in strides.iter().enumerate() {
            let score_idx = s_idx;
            let bbox_idx = s_idx + 3;
            let kps_idx = s_idx + 6;

            if score_idx >= self.output_names.len()
                || bbox_idx >= self.output_names.len()
                || kps_idx >= self.output_names.len()
            {
                continue;
            }

            let score_name = &self.output_names[score_idx];
            let bbox_name = &self.output_names[bbox_idx];
            let kps_name = &self.output_names[kps_idx];

            if let (Some(score_val), Some(bbox_val), Some(kps_val)) = (
                outputs.get(score_name),
                outputs.get(bbox_name),
                outputs.get(kps_name),
            ) {
                let (_score_shape, score_slice) = score_val.try_extract_tensor::<f32>()?;
                let (_bbox_shape, bbox_slice) = bbox_val.try_extract_tensor::<f32>()?;
                let (_kps_shape, kps_slice) = kps_val.try_extract_tensor::<f32>()?;

                let feat_h = (self.input_size / stride) as usize;
                let feat_w = (self.input_size / stride) as usize;
                let num_anchors = 2usize;

                let cells = feat_h * feat_w * num_anchors;
                if score_slice.len() != cells
                    || bbox_slice.len() != cells * 4
                    || kps_slice.len() != cells * 10
                {
                    bail!(
                        "Unexpected SCRFD output size at stride {} (got {} scores, expected {})",
                        stride,
                        score_slice.len(),
                        cells
                    );
                }

                for r in 0..feat_h {
                    for c in 0..feat_w {
                        for a in 0..num_anchors {
                            let idx = (r * feat_w + c) * num_anchors + a;
                            let score = score_slice[idx];

                            if score >= 0.10 {
                                let cx = (c as f32) * (stride as f32);
                                let cy = (r as f32) * (stride as f32);

                                let b_idx = idx * 4;
                                let dx1 = bbox_slice[b_idx] * (stride as f32);
                                let dy1 = bbox_slice[b_idx + 1] * (stride as f32);
                                let dx2 = bbox_slice[b_idx + 2] * (stride as f32);
                                let dy2 = bbox_slice[b_idx + 3] * (stride as f32);

                                let x1 = (cx - dx1) / scale;
                                let y1 = (cy - dy1) / scale;
                                let x2 = (cx + dx2) / scale;
                                let y2 = (cy + dy2) / scale;

                                let bw = (x2 - x1).max(0.0);
                                let bh = (y2 - y1).max(0.0);

                                let k_idx = idx * 10;
                                let mut landmarks = [[0.0f32; 2]; 5];
                                for k in 0..5 {
                                    let kx = (cx + kps_slice[k_idx + k * 2] * (stride as f32)) / scale;
                                    let ky = (cy + kps_slice[k_idx + k * 2 + 1] * (stride as f32)) / scale;
                                    landmarks[k] = [kx, ky];
                                }

                                let low_score = score < self.score_threshold;
                                let too_small = bw < (self.min_face_size_px as f32)
                                    || bh < (self.min_face_size_px as f32);

                                if collect_raw {
                                    let filter_reason = if low_score {
                                        Some(format!("score below {:.2} threshold", self.score_threshold))
                                    } else if too_small {
                                        let min_dim = bw.min(bh);
                                        Some(format!("face detected but too small (bbox={:.0}px, min={}px)", min_dim, self.min_face_size_px))
                                    } else {
                                        None
                                    };
                                    raw_candidates.push(RawCandidate {
                                        bbox: [x1, y1, x2, y2],
                                        landmarks,
                                        score,
                                        bw,
                                        bh,
                                        filter_reason,
                                    });
                                }

                                if !low_score && !too_small {
                                    valid_candidates.push(FaceDetection {
                                        bbox: [x1, y1, x2, y2],
                                        landmarks,
                                        score,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        drop(outputs);

        let mut sorted_raw = raw_candidates;
        sorted_raw.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

        let nms_results = self.apply_nms(valid_candidates);
        Ok(ScrfdResult {
            detections: nms_results,
            raw_candidates: sorted_raw,
        })
    }

    fn apply_nms(&self, mut candidates: Vec<FaceDetection>) -> Vec<FaceDetection> {
        candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        let mut keep = Vec::<FaceDetection>::new();

        while !candidates.is_empty() {
            let current = candidates.remove(0);
            keep.push(current.clone());
            candidates.retain(|item| {
                let iou = compute_iou(&current.bbox, &item.bbox);
                iou < self.nms_threshold
            });
        }
        keep
    }
}

fn compute_iou(box1: &[f32; 4], box2: &[f32; 4]) -> f32 {
    let x1 = box1[0].max(box2[0]);
    let y1 = box1[1].max(box2[1]);
    let x2 = box1[2].min(box2[2]);
    let y2 = box1[3].min(box2[3]);

    let inter_w = (x2 - x1).max(0.0);
    let inter_h = (y2 - y1).max(0.0);
    let inter_area = inter_w * inter_h;

    let area1 = (box1[2] - box1[0]) * (box1[3] - box1[1]);
    let area2 = (box2[2] - box2[0]) * (box2[3] - box2[1]);
    let union_area = area1 + area2 - inter_area;

    if union_area < 1e-5 {
        return 0.0;
    }
    inter_area / union_area
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scrfd_detection_on_saved_frame() {
        let model_path = "/var/cache/sentinel/models/scrfd_500m_kps.onnx";
        let frame_path = "/tmp/sentinel_debug/frame_0140.jpg";

        if std::path::Path::new(model_path).exists() && std::path::Path::new(frame_path).exists() {
            let mut detector = ScrfdDetector::new(model_path, 0.50, 0.30, 60).unwrap();
            let img = image::open(frame_path).unwrap().to_rgb8();
            let res = detector.detect_detailed(&img).unwrap();
            println!("\n=== SCRFD TEST ON SAVED FRAME ===");
            println!("Raw candidates count: {}", res.raw_candidates.len());
            println!("Detections count: {}", res.detections.len());
            for (idx, det) in res.detections.iter().enumerate() {
                println!("Detection {}: score={:.3}, bbox={:?}", idx, det.score, det.bbox);
            }
        }
    }

    #[test]
    fn test_oversized_frame_dimension_cap() {
        assert!(!ScrfdDetector::is_valid_frame_size(8193, 100));
        assert!(!ScrfdDetector::is_valid_frame_size(100, 8193));
        assert!(!ScrfdDetector::is_valid_frame_size(0, 100));
        assert!(!ScrfdDetector::is_valid_frame_size(100, 0));
        assert!(ScrfdDetector::is_valid_frame_size(640, 480));
        assert!(ScrfdDetector::is_valid_frame_size(8192, 8192));
    }
}
