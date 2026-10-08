use anyhow::{bail, Context, Result};
use image::{imageops, RgbImage};
use ort::{session::Session, value::Tensor};

/// Scale `v` to unit length. Returns false (leaving `v` untouched) when the
/// vector has no usable direction (near-zero or non-finite norm).
pub fn l2_normalize(v: &mut [f32; 512]) -> bool {
    let mut sum_sq = 0.0f64;
    for x in v.iter() {
        sum_sq += (*x as f64) * (*x as f64);
    }
    let norm = sum_sq.sqrt() as f32;
    if !norm.is_finite() || norm < 1e-10 {
        return false;
    }
    for x in v.iter_mut() {
        *x /= norm;
    }
    true
}

pub struct MobileFaceNet {
    session: Session,
}

impl MobileFaceNet {
    pub fn new(model_path: &str) -> Result<Self> {
        let session = Session::builder()
            .map_err(|e| anyhow::anyhow!("{:?}", e))?
            .commit_from_file(model_path)
            .with_context(|| format!("Failed to load MobileFaceNet ONNX model from: {}", model_path))?;
        Ok(Self { session })
    }

    /// Raw (not normalised) 512-d network output for one aligned face.
    fn forward(&mut self, aligned_face: &RgbImage) -> Result<[f32; 512]> {
        if aligned_face.width() != 112 || aligned_face.height() != 112 {
            bail!(
                "Invalid input size for MobileFaceNet: expected 112x112, got {}x{}",
                aligned_face.width(),
                aligned_face.height()
            );
        }

        // Build CHW float tensor: [1, 3, 112, 112]
        let mut flat = Vec::with_capacity(1 * 3 * 112 * 112);
        for ch in 0..3usize {
            for r in 0..112u32 {
                for c in 0..112u32 {
                    let pixel = aligned_face.get_pixel(c, r);
                    let val = pixel[ch] as f32;
                    // MobileFaceNet normalization: (x / 127.5) - 1.0
                    flat.push((val / 127.5) - 1.0);
                }
            }
        }

        let input_tensor = Tensor::<f32>::from_array(([1usize, 3, 112, 112], flat.into_boxed_slice()))?;
        let outputs = self.session.run(ort::inputs![input_tensor])?;

        let output_value = outputs.values().next().context("No output tensor returned by MobileFaceNet")?;
        let (_shape, slice) = output_value.try_extract_tensor::<f32>()?;

        if slice.len() != 512 {
            bail!("Expected 512-d embedding output, got {} elements", slice.len());
        }

        let mut vec = [0.0f32; 512];
        vec.copy_from_slice(slice);
        Ok(vec)
    }

    pub fn embed(&mut self, aligned_face: &RgbImage) -> Result<[f32; 512]> {
        let mut vec = self.forward(aligned_face)?;
        if !l2_normalize(&mut vec) {
            bail!("Degenerate embedding vector: L2 norm is near zero");
        }
        Ok(vec)
    }

    /// Embedding of the face plus its mirror image, summed then normalised.
    /// Twice the cost of `embed`; used at enrollment to get steadier templates.
    pub fn embed_with_flip(&mut self, aligned_face: &RgbImage) -> Result<[f32; 512]> {
        let mut vec = self.forward(aligned_face)?;
        let mirrored = self.forward(&imageops::flip_horizontal(aligned_face))?;
        for (a, b) in vec.iter_mut().zip(mirrored.iter()) {
            *a += b;
        }
        if !l2_normalize(&mut vec) {
            bail!("Degenerate embedding vector: L2 norm is near zero");
        }
        Ok(vec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_embedding_unit_norm() {
        let mut raw = [0.5f32; 512];
        assert!(l2_normalize(&mut raw));
        let mut norm_sq = 0.0f32;
        for x in raw {
            norm_sq += x * x;
        }
        let norm = norm_sq.sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_degenerate_embedding_is_rejected_not_a_panic() {
        assert!(!l2_normalize(&mut [0.0f32; 512]));
        assert!(!l2_normalize(&mut [f32::NAN; 512]));
    }
}
