//! The study-file preview thumbnail: the 3D viewport, scaled to at most 512
//! pixels on the long side and encoded as a small PNG. Saving never waits long
//! for it and never fails because of it.

use crate::export_panel::crop_rgba;
use eframe::egui;
use image::{
    ExtendedColorType, ImageEncoder, RgbImage,
    codecs::png::{CompressionType, FilterType as PngFilter, PngEncoder},
    imageops::{self, FilterType},
};
use std::time::Duration;

/// Tag on the screenshot command so it is not mistaken for --capture, the
/// export panel's capture or the recorder's.
pub const SCREENSHOT_TAG: &str = "faris-study-thumbnail";
/// How long a save waits for the window capture before writing without it.
pub const CAPTURE_TIMEOUT: Duration = Duration::from_millis(1500);
/// Longest side of the thumbnail in pixels.
pub const MAX_SIDE: u32 = 512;
/// Size the thumbnail aims to stay under; smaller steps are tried above it.
pub const TARGET_BYTES: usize = 150_000;
/// Long-side sizes tried in turn until the PNG fits `TARGET_BYTES`.
const STEPS: [u32; 5] = [MAX_SIDE, 384, 256, 192, 128];

pub fn is_ours(user_data: &egui::UserData) -> bool {
    user_data
        .data
        .as_ref()
        .and_then(|d| d.downcast_ref::<&str>())
        .is_some_and(|tag| *tag == SCREENSHOT_TAG)
}

/// Size that fits `width` x `height` inside a `max` x `max` box, keeping the
/// aspect ratio and never enlarging. Each side is at least one pixel.
pub fn fit_size(width: u32, height: u32, max: u32) -> (u32, u32) {
    let (width, height) = (width.max(1), height.max(1));
    if width <= max && height <= max {
        return (width, height);
    }
    let scale = f64::from(max) / f64::from(width.max(height));
    let scaled = |side: u32| ((f64::from(side) * scale).round() as u32).clamp(1, max);
    (scaled(width), scaled(height))
}

/// Scale opaque RGBA pixels down and encode them as PNG, trying smaller sizes
/// until the file fits `TARGET_BYTES`. The last attempt is returned if none do.
pub fn encode(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let rgb: Vec<u8> = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    let source = RgbImage::from_raw(width, height, rgb).ok_or("pixel data does not match size")?;
    let mut last = Vec::new();
    for side in STEPS {
        let (w, h) = fit_size(width, height, side);
        let scaled = if (w, h) == (width, height) {
            source.clone()
        } else {
            imageops::resize(&source, w, h, FilterType::Triangle)
        };
        let mut png = Vec::new();
        PngEncoder::new_with_quality(&mut png, CompressionType::Best, PngFilter::Adaptive)
            .write_image(scaled.as_raw(), w, h, ExtendedColorType::Rgb8)
            .map_err(|e| format!("could not encode the thumbnail: {e}"))?;
        let fits = png.len() <= TARGET_BYTES;
        last = png;
        if fits {
            break;
        }
    }
    Ok(last)
}

/// Crop the window screenshot to the viewport and encode the thumbnail. `None`
/// when there is no usable picture; the study is then saved without one.
pub fn from_screenshot(
    screenshot: &egui::ColorImage,
    viewport: Option<egui::Rect>,
    pixels_per_point: f32,
) -> Option<Vec<u8>> {
    let (rgba, size) = crop_rgba(screenshot, viewport, pixels_per_point).ok()?;
    encode(&rgba, size[0] as u32, size[1] as u32).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_keeps_aspect_and_the_bound() {
        assert_eq!(fit_size(1024, 512, 512), (512, 256));
        assert_eq!(fit_size(512, 1024, 512), (256, 512));
        assert_eq!(fit_size(1600, 900, 512), (512, 288));
        assert_eq!(fit_size(1000, 1000, 512), (512, 512));
    }

    #[test]
    fn fit_never_enlarges_and_never_reaches_zero() {
        assert_eq!(fit_size(300, 200, 512), (300, 200));
        assert_eq!(fit_size(512, 512, 512), (512, 512));
        assert_eq!(fit_size(10_000, 3, 512), (512, 1));
        assert_eq!(fit_size(0, 0, 512), (1, 1));
        for (w, h) in [(1921, 1081), (777, 5000), (4096, 4095)] {
            let (fw, fh) = fit_size(w, h, 512);
            assert!(fw <= 512 && fh <= 512 && fw.max(fh) == 512);
            let before = f64::from(w) / f64::from(h);
            let after = f64::from(fw) / f64::from(fh);
            assert!((before / after - 1.0).abs() < 0.01, "{w}x{h} -> {fw}x{fh}");
        }
    }

    fn rgba(width: u32, height: u32, mut pixel: impl FnMut(u32, u32) -> [u8; 3]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let [r, g, b] = pixel(x, y);
                bytes.extend_from_slice(&[r, g, b, 255]);
            }
        }
        bytes
    }

    fn dimensions(png: &[u8]) -> (u32, u32) {
        (
            u32::from_be_bytes(png[16..20].try_into().unwrap()),
            u32::from_be_bytes(png[20..24].try_into().unwrap()),
        )
    }

    #[test]
    fn a_smooth_view_is_scaled_to_512_and_small() {
        let pixels = rgba(1600, 900, |x, y| {
            [(x / 8) as u8, (y / 6) as u8, ((x + y) / 16) as u8]
        });
        let png = encode(&pixels, 1600, 900).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(dimensions(&png), (512, 288));
        assert!(png.len() <= TARGET_BYTES, "{} bytes", png.len());
    }

    #[test]
    fn noise_steps_down_until_it_fits_and_keeps_the_aspect() {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let pixels = rgba(1200, 800, |_, _| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            [state as u8, (state >> 8) as u8, (state >> 16) as u8]
        });
        let png = encode(&pixels, 1200, 800).unwrap();
        let (w, h) = dimensions(&png);
        assert!(png.len() <= TARGET_BYTES, "{} bytes at {w}x{h}", png.len());
        assert!(w < 512, "noise at 512 px cannot fit, so it must shrink");
        assert!((f64::from(w) / f64::from(h) - 1.5).abs() < 0.02);
    }

    #[test]
    fn a_small_viewport_is_not_enlarged() {
        let pixels = rgba(300, 200, |x, _| [x as u8, 0, 0]);
        let png = encode(&pixels, 300, 200).unwrap();
        assert_eq!(dimensions(&png), (300, 200));
    }

    #[test]
    fn mismatched_pixel_data_is_an_error_not_a_panic() {
        assert!(encode(&[0; 12], 4, 4).is_err());
    }

    #[test]
    fn only_our_tag_is_recognised() {
        assert!(is_ours(&egui::UserData::new(SCREENSHOT_TAG)));
        assert!(!is_ours(&egui::UserData::default()));
        assert!(!is_ours(&egui::UserData::new("faris-export-view")));
    }

    #[test]
    fn no_viewport_means_no_thumbnail() {
        let shot = egui::ColorImage::filled([400, 300], egui::Color32::BLACK);
        assert!(from_screenshot(&shot, None, 1.0).is_none());
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(300.0, 200.0));
        let png = from_screenshot(&shot, Some(rect), 1.0).unwrap();
        assert_eq!(dimensions(&png), (300, 200));
    }
}
