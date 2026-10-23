//! Still capture: file names, saving with the capture's details (exposure, gain, ...)
//! written into the image itself, averaging frames, and reading/writing image files
//! (same rules as camcontrol/capture.py, which writes the details to a .json beside the image).
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
    save_image_with(path, img, None)
}

/// The details as JSON text that's plain ASCII (TIFF text fields must be): anything else as \uXXXX.
fn ascii_json(meta: &Value) -> String {
    let mut out = String::new();
    for c in serde_json::to_string(meta).unwrap_or_default().chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            let mut units = [0u16; 2];
            for u in c.encode_utf16(&mut units) {
                out += &format!("\\u{u:04x}");
            }
        }
    }
    out
}

/// Write an image with the capture's details (`meta`, as JSON text) inside it: a TIFF's
/// ImageDescription field, a PNG "Description" text chunk, or a JPEG comment
/// (a BMP can't hold them). Image tools such as ImageJ/Fiji or exiftool show them.
pub fn save_image_with(path: &Path, img: &RgbImage, meta: Option<&Value>) -> Result<(), String> {
    use std::io::Write;
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let text = meta.map(ascii_json);
    let create = || std::fs::File::create(path).map(std::io::BufWriter::new).map_err(|e| e.to_string());
    let result = match ext.as_str() {
        "jpg" | "jpeg" => (|| {
            let mut data = Vec::new();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut data, 95).encode_image(img).map_err(|e| e.to_string())?;
            if let Some(t) = &text {
                // A comment segment (FF FE, length) right after the start marker; at most 64 KB.
                let bytes = &t.as_bytes()[..t.len().min(65000)];
                let len = (bytes.len() + 2) as u16;
                let mut seg = vec![0xFF, 0xFE, (len >> 8) as u8, len as u8];
                seg.extend_from_slice(bytes);
                data.splice(2..2, seg);
            }
            create()?.write_all(&data).map_err(|e| e.to_string())
        })(),
        "png" => (|| {
            let mut enc = png::Encoder::new(create()?, img.width(), img.height());
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            if let Some(t) = &text {
                enc.add_itxt_chunk("Description".into(), t.clone()).map_err(|e| e.to_string())?;
            }
            let mut w = enc.write_header().map_err(|e| e.to_string())?;
            w.write_image_data(img.as_raw()).map_err(|e| e.to_string())?;
            w.finish().map_err(|e| e.to_string())
        })(),
        "bmp" => img.save(path).map_err(|e| e.to_string()),
        _ => (|| {
            let mut enc = tiff::encoder::TiffEncoder::new(create()?).map_err(|e| e.to_string())?;
            let mut image = enc.new_image::<tiff::encoder::colortype::RGB8>(img.width(), img.height()).map_err(|e| e.to_string())?;
            if let Some(t) = &text {
                image.encoder().write_tag(tiff::tags::Tag::ImageDescription, t.as_str()).map_err(|e| e.to_string())?;
            }
            image.write_data(img.as_raw()).map_err(|e| e.to_string())
        })(),
    };
    result.map_err(|e| format!("Could not save {}: {e}", path.display()))
}

/// The saved details in one line for the status bar, e.g. "2026-10-20 18:17 · 15 ms · gain 30 · 4 frames".
pub fn details_summary(meta: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(t) = meta["timestamp"].as_str() {
        parts.push(t.replace('T', " "));
    }
    if let Some(s) = meta["exposure_seconds_approx"].as_f64() {
        let ms = s * 1000.0;
        parts.push(if ms >= 1000.0 { format!("exposure {} s", (s * 100.0).round() / 100.0) } else { format!("exposure {} ms", (ms * 100.0).round() / 100.0) });
    }
    if let Some(g) = meta["gain"].as_f64() {
        parts.push(format!("gain {g}"));
    }
    if let Some(n) = meta["frames_averaged"].as_u64().filter(|&n| n > 1) {
        parts.push(format!("{n} frames averaged"));
    }
    if let Some(n) = meta["frames_summed_per_exposure"].as_u64().filter(|&n| n > 1) {
        parts.push(format!("{n} frames summed per exposure"));
    }
    if meta["upscaled"].as_bool() == Some(true) {
        parts.push("HD2 size".into());
    }
    if let Some(f) = meta["flat_field"].as_str() {
        parts.push(format!("flat-field {f}"));
    }
    if let Some(f) = meta["video_frame"].as_u64() {
        parts.push(format!("video frame {f}"));
    }
    if let Some(s) = meta["source"].as_str().filter(|s| !s.is_empty()) {
        parts.push(format!("from {s}"));
    }
    parts.join("  ·  ")
}

