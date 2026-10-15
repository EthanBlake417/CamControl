//! Fluorescence composites: each channel image turned to brightness, stretched
//! between a black and a white level, tinted, and the tinted images added.

use image::RgbImage;
use serde::{Deserialize, Serialize};

use crate::imgproc as ip;
use crate::processing::common::check_same_size;

/// Display name, (R, G, B)
pub const COLORS: [(&str, [f32; 3]); 7] = [
    ("Blue", [0.0, 0.0, 255.0]),
    ("Green", [0.0, 255.0, 0.0]),
    ("Red", [255.0, 0.0, 0.0]),
    ("Magenta", [255.0, 0.0, 255.0]),
    ("Cyan", [0.0, 255.0, 255.0]),
    ("Yellow", [255.0, 255.0, 0.0]),
    ("Gray", [255.0, 255.0, 255.0]),
];

const NAME_HINTS: [(&str, &[&str]); 4] = [
    ("Blue", &["dapi", "hoechst", "blue", "uv"]),
    ("Green", &["fitc", "gfp", "green", "alexa488", "af488"]),
    ("Red", &["tritc", "rfp", "mcherry", "texas", "cy3", "red", "alexa594", "af594"]),
    ("Magenta", &["cy5", "far-red", "farred", "magenta"]),
];

pub fn guess_color(file_name: &str) -> Option<&'static str> {
    let name = file_name.to_lowercase();
    NAME_HINTS.iter().find(|(_, words)| words.iter().any(|w| name.contains(w))).map(|(c, _)| *c)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChannelSettings {
    pub color: String,
    pub auto_levels: bool,
    pub brightness: f64,
}

pub fn composite(images: &[RgbImage], settings: &[ChannelSettings]) -> Result<RgbImage, String> {
    if images.is_empty() {
        return Err("Add at least one channel.".into());
    }
    check_same_size(images)?;
    let (w, h) = images[0].dimensions();
    let n = (w * h) as usize;
    let mut out = vec![[0f32; 3]; n];
    for (img, s) in images.iter().zip(settings) {
        let signal: Vec<f32> = img.pixels().map(|p| p.0.iter().cloned().max().unwrap() as f32).collect();
        let (black, white) = if s.auto_levels {
            let mut sorted = signal.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
            (ip::percentile_sorted(&sorted, 0.5), ip::percentile_sorted(&sorted, 99.9))
        } else {
            (0.0, 255.0)
        };
        let rgb = COLORS.iter().find(|c| c.0 == s.color).map(|c| c.1).unwrap_or([0.0, 255.0, 0.0]);
        let span = (white - black).max(1.0);
        for i in 0..n {
            let v = ((signal[i] - black) / span).clamp(0.0, 1.0) * s.brightness as f32;
            for c in 0..3 {
                out[i][c] += v * rgb[c];
            }
        }
    }
    Ok(RgbImage::from_fn(w, h, |x, y| {
        let p = out[(y * w + x) as usize];
        image::Rgb([ip::sat_u8((p[0] + 0.5).floor()), ip::sat_u8((p[1] + 0.5).floor()), ip::sat_u8((p[2] + 0.5).floor())])
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    #[test]
    fn adds_tinted_channels() {
        let dapi = RgbImage::from_fn(100, 100, |x, y| { let v = if (20..40).contains(&x) && (20..40).contains(&y) { 200 } else { 0 }; Rgb([v, v, v]) });
        let gfp = RgbImage::from_fn(100, 100, |x, y| { let v = if (30..60).contains(&x) && (30..60).contains(&y) { 90 } else { 0 }; Rgb([v, v, v]) });
        let s = |c: &str| ChannelSettings { color: c.into(), auto_levels: true, brightness: 1.0 };
        let img = composite(&[dapi, gfp], &[s("Blue"), s("Green")]).unwrap();
        assert_eq!(img.get_pixel(25, 25).0, [0, 0, 255]);
        assert_eq!(img.get_pixel(50, 50).0, [0, 255, 0]);
        assert_eq!(img.get_pixel(35, 35).0, [0, 255, 255]);
        assert_eq!(guess_color("sample_DAPI-001.tif"), Some("Blue"));
    }
}
