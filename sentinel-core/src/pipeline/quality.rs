//! Frame quality gate: decides whether a detected face is good enough to be
//! recognised. A frame that fails is simply skipped — it is never counted as a
//! failed match — so the limits here are deliberately lenient.

use image::RgbImage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityIssue {
    OutOfFrame,
    NotFrontal,
    TooDark,
    TooBright,
    Blurry,
}

impl QualityIssue {
    pub fn hint(&self) -> &'static str {
        match self {
            QualityIssue::OutOfFrame => "Move your whole face into view",
            QualityIssue::NotFrontal => "Look straight at the camera",
            QualityIssue::TooDark => "Too dark",
            QualityIssue::TooBright => "Too bright",
            QualityIssue::Blurry => "Hold still",
        }
    }
}

/// Nose offset from the eye midpoint, in units of the eye distance. About 0
/// when facing the camera; this is the limit for "turned too far sideways".
const MAX_YAW_RATIO: f32 = 0.45;
/// Nose height below the eye line, same units. About 0.57 for a level head.
const PITCH_RATIO_RANGE: (f32, f32) = (0.20, 1.00);
/// Head tilt (roll) limit in degrees. Alignment removes roll, but an extreme
/// tilt means the landmarks themselves are unreliable.
const MAX_ROLL_DEG: f32 = 35.0;
/// Acceptable mean brightness (0..255) of the aligned face.
const LUMA_RANGE: (f32, f32) = (35.0, 225.0);
/// Minimum variance of the Laplacian over the aligned face (see `sharpness`).
const MIN_SHARPNESS: f32 = 12.0;

/// Pose check from the five landmarks (eyes, nose, mouth corners). Cheap, so
/// it runs before alignment and embedding.
pub fn check_pose(landmarks: &[[f32; 2]; 5], frame_w: u32, frame_h: u32) -> Result<(), QualityIssue> {
    let inside = |p: &[f32; 2]| p[0] >= 0.0 && p[1] >= 0.0 && p[0] < frame_w as f32 && p[1] < frame_h as f32;
    if !landmarks.iter().all(inside) {
        return Err(QualityIssue::OutOfFrame);
    }

    let (left_eye, right_eye, nose) = (landmarks[0], landmarks[1], landmarks[2]);
    let (dx, dy) = (right_eye[0] - left_eye[0], right_eye[1] - left_eye[1]);
    let eye_dist = (dx * dx + dy * dy).sqrt();
    if !(eye_dist >= 1.0) {
        return Err(QualityIssue::NotFrontal);
    }

    // Nose position in a frame of reference that follows the eye line, so
    // tilting the head does not look like turning it.
    let (ux, uy) = (dx / eye_dist, dy / eye_dist);
    let (nx, ny) = (nose[0] - (left_eye[0] + right_eye[0]) / 2.0, nose[1] - (left_eye[1] + right_eye[1]) / 2.0);
    let yaw_ratio = (nx * ux + ny * uy) / eye_dist;
    let pitch_ratio = (-nx * uy + ny * ux) / eye_dist;
    let roll_deg = dy.atan2(dx).to_degrees();

    if yaw_ratio.abs() > MAX_YAW_RATIO
        || pitch_ratio < PITCH_RATIO_RANGE.0
        || pitch_ratio > PITCH_RATIO_RANGE.1
        || roll_deg.abs() > MAX_ROLL_DEG
    {
        return Err(QualityIssue::NotFrontal);
    }
    Ok(())
}

fn gray(image: &RgbImage) -> Vec<f32> {
    image
        .pixels()
        .map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
        .collect()
}

/// Variance of the 4-neighbour Laplacian: high for crisp edges, low for
/// motion blur or an out-of-focus face.
pub fn sharpness(image: &RgbImage) -> f32 {
    let (w, h) = (image.width() as usize, image.height() as usize);
    if w < 3 || h < 3 {
        return 0.0;
    }
    let g = gray(image);
    let mut sum = 0.0f64;
    let mut sum_sq = 0.0f64;
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let i = y * w + x;
            let lap = (g[i - 1] + g[i + 1] + g[i - w] + g[i + w] - 4.0 * g[i]) as f64;
            sum += lap;
            sum_sq += lap * lap;
        }
    }
    let n = ((w - 2) * (h - 2)) as f64;
    let mean = sum / n;
    (sum_sq / n - mean * mean) as f32
}

/// Exposure and sharpness check on the aligned 112x112 face.
pub fn check_aligned_face(aligned: &RgbImage) -> Result<(), QualityIssue> {
    let g = gray(aligned);
    let luma = g.iter().sum::<f32>() / g.len().max(1) as f32;
    if luma < LUMA_RANGE.0 {
        return Err(QualityIssue::TooDark);
    }
    if luma > LUMA_RANGE.1 {
        return Err(QualityIssue::TooBright);
    }
    if sharpness(aligned) < MIN_SHARPNESS {
        return Err(QualityIssue::Blurry);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::align::CANONICAL_LANDMARKS;
    use image::Rgb;

    fn checkerboard(low: u8, high: u8, cell: u32) -> RgbImage {
        RgbImage::from_fn(112, 112, |x, y| {
            let v = if (x / cell + y / cell) % 2 == 0 { low } else { high };
            Rgb([v, v, v])
        })
    }

    #[test]
    fn test_frontal_pose_passes() {
        assert_eq!(check_pose(&CANONICAL_LANDMARKS, 112, 112), Ok(()));
    }

    #[test]
    fn test_tilted_frontal_pose_passes() {
        // Same face rolled 20 degrees: still frontal, alignment handles roll.
        let (sin, cos) = 20f32.to_radians().sin_cos();
        let mut lm = CANONICAL_LANDMARKS;
        for p in lm.iter_mut() {
            let (x, y) = (p[0] - 56.0, p[1] - 72.0);
            *p = [cos * x - sin * y + 200.0, sin * x + cos * y + 200.0];
        }
        assert_eq!(check_pose(&lm, 640, 480), Ok(()));
    }

    #[test]
    fn test_turned_head_is_not_frontal() {
        let mut lm = CANONICAL_LANDMARKS;
        lm[2][0] += 20.0; // nose far off the eye midpoint
        assert_eq!(check_pose(&lm, 112, 112), Err(QualityIssue::NotFrontal));
    }

    #[test]
    fn test_landmark_outside_frame() {
        let mut lm = CANONICAL_LANDMARKS;
        lm[0][0] = -3.0;
        assert_eq!(check_pose(&lm, 112, 112), Err(QualityIssue::OutOfFrame));
    }

    #[test]
    fn test_exposure_and_sharpness() {
        assert_eq!(check_aligned_face(&checkerboard(80, 180, 4)), Ok(()));
        assert_eq!(check_aligned_face(&checkerboard(0, 20, 4)), Err(QualityIssue::TooDark));
        assert_eq!(check_aligned_face(&checkerboard(240, 255, 4)), Err(QualityIssue::TooBright));
        // A flat grey patch has no edges at all.
        assert_eq!(check_aligned_face(&checkerboard(128, 128, 4)), Err(QualityIssue::Blurry));
    }
}
