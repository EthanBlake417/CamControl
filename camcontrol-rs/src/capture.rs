//! Still capture: file names, saving with a .json sidecar, averaging frames,
//! and reading/writing image files (same rules as camcontrol/capture.py).
//!
//! Captures average N frames (less noise) and are saved at the camera's size
//! or scaled up 1.7x to match HD2 files (1920x1080 becomes 3264x1836).

use std::path::{Path, PathBuf};

use chrono::Local;
use image::{DynamicImage, RgbImage};
use serde_json::{json, Map, Value};

use crate::imgproc::{self as ip, Interp};

/// HD2 saves the MC802's 1920x1080 frames at 3264x1836: 1.7x larger.
pub const HD2_SCALE: f64 = 3264.0 / 1920.0;
pub const IMAGE_EXTENSIONS: [&str; 6] = ["tif", "tiff", "png", "jpg", "jpeg", "bmp"];

pub fn is_image(path: &Path) -> bool {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.as_str()))
}

pub fn scaled_size(w: u32, h: u32, scale: f64) -> (u32, u32) {
    ((w as f64 * scale).round() as u32, (h as f64 * scale).round() as u32)
}

/// Make a user-typed capture name safe as a file name (Windows rules, the strictest).
pub fn clean_name(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| if "<>:\"/\\|?*".contains(c) || (c as u32) < 0x20 { '_' } else { c })
        .collect();
    cleaned.trim_end_matches(['.', ' ']).to_string() // Windows doesn't allow trailing dots/spaces
}

/// Where the next capture will be saved.
///
/// With a name:    name-001.tif, name-002.tif, ... continuing after the highest
///                 number already in the folder (any extension), so nothing is
///                 overwritten. Same style as HD2 ("..._bright-001.tif").
/// Without a name: <prefix>_YYYYMMDD_HHMMSS.tif (with _1, _2 if taken).
pub fn next_capture_path(folder: &Path, name: &str, fmt: &str, prefix: &str) -> PathBuf {
    let name = clean_name(name);
    if !name.is_empty() {
        let lower = format!("{}-", name.to_lowercase());
        let highest = std::fs::read_dir(folder)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let file = entry.file_name().to_string_lossy().to_lowercase();
                let (stem, ext) = file.rsplit_once('.')?;
                if ext.is_empty() {
                    return None;
                }
                let digits = stem.strip_prefix(&lower)?;
                if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                digits.parse::<u64>().ok()
            })
            .max()
            .unwrap_or(0);
        return folder.join(format!("{name}-{:03}.{fmt}", highest + 1));
    }

    let stem = format!("{prefix}_{}", Local::now().format("%Y%m%d_%H%M%S"));
    let mut path = folder.join(format!("{stem}.{fmt}"));
    let mut n = 1;
    while path.exists() {
        // two captures in the same second
        path = folder.join(format!("{stem}_{n}.{fmt}"));
        n += 1;
    }
    path
}

/// Read an image as 8-bit RGB (16-bit shifted down, as the Python app does). None if unreadable.
pub fn load_image(path: &Path) -> Option<RgbImage> {
    let img = image::open(path).ok()?;
    Some(match img {
        DynamicImage::ImageRgb8(i) => i,
        DynamicImage::ImageLuma16(i) => {
            let (w, h) = i.dimensions();
            RgbImage::from_fn(w, h, |x, y| {
                let v = (i.get_pixel(x, y).0[0] >> 8) as u8;
                image::Rgb([v, v, v])
            })
        }
        DynamicImage::ImageRgb16(i) => {
            let (w, h) = i.dimensions();
            RgbImage::from_fn(w, h, |x, y| {
                let p = i.get_pixel(x, y).0;
                image::Rgb([(p[0] >> 8) as u8, (p[1] >> 8) as u8, (p[2] >> 8) as u8])
            })
        }
        DynamicImage::ImageRgba16(i) => {
            let (w, h) = i.dimensions();
            RgbImage::from_fn(w, h, |x, y| {
                let p = i.get_pixel(x, y).0;
                image::Rgb([(p[0] >> 8) as u8, (p[1] >> 8) as u8, (p[2] >> 8) as u8])
            })
        }
        other => other.to_rgb8(),
    })
}

/// Write an image; the format comes from the file extension (TIFF by default).
pub fn save_image(path: &Path, img: &RgbImage) -> Result<(), String> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let result = match ext.as_str() {
        "jpg" | "jpeg" => {
            let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
            let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(std::io::BufWriter::new(file), 95);
            enc.encode_image(img).map_err(|e| e.to_string())
        }
        "png" | "bmp" | "tif" | "tiff" => img.save(path).map_err(|e| e.to_string()),
        _ => img.save_with_format(path, image::ImageFormat::Tiff).map_err(|e| e.to_string()),
    };
    result.map_err(|e| format!("Could not save {}: {e}", path.display()))
}

