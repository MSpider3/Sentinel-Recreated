use anyhow::{Context, Result};
use image::{imageops, RgbImage};
use ort::execution_providers::{CPUExecutionProvider, OpenVINOExecutionProvider};
use ort::{session::Session, value::Tensor};

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
            .with_execution_providers([
                OpenVINOExecutionProvider::default().build(),
                CPUExecutionProvider::default().build(),
            ])
            .map_err(|e| anyhow::anyhow!("{:?}", e))?
            .with_intra_threads(2)
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
            input_size: input_size.max(160),
        })
    }

    pub fn detect(&mut self, frame: &RgbImage) -> Result<Vec<FaceDetection>> {
        let res = self.detect_detailed(frame)?;
        Ok(res.detections)
    }

    pub fn detect_detailed(&mut self, frame: &RgbImage) -> Result<ScrfdResult> {
        let orig_width = frame.width() as f32;
        let orig_height = frame.height() as f32;

        if orig_width < 1.0 || orig_height < 1.0 {
            return Ok(ScrfdResult {
                detections: Vec::new(),
                raw_candidates: Vec::new(),
            });
        }

        let input_size_f = self.input_size as f32;
        let input_size_u = self.input_size as u32;

        let scale_x = input_size_f / orig_width;
        let scale_y = input_size_f / orig_height;

        let resized = imageops::resize(frame, input_size_u, input_size_u, imageops::FilterType::Triangle);
        let raw_pixels = resized.as_raw();

        let plane_size = (input_size_u * input_size_u) as usize;
        let mut flat = vec![0.0f32; 3 * plane_size];

        for i in 0..plane_size {
            let r = raw_pixels[i * 3] as f32;
            let g = raw_pixels[i * 3 + 1] as f32;
            let b = raw_pixels[i * 3 + 2] as f32;

            flat[i] = (r - 127.5) / 128.0;
            flat[plane_size + i] = (g - 127.5) / 128.0;
            flat[plane_size * 2 + i] = (b - 127.5) / 128.0;
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

                                let x1 = (cx - dx1) / scale_x;
                                let y1 = (cy - dy1) / scale_y;
                                let x2 = (cx + dx2) / scale_x;
                                let y2 = (cy + dy2) / scale_y;

                                let bw = (x2 - x1).max(0.0);
                                let bh = (y2 - y1).max(0.0);

                                let k_idx = idx * 10;
                                let mut landmarks = [[0.0f32; 2]; 5];
                                for k in 0..5 {
                                    let kx = (cx + kps_slice[k_idx + k * 2] * (stride as f32)) / scale_x;
                                    let ky = (cy + kps_slice[k_idx + k * 2 + 1] * (stride as f32)) / scale_y;
                                    landmarks[k] = [kx, ky];
                                }

                                let filter_reason = if score < self.score_threshold {
                                    Some(format!("score below {:.2} threshold", self.score_threshold))
                                } else if bw < (self.min_face_size_px as f32) || bh < (self.min_face_size_px as f32) {
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
                                    filter_reason: filter_reason.clone(),
                                });

                                if filter_reason.is_none() {
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

        let mut sorted_raw = raw_candidates.clone();
        sorted_raw.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

        if sorted_raw.is_empty() {
            println!("[SCRFD] Raw detections: 0 faces found");
        } else {
            let top = &sorted_raw[0];
            let x = top.bbox[0] as i32;
            let y = top.bbox[1] as i32;
            let w = top.bw as i32;
            let h = top.bh as i32;
            if let Some(ref reason) = top.filter_reason {
                println!(
                    "[SCRFD] Raw detections: 1 face (score={:.2}, bbox=[x={},y={},w={},h={}]) — FILTERED: {}",
                    top.score, x, y, w, h, reason
                );
            } else {
                println!(
                    "[SCRFD] Raw detections: 1 face (score={:.2}, bbox=[x={},y={},w={},h={}]) — PASSED",
                    top.score, x, y, w, h
                );
            }
        }

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
}