/// The capture details saved inside an image (see save_image_with); for images saved
/// before that (or by the Python app), the .json file beside it.
pub fn read_image_details(path: &Path) -> Option<Value> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let text: Option<String> = match ext.as_str() {
        "tif" | "tiff" => (|| {
            let file = std::io::BufReader::new(std::fs::File::open(path).ok()?);
            tiff::decoder::Decoder::new(file).ok()?.get_tag_ascii_string(tiff::tags::Tag::ImageDescription).ok()
        })(),
        "png" => (|| {
            let file = std::io::BufReader::new(std::fs::File::open(path).ok()?);
            let reader = png::Decoder::new(file).read_info().ok()?;
            let info = reader.info();
            info.utf8_text.iter().find(|c| c.keyword == "Description").and_then(|c| c.get_text().ok())
                .or_else(|| info.uncompressed_latin1_text.iter().find(|c| c.keyword == "Description").map(|c| c.text.clone()))
        })(),
        "jpg" | "jpeg" => (|| {
            let data = std::fs::read(path).ok()?;
            let mut i = 2;
            while i + 4 <= data.len() && data[i] == 0xFF {
                let (marker, len) = (data[i + 1], ((data[i + 2] as usize) << 8) | data[i + 3] as usize);
                if marker == 0xFE {
                    return data.get(i + 4..i + 2 + len).map(|b| String::from_utf8_lossy(b).into_owned());
                }
                if marker == 0xDA {
                    break; // image data: no more headers
                }
                i += 2 + len;
            }
            None
        })(),
        _ => None,
    };
    text.and_then(|t| serde_json::from_str::<Value>(&t).ok()).filter(Value::is_object).or_else(|| {
        serde_json::from_str(&std::fs::read_to_string(path.with_extension("json")).ok()?).ok()
    })
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

/// Save an image (scaled by `scale`, if not 1) with its details inside. Returns the image path.
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
    save_image_with(&path, to_save, Some(&meta))?;
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
    fn saves_scaled_capture_with_its_details() {
        let dir = temp_folder("save");
        let o = CaptureOptions {
            exposure: Some(-6.0), exposure_seconds: Some(0.156), gain: Some(48.0), frames_averaged: 4, scale: HD2_SCALE,
            fmt: "png".into(), folder: dir.join("sub"), name: "x".into(), prefix: "cap".into(), extra: Map::new(),
        };
        let path = save_capture(&RgbImage::new(192, 108), &o).unwrap();
        assert_eq!(path, dir.join("sub").join("x-001.png"));
        assert_eq!(load_image(&path).unwrap().dimensions(), (326, 184));
        assert!(!path.with_extension("json").exists(), "no .json beside it any more");
        let meta = read_image_details(&path).unwrap();
        assert_eq!(meta["frames_averaged"], 4);
        assert_eq!(meta["upscaled"], true);
        // Every format that can hold them, with text that isn't plain ASCII.
        let img = RgbImage::from_fn(40, 30, |x, y| image::Rgb([x as u8 * 6, y as u8 * 8, 100]));
        let meta = json!({"gain": 30.0, "calibration": {"unit": "µm", "per_pixel": 0.5}});
        for ext in ["tif", "png", "jpg"] {
            let p = dir.join(format!("d.{ext}"));
            save_image_with(&p, &img, Some(&meta)).unwrap();
            assert_eq!(read_image_details(&p).as_ref(), Some(&meta), "{ext}");
            assert_eq!(load_image(&p).unwrap().dimensions(), (40, 30), "{ext}");
            if ext != "jpg" {
                assert_eq!(load_image(&p).unwrap(), img, "{ext}: pixels unchanged");
            }
        }
        // Older captures: the .json beside them still counts.
        let old = dir.join("old.tif");
        save_image(&old, &img).unwrap();
        std::fs::write(old.with_extension("json"), r#"{"gain": 12}"#).unwrap();
        assert_eq!(read_image_details(&old).unwrap()["gain"], 12);
        let summary = details_summary(&json!({"timestamp": "2026-10-20T18:17:20", "exposure_seconds_approx": 0.015, "gain": 30.0,
                                              "frames_averaged": 4, "upscaled": false}));
        assert_eq!(summary, "2026-10-20 18:17:20  ·  exposure 15 ms  ·  gain 30  ·  4 frames averaged");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
