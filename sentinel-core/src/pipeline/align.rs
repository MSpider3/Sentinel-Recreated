use anyhow::{bail, Result};
use image::{RgbImage, Rgb};

pub const CANONICAL_LANDMARKS: [[f32; 2]; 5] = [
    [38.2946, 51.6963], // Left eye
    [73.5318, 51.5014], // Right eye
    [56.0252, 71.7366], // Nose tip
    [41.5493, 92.3655], // Left mouth corner
    [70.7299, 92.2041], // Right mouth corner
];

/// Computes similarity transformation matrix M (2x3) mapping src 5 points to
/// dst 5 points using the 2D Umeyama method.
pub fn get_similarity_transform(
    src: &[[f32; 2]; 5],
    dst: &[[f32; 2]; 5],
) -> Result<[[f64; 3]; 2]> {
    let mut src_mean = [0.0f64; 2];
    let mut dst_mean = [0.0f64; 2];
    for i in 0..5 {
        src_mean[0] += src[i][0] as f64;
        src_mean[1] += src[i][1] as f64;
        dst_mean[0] += dst[i][0] as f64;
        dst_mean[1] += dst[i][1] as f64;
    }
    src_mean[0] /= 5.0;
    src_mean[1] /= 5.0;
    dst_mean[0] /= 5.0;
    dst_mean[1] /= 5.0;

    let mut src_var = 0.0f64;
    let mut sxy_00 = 0.0f64;
    let mut sxy_01 = 0.0f64;
    let mut sxy_10 = 0.0f64;
    let mut sxy_11 = 0.0f64;

    for i in 0..5 {
        let sx = src[i][0] as f64 - src_mean[0];
        let sy = src[i][1] as f64 - src_mean[1];
        let dx = dst[i][0] as f64 - dst_mean[0];
        let dy = dst[i][1] as f64 - dst_mean[1];
        src_var += sx * sx + sy * sy;
        sxy_00 += sx * dx;
        sxy_01 += sx * dy;
        sxy_10 += sy * dx;
        sxy_11 += sy * dy;
    }

    if src_var.abs() < 1e-10 {
        bail!("Degenerate source landmarks: variance near zero");
    }

    let a = sxy_00 + sxy_11;
    let b = sxy_10 - sxy_01;
    let scale = (a * a + b * b).sqrt() / src_var;
    let angle = b.atan2(a);

    let cos_theta = angle.cos();
    let sin_theta = angle.sin();

    let r00 = scale * cos_theta;
    let r01 = -scale * sin_theta;
    let r10 = scale * sin_theta;
    let r11 = scale * cos_theta;

    let tx = dst_mean[0] - (r00 * src_mean[0] + r01 * src_mean[1]);
    let ty = dst_mean[1] - (r10 * src_mean[0] + r11 * src_mean[1]);

    Ok([[r00, r01, tx], [r10, r11, ty]])
}

/// Apply 2x3 affine warp to produce a 112x112 cropped/aligned face image.
/// Uses bilinear sampling over the source image.
pub fn warp_affine_112(src: &RgbImage, m: [[f64; 3]; 2]) -> RgbImage {
    let (src_w, src_h) = src.dimensions();
    let mut dst = RgbImage::new(112, 112);

    // Invert the 2x3 affine matrix so we can do inverse mapping (dst → src).
    // For a similarity transform M, inv(M) has the form:
    // det = r00*r11 - r01*r10
    let r00 = m[0][0]; let r01 = m[0][1]; let tx = m[0][2];
    let r10 = m[1][0]; let r11 = m[1][1]; let ty = m[1][2];
    let det = r00 * r11 - r01 * r10;
    if det.abs() < 1e-10 {
        return dst; // degenerate, return black
    }
    let inv_det = 1.0 / det;
    let ir00 = r11 * inv_det;
    let ir01 = -r01 * inv_det;
    let ir10 = -r10 * inv_det;
    let ir11 = r00 * inv_det;
    let itx = (r01 * ty - r11 * tx) * inv_det;
    let ity = (r10 * tx - r00 * ty) * inv_det;

    for dy in 0u32..112 {
        for dx in 0u32..112 {
            let fx = dx as f64;
            let fy = dy as f64;
            // Map destination pixel back to source coordinates
            let sx = ir00 * fx + ir01 * fy + itx;
            let sy = ir10 * fx + ir11 * fy + ity;

            // Bilinear interpolation
            let x0 = sx.floor() as i64;
            let y0 = sy.floor() as i64;
            let x1 = x0 + 1;
            let y1 = y0 + 1;
            let fx_frac = (sx - x0 as f64) as f32;
            let fy_frac = (sy - y0 as f64) as f32;

            let clamp_x = |x: i64| x.clamp(0, (src_w as i64) - 1) as u32;
            let clamp_y = |y: i64| y.clamp(0, (src_h as i64) - 1) as u32;

            let p00 = src.get_pixel(clamp_x(x0), clamp_y(y0));
            let p10 = src.get_pixel(clamp_x(x1), clamp_y(y0));
            let p01 = src.get_pixel(clamp_x(x0), clamp_y(y1));
            let p11 = src.get_pixel(clamp_x(x1), clamp_y(y1));

            let lerp = |a: u8, b: u8, t: f32| -> u8 {
                let v = (a as f32) * (1.0 - t) + (b as f32) * t;
                v.clamp(0.0, 255.0) as u8
            };

            let mut out_pixel = [0u8; 3];
            for ch in 0..3 {
                let top = lerp(p00[ch], p10[ch], fx_frac);
                let bot = lerp(p01[ch], p11[ch], fx_frac);
                out_pixel[ch] = lerp(top, bot, fy_frac);
            }
            dst.put_pixel(dx, dy, Rgb(out_pixel));
        }
    }
    dst
}

pub fn align_face(frame: &RgbImage, landmarks: &[[f32; 2]; 5]) -> Result<RgbImage> {
    let transform_matrix = get_similarity_transform(landmarks, &CANONICAL_LANDMARKS)?;
    let aligned = warp_affine_112(frame, transform_matrix);
    if aligned.width() != 112 || aligned.height() != 112 {
        bail!("Failed to generate 112x112 aligned image");
    }
    Ok(aligned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alignment_canonical_identity() {
        let m = get_similarity_transform(&CANONICAL_LANDMARKS, &CANONICAL_LANDMARKS).unwrap();
        assert!((m[0][0] - 1.0).abs() < 1e-4);
        assert!((m[0][1] - 0.0).abs() < 1e-4);
        assert!((m[0][2] - 0.0).abs() < 1e-4);
        assert!((m[1][0] - 0.0).abs() < 1e-4);
        assert!((m[1][1] - 1.0).abs() < 1e-4);
        assert!((m[1][2] - 0.0).abs() < 1e-4);
    }

    #[test]
    fn test_alignment_determinism() {
        let dummy_frame = RgbImage::new(640, 480);
        let landmarks = [
            [200.0, 180.0],
            [350.0, 180.0],
            [275.0, 250.0],
            [220.0, 320.0],
            [330.0, 320.0],
        ];

        let res1 = align_face(&dummy_frame, &landmarks).unwrap();
        let res2 = align_face(&dummy_frame, &landmarks).unwrap();

        assert_eq!(res1.width(), 112);
        assert_eq!(res1.height(), 112);
        // Both calls should be identical
        assert_eq!(res1.as_raw(), res2.as_raw());
    }
}
