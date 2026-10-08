use anyhow::{bail, Context, Result};
use image::{Rgb, RgbImage};
use ort::{session::Session, value::Tensor};

/// MiniFASNet output classes: index 1 is "real", 0 and 2 are attack types.
const REAL_CLASS: usize = 1;
/// Network input side in pixels.
const INPUT_SIZE: u32 = 80;

pub fn softmax(logits: &[f32]) -> Vec<f32> {
    if logits.is_empty() {
        return Vec::new();
    }
    let max_val = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let exps: Vec<f32> = logits.iter().map(|x| (x - max_val).exp()).collect();
    let sum_exp: f32 = exps.iter().sum();
    if sum_exp < 1e-10 {
        return vec![1.0 / (logits.len() as f32); logits.len()];
    }
    exps.into_iter().map(|x| x / sum_exp).collect()
}

/// Passive anti-spoof check with one or more MiniFASNet models.
///
/// Each model was trained on a face crop of a specific size relative to the
/// detected face box (its "scale": 2.7 for MiniFASNetV2, 4.0 for
/// MiniFASNetV1SE) and must be fed the same way: BGR channel order, raw
/// 0..255 values, no mean/std.
pub struct SpoofDetector {
    models: Vec<(Session, f32)>,
}

impl SpoofDetector {
    /// Load the given `(model path, crop scale)` pairs. Every model is run
    /// once on a blank input so a broken model is caught here, not mid-login.
    pub fn new(models: &[(&str, f32)]) -> Result<Self> {
        if models.is_empty() {
            bail!("No anti-spoof model given");
        }
        let mut loaded = Vec::with_capacity(models.len());
        for &(model_path, scale) in models {
            let mut session = Session::builder()
                .map_err(|e| anyhow::anyhow!("{:?}", e))?
                .commit_from_file(model_path)
                .with_context(|| format!("Failed to load MiniFASNet model from: {}", model_path))?;

            let logits = Self::run(&mut session, &RgbImage::new(INPUT_SIZE, INPUT_SIZE))?;
            if logits.len() != 3 || logits.iter().any(|x| !x.is_finite()) {
                bail!("MiniFASNet self-check failed for {}: output {:?}", model_path, logits);
            }
            log::debug!("MiniFASNet self-check for {}: blank-input logits {:?}", model_path, logits);
            loaded.push((session, scale));
        }
        Ok(Self { models: loaded })
    }

    /// Crop box around the face, `scale` times the face box. The box keeps the
    /// face box's aspect ratio, shrinks if the frame is too small for it and is
    /// shifted (not cut) to stay inside the frame — the same rule the models
    /// were trained with. Returns inclusive pixel corners `[x1, y1, x2, y2]`.
    fn crop_box(frame_w: u32, frame_h: u32, bbox: [f32; 4], scale: f32) -> Option<[u32; 4]> {
        let (src_w, src_h) = (frame_w as f32, frame_h as f32);
        let box_w = bbox[2] - bbox[0];
        let box_h = bbox[3] - bbox[1];
        if frame_w < 2 || frame_h < 2 || !(box_w >= 1.0) || !(box_h >= 1.0) {
            return None;
        }

        let scale = scale.min((src_h - 1.0) / box_h).min((src_w - 1.0) / box_w);
        let new_w = box_w * scale;
        let new_h = box_h * scale;
        let center_x = bbox[0] + box_w / 2.0;
        let center_y = bbox[1] + box_h / 2.0;

        let mut x1 = center_x - new_w / 2.0;
        let mut y1 = center_y - new_h / 2.0;
        let mut x2 = center_x + new_w / 2.0;
        let mut y2 = center_y + new_h / 2.0;

        if x1 < 0.0 {
            x2 -= x1;
            x1 = 0.0;
        }
        if y1 < 0.0 {
            y2 -= y1;
            y1 = 0.0;
        }
        if x2 > src_w - 1.0 {
            x1 -= x2 - src_w + 1.0;
            x2 = src_w - 1.0;
        }
        if y2 > src_h - 1.0 {
            y1 -= y2 - src_h + 1.0;
            y2 = src_h - 1.0;
        }

        let clamp = |v: f32, max: u32| (v.max(0.0) as u32).min(max);
        let (x1, y1) = (clamp(x1, frame_w - 1), clamp(y1, frame_h - 1));
        let (x2, y2) = (clamp(x2, frame_w - 1), clamp(y2, frame_h - 1));
        (x2 > x1 && y2 > y1).then_some([x1, y1, x2, y2])
    }

