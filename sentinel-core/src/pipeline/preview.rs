use anyhow::Result;
use image::RgbImage;
use minifb::{Key, Window, WindowOptions};

/// Tier-coded face box color as 0x00RRGGBB
/// Tier 1 Golden  -> Green    0x0000FF00
/// Tier 2 Standard-> Cyan     0x0000FFFF
/// Tier 3 TwoFA   -> Orange   0x00FF7F00
/// Tier 4 Denied  -> Red      0x00FF0000
/// Calibrating    -> Yellow   0x00FFFF00  (any bbox with no tier yet)
pub const COLOR_TIER1: u32 = 0x0000FF00;
pub const COLOR_TIER2: u32 = 0x0000FFFF;
pub const COLOR_TIER3: u32 = 0x00FF7F00;
pub const COLOR_TIER4: u32 = 0x00FF0000;
pub const COLOR_CALIB: u32 = 0x00FFFF00;
pub const COLOR_DEFAULT: u32 = 0x00FF0000;

pub struct DebugPreviewWindow {
    window: Window,
    buffer: Vec<u32>,
    width: usize,
    height: usize,
}

impl DebugPreviewWindow {
    pub fn new(title: &str, width: usize, height: usize) -> Result<Self> {
        let mut window = Window::new(
            title,
            width,
            height,
            WindowOptions {
                resize: false,
                ..WindowOptions::default()
            },
        )
        .map_err(|e| anyhow::anyhow!("Failed to create minifb window: {}", e))?;

        window.set_target_fps(60);

        Ok(Self {
            window,
            buffer: vec![0u32; width * height],
            width,
            height,
        })
    }

    /// Draw frame with uniformly colored bboxes (legacy).
    pub fn draw_frame(&mut self, frame: &RgbImage, bboxes: &[[f32; 4]]) -> bool {
        let colored: Vec<([f32; 4], u32)> = bboxes.iter().map(|b| (*b, COLOR_DEFAULT)).collect();
        self.draw_frame_colored(frame, &colored)
    }

    /// Draw frame with per-bbox color (tier-coded).
    /// `bboxes` is a slice of (bbox, color_0x00RRGGBB).
    pub fn draw_frame_colored(&mut self, frame: &RgbImage, bboxes: &[([f32; 4], u32)]) -> bool {
        if !self.window.is_open() || self.window.is_key_down(Key::Escape) {
            return false;
        }

        let fw = frame.width() as usize;
        let fh = frame.height() as usize;

        for y in 0..self.height {
            for x in 0..self.width {
                if x < fw && y < fh {
                    let pixel = frame.get_pixel(x as u32, y as u32);
                    let r = pixel[0] as u32;
                    let g = pixel[1] as u32;
                    let b = pixel[2] as u32;
                    self.buffer[y * self.width + x] = (r << 16) | (g << 8) | b;
                } else {
                    self.buffer[y * self.width + x] = 0;
                }
            }
        }

        for (bbox, color) in bboxes {
            draw_rect(&mut self.buffer, self.width, self.height, bbox, *color);
        }

        if let Err(e) = self.window.update_with_buffer(&self.buffer, self.width, self.height) {
            eprintln!("[Preview] Window update error: {:?}", e);
            return false;
        }

        true
    }
}

fn draw_rect(buffer: &mut [u32], width: usize, height: usize, bbox: &[f32; 4], color: u32) {
    let x1 = (bbox[0].max(0.0) as usize).min(width.saturating_sub(1));
    let y1 = (bbox[1].max(0.0) as usize).min(height.saturating_sub(1));
    let x2 = (bbox[2].max(0.0) as usize).min(width.saturating_sub(1));
    let y2 = (bbox[3].max(0.0) as usize).min(height.saturating_sub(1));

    if x1 >= x2 || y1 >= y2 {
        return;
    }

    let thickness = 3;
    for t in 0..thickness {
        let min_y = y1 + t;
        let max_y = if y2 >= t { y2 - t } else { y2 };
        let min_x = x1 + t;
        let max_x = if x2 >= t { x2 - t } else { x2 };

        for x in x1..=x2 {
            if min_y < height {
                buffer[min_y * width + x] = color;
            }
            if max_y < height {
                buffer[max_y * width + x] = color;
            }
        }
        for y in y1..=y2 {
            if min_x < width {
                buffer[y * width + min_x] = color;
            }
            if max_x < width {
                buffer[y * width + max_x] = color;
            }
        }
    }
}