/// Write the .json sidecar beside an image.
pub fn write_sidecar(image_path: &Path, meta: &Value) -> Result<(), String> {
    std::fs::write(image_path.with_extension("json"), serde_json::to_string_pretty(meta).unwrap_or_default())
        .map_err(|e| format!("Could not save settings beside {}: {e}", image_path.display()))
}

pub struct CaptureOptions {
    pub exposure: Option<f64>,
    pub exposure_seconds: Option<f64>,
    pub gain: Option<f64>,
    pub frames_averaged: usize,
    pub scale: f64,
    pub fmt: String,
    pub folder: PathBuf,
    pub name: String,
    pub prefix: String,
    pub extra: Map<String, Value>,
}

/// Save an image (scaled by `scale`, if not 1) plus a JSON sidecar. Returns the image path.
pub fn save_capture(image: &RgbImage, o: &CaptureOptions) -> Result<PathBuf, String> {
    std::fs::create_dir_all(&o.folder).map_err(|e| format!("Can't make {}: {e}", o.folder.display()))?;
    let now = Local::now();
    let path = next_capture_path(&o.folder, &o.name, &o.fmt, &o.prefix);
    let (sw, sh) = image.dimensions();
    let (w, h) = scaled_size(sw, sh, o.scale);
    let scaled;
    let to_save = if (w, h) != (sw, sh) {
        scaled = ip::resize_rgb(image, w, h, Interp::Cubic); // bicubic: good for scaling up
        &scaled
    } else {
        image
    };
    save_image(&path, to_save)?;
    let mut meta = json!({
        "timestamp": now.format("%Y-%m-%dT%H:%M:%S").to_string(),
        "source_size": [sw, sh],
        "saved_size": [w, h],
        "upscaled": (w, h) != (sw, sh),
        "exposure_value": o.exposure,
        "exposure_seconds_approx": o.exposure_seconds,
        "gain": o.gain,
        "frames_averaged": o.frames_averaged,
        "calibration": null,
    });
    for (k, v) in &o.extra {
        meta[k] = v.clone();
    }
    write_sidecar(&path, &meta)?;
    Ok(path)
}

/// The average of frames (rounded), as 8-bit.
pub fn average(frames: &[RgbImage]) -> RgbImage {
    let (w, h) = frames[0].dimensions();
    let mut sum = vec![0f32; (w * h * 3) as usize];
    for f in frames {
        for (s, &v) in sum.iter_mut().zip(f.as_raw()) {
            *s += v as f32;
        }
    }
    let n = frames.len() as f32;
    RgbImage::from_raw(w, h, sum.iter().map(|&s| (s / n + 0.5).clamp(0.0, 255.0) as u8).collect()).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_folder(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("camcontrol-rs-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn clean_name_replaces_bad_characters() {
        assert_eq!(clean_name("  a/b:c*d?. "), "a_b_c_d_");
        assert_eq!(clean_name("fiber 3..."), "fiber 3");
        assert_eq!(clean_name("ok\tname"), "ok_name");
    }

    #[test]
    fn numbered_names_continue_after_highest() {
        let dir = temp_folder("numbers");
        assert_eq!(next_capture_path(&dir, "s", "tif", "cap"), dir.join("s-001.tif"));
        for f in ["s-001.tif", "S-007.PNG", "s-002.json", "s-x.tif", "s-003.tif.bak", "other-050.tif"] {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        assert_eq!(next_capture_path(&dir, "s", "png", "cap"), dir.join("s-008.png"));
        assert_eq!(next_capture_path(&dir, "new", "tif", "cap"), dir.join("new-001.tif"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unnamed_captures_never_overwrite() {
        let dir = temp_folder("unnamed");
        let first = next_capture_path(&dir, "", "tif", "vid");
        assert!(first.file_name().unwrap().to_string_lossy().starts_with("vid_"));
        std::fs::write(&first, b"").unwrap();
        assert_ne!(first, next_capture_path(&dir, "", "tif", "vid"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn saves_scaled_capture_and_sidecar() {
        let dir = temp_folder("save");
        let o = CaptureOptions {
            exposure: Some(-6.0), exposure_seconds: Some(0.156), gain: Some(48.0), frames_averaged: 4, scale: HD2_SCALE,
            fmt: "png".into(), folder: dir.join("sub"), name: "x".into(), prefix: "cap".into(), extra: Map::new(),
        };
        let path = save_capture(&RgbImage::new(192, 108), &o).unwrap();
        assert_eq!(path, dir.join("sub").join("x-001.png"));
        assert_eq!(load_image(&path).unwrap().dimensions(), (326, 184));
        let meta: Value = serde_json::from_str(&std::fs::read_to_string(path.with_extension("json")).unwrap()).unwrap();
        assert_eq!(meta["frames_averaged"], 4);
        assert_eq!(meta["upscaled"], true);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