    /// Cut the crop box out of the frame and scale it to the 80x80 network
    /// input with plain bilinear sampling (no smoothing), as in training.
    pub fn crop(frame: &RgbImage, bbox: [f32; 4], scale: f32) -> Result<RgbImage> {
        let [x1, y1, x2, y2] = Self::crop_box(frame.width(), frame.height(), bbox, scale)
            .context("Face box unusable for anti-spoof crop")?;
        let (crop_w, crop_h) = ((x2 - x1 + 1) as f32, (y2 - y1 + 1) as f32);

        let mut out = RgbImage::new(INPUT_SIZE, INPUT_SIZE);
        for oy in 0..INPUT_SIZE {
            let sy = ((oy as f32 + 0.5) * crop_h / INPUT_SIZE as f32 - 0.5).clamp(0.0, crop_h - 1.0);
            let (y0, fy) = (sy.floor() as u32, sy.fract());
            let y_next = (y0 + 1).min(y2 - y1);
            for ox in 0..INPUT_SIZE {
                let sx = ((ox as f32 + 0.5) * crop_w / INPUT_SIZE as f32 - 0.5).clamp(0.0, crop_w - 1.0);
                let (x0, fx) = (sx.floor() as u32, sx.fract());
                let x_next = (x0 + 1).min(x2 - x1);

                let p00 = frame.get_pixel(x1 + x0, y1 + y0);
                let p10 = frame.get_pixel(x1 + x_next, y1 + y0);
                let p01 = frame.get_pixel(x1 + x0, y1 + y_next);
                let p11 = frame.get_pixel(x1 + x_next, y1 + y_next);

                let mut px = [0u8; 3];
                for ch in 0..3 {
                    let top = p00[ch] as f32 * (1.0 - fx) + p10[ch] as f32 * fx;
                    let bottom = p01[ch] as f32 * (1.0 - fx) + p11[ch] as f32 * fx;
                    px[ch] = (top * (1.0 - fy) + bottom * fy).round().clamp(0.0, 255.0) as u8;
                }
                out.put_pixel(ox, oy, Rgb(px));
            }
        }
        Ok(out)
    }

    fn run(session: &mut Session, crop_80x80: &RgbImage) -> Result<Vec<f32>> {
        let mut flat = Vec::with_capacity(1 * 3 * 80 * 80);
        // The frame is RGB; the model wants planes in B, G, R order.
        for ch in [2usize, 1, 0] {
            for r in 0..INPUT_SIZE {
                for c in 0..INPUT_SIZE {
                    flat.push(crop_80x80.get_pixel(c, r)[ch] as f32);
                }
            }
        }

        let input_tensor = Tensor::<f32>::from_array(([1usize, 3, 80, 80], flat.into_boxed_slice()))?;
        let outputs = session.run(ort::inputs![input_tensor])?;

        let output_val = outputs.values().next().context("No output from MiniFASNet")?;
        let (_shape, slice) = output_val.try_extract_tensor::<f32>()?;
        Ok(slice.to_vec())
    }

    /// Probability (0..1) that the face in `bbox` is a live person, averaged
    /// over all loaded models.
    pub fn predict(&mut self, frame: &RgbImage, bbox: [f32; 4]) -> Result<f32> {
        let mut real_sum = 0.0f32;
        for (session, scale) in self.models.iter_mut() {
            let crop = Self::crop(frame, bbox, *scale)?;
            let probs = softmax(&Self::run(session, &crop)?);
            real_sum += *probs.get(REAL_CLASS).context("MiniFASNet output has no 'real' class")?;
        }
        Ok(real_sum / self.models.len() as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crop_box_is_scaled_and_centred() {
        // 100x100 face in the middle of a large frame, scale 2.7 → 270x270 box.
        let b = SpoofDetector::crop_box(2000, 2000, [950.0, 950.0, 1050.0, 1050.0], 2.7).unwrap();
        assert_eq!(b, [865, 865, 1135, 1135]);
    }

    #[test]
    fn test_crop_box_is_shifted_not_cut_at_the_edge() {
        // Face near the left edge: the box keeps its width and slides right.
        let b = SpoofDetector::crop_box(640, 480, [10.0, 200.0, 110.0, 300.0], 2.7).unwrap();
        assert_eq!(b[0], 0);
        assert_eq!(b[2] - b[0], 270);
        assert_eq!(b[3] - b[1], 270);
    }

    #[test]
    fn test_crop_box_shrinks_to_fit_small_frame() {
        // 160x215 face in a 640x480 frame: 2.7x would be 580 px tall, so the
        // scale drops to (480-1)/215 and the box spans the full frame height.
        let b = SpoofDetector::crop_box(640, 480, [240.0, 120.0, 400.0, 335.0], 2.7).unwrap();
        assert!(b[1] == 0 && b[3] >= 478);
        let aspect = (b[2] - b[0]) as f32 / (b[3] - b[1]) as f32;
        assert!((aspect - 160.0 / 215.0).abs() < 0.01);
    }

    /// Guards the model file and the input format together: MiniFASNetV2 fed a
    /// blank 80x80 input must give these logits (the same values come out of
    /// OpenCV's DNN module for this file). Skipped where the model is absent.
    #[test]
    fn test_minifasnet_v2_blank_input_fingerprint() {
        let model_path = "/var/cache/sentinel/models/MiniFASNetV2.onnx";
        if !std::path::Path::new(model_path).exists() {
            return;
        }
        let mut session = Session::builder().unwrap().commit_from_file(model_path).unwrap();
        let logits = SpoofDetector::run(&mut session, &RgbImage::new(INPUT_SIZE, INPUT_SIZE)).unwrap();
        let expected = [-3.4481f32, -0.7003, 4.1493];
        assert_eq!(logits.len(), 3);
        for (got, want) in logits.iter().zip(expected.iter()) {
            assert!((got - want).abs() < 0.01, "logits {:?} != {:?}", logits, expected);
        }
    }

    #[test]
    fn test_crop_rejects_degenerate_boxes() {
        assert!(SpoofDetector::crop_box(640, 480, [100.0, 100.0, 100.0, 100.0], 2.7).is_none());
        assert!(SpoofDetector::crop_box(640, 480, [f32::NAN, 0.0, 50.0, 50.0], 2.7).is_none());
        let frame = RgbImage::new(640, 480);
        assert_eq!(
            SpoofDetector::crop(&frame, [240.0, 120.0, 400.0, 335.0], 2.7).unwrap().dimensions(),
            (80, 80)
        );
    }
}
