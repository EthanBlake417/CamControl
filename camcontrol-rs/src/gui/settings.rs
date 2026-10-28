//! What the app remembers between runs (QSettings in the Python app), and
//! settings files (settings_file.py): the app's setup as a JSON file you can
//! name, open later, or copy to another PC. Same format as the Python app's.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::stripes::PanelLayout;
use super::style::{CrosshairStyle, FiberLineStyle, GridStyle};
use crate::processing::fiber_axis::{Detection, Region};

pub const MAX_RECENT: usize = 8;
/// 2: the bottom panel at least 300 px tall (its tabs fit without scrolling).
pub const LAYOUT_VERSION: u32 = 2;
pub const AVERAGE_CHOICES: [usize; 5] = [1, 4, 8, 16, 32];
pub const FORMATS: [(&str, &str); 2] = [("TIFF", "tif"), ("PNG", "png")];
pub const UNITS: [(&str, f64); 3] = [("seconds", 1.0), ("minutes", 60.0), ("hours", 3600.0)];

/// Video and time-lapse settings (record_panel.state()).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecordState {
    pub video_format: String,
    pub video_fps: u32,
    pub video_limit: u32,
    /// Draw the fiber axes and angle readout on recorded videos.
    pub draw_analysis: bool,
    pub tl_interval: f64,
    pub tl_unit: usize,
    pub tl_count: u32,
    pub tl_video: bool,
    pub tl_video_fps: u32,
    /// Draw the view's markup on photos and time-lapse images.
    pub photo_markup: bool,
    pub tl_markup: bool,
}

impl Default for RecordState {
    fn default() -> Self {
        Self { video_format: "MP4".into(), video_fps: 25, video_limit: 0, draw_analysis: true, tl_interval: 10.0, tl_unit: 0, tl_count: 60, tl_video: true, tl_video_fps: 10,
               photo_markup: false, tl_markup: false }
    }
}

impl RecordState {
    pub fn interval_seconds(&self) -> f64 {
        self.tl_interval * UNITS.get(self.tl_unit).map(|u| u.1).unwrap_or(1.0)
    }

    /// Fields from a saved dict that are there and valid.
    pub fn merge(&mut self, v: &Value) {
        if let Some(s) = v["video_format"].as_str() {
            if crate::video::VIDEO_FORMATS.iter().any(|f| f.0 == s) {
                self.video_format = s.into();
            }
        }
        let num = |k: &str| v[k].as_f64();
        if let Some(n) = num("video_fps") { self.video_fps = (n as u32).clamp(1, 60); }
        if let Some(n) = num("video_limit") { self.video_limit = (n as u32).min(24 * 3600); }
        if let Some(b) = v["draw_analysis"].as_bool() { self.draw_analysis = b; }
        if let Some(n) = num("tl_interval") { self.tl_interval = n.clamp(0.5, 100000.0); }
        if let Some(n) = num("tl_unit") { self.tl_unit = (n as usize).min(2); }
        if let Some(n) = num("tl_count") { self.tl_count = (n as u32).min(100000); }
        if let Some(b) = v["tl_video"].as_bool() { self.tl_video = b; }
        if let Some(n) = num("tl_video_fps") { self.tl_video_fps = (n as u32).clamp(1, 60); }
        if let Some(b) = v["photo_markup"].as_bool() { self.photo_markup = b; }
        if let Some(b) = v["tl_markup"].as_bool() { self.tl_markup = b; }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    pub folder: String,
    pub name: String,
    pub average_idx: usize,
    pub size_idx: usize,
    pub format_idx: usize,
    pub capture_tab: usize,
    pub gallery_count: usize,
    pub gallery_follow: bool,
    pub gallery_folder: String,
    pub fiber_method: String,
    pub fiber_advanced: bool,
    pub fiber_centre_only: bool,
    pub fiber_show_parts: bool,
    /// Axis lines outside the fibers only, to the edges of the picture (and the crosshair on a fiber outside it only).
    pub fiber_long_lines: bool,
    pub fiber_lines: FiberLineStyle,
    pub fiber_live: bool,
    pub fiber_log_record: bool,
    /// Alignment panel: readings per Take readings, region of interest, reference line (x0, y0, x1, y1).
    pub align_n: usize,
    pub align_region: Option<Region>,
    pub align_reference: Option<[f64; 4]>,
    /// The next fiber's number in the Alignment table, and counting up (else down).
    pub align_next: i64,
    pub align_ascending: bool,
    /// Fiber count panel: the top fiber's number, and counting up from it.
    pub count_first: i64,
    pub count_ascending: bool,
    /// Fiber count panel open: still show the axis lines, angles, region and reference line.
    pub count_show_axes: bool,
    /// The layout "Several views" goes back to.
    pub multi_layout: String,
    /// The tab last shown in the grid, crosshair and fiber axes window.
    pub overlay_tab: String,
    pub histogram_log: bool,
    pub histogram_colors: bool,
    pub fiber_detection: Detection,
    pub recording: RecordState,
    pub flat_path: String,
    pub flat_on: bool,
    pub grid: bool,
    pub crosshair: bool,
    pub grid_style: GridStyle,
    pub crosshair_style: CrosshairStyle,
    pub views: Vec<Value>,
    pub views_layout: String,
    pub views_link: bool,
    pub panels: PanelLayout,
    pub recent_settings: Vec<String>,
    /// camera label -> "WxH"
    pub resolutions: BTreeMap<String, String>,
    pub left_width: f32,
    pub right_width: f32,
    pub bottom_height: f32,
    /// Bumped when a new default layout should be applied once to saved settings.
    pub layout_version: u32,
}

impl Default for Saved {
    fn default() -> Self {
        Self {
            folder: default_capture_dir().display().to_string(),
            name: String::new(),
            average_idx: 0,
            size_idx: 0,
            format_idx: 0,
            capture_tab: 0,
            gallery_count: 12,
            gallery_follow: true,
            gallery_folder: String::new(),
            fiber_method: "auto".into(),
            fiber_advanced: false,
            fiber_centre_only: false,
            fiber_show_parts: true,
            fiber_long_lines: false,
            fiber_lines: FiberLineStyle::default(),
            fiber_live: false,
            fiber_log_record: false,
            align_n: 10,
            align_region: None,
            align_reference: None,
            align_next: 1,
            align_ascending: true,
            count_first: 1,
            count_ascending: true,
            count_show_axes: false,
            multi_layout: "grid".into(),
            overlay_tab: "grid".into(),
            histogram_log: false,
            histogram_colors: false,
            fiber_detection: Detection::default(),
            recording: RecordState::default(),
            flat_path: String::new(),
            flat_on: false,
            grid: false,
            crosshair: false,
            grid_style: GridStyle::default(),
            crosshair_style: CrosshairStyle::default(),
            views: Vec::new(),
            views_layout: "single".into(),
            views_link: true,
            panels: PanelLayout::default(),
            recent_settings: Vec::new(),
            resolutions: BTreeMap::new(),
            left_width: 300.0,
            right_width: 380.0,
            bottom_height: 300.0,
            layout_version: LAYOUT_VERSION,
        }
    }
}

/// Where captures go by default: Pictures/CamControl (the program folder may not be writable).
pub fn default_capture_dir() -> PathBuf {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_default();
    home.join("Pictures").join("CamControl")
}

pub fn default_settings_dir() -> PathBuf {
    default_capture_dir().join("settings")
}

pub fn flat_dir() -> PathBuf {
    default_capture_dir().join("flats")
}

pub fn save_settings_file(path: &Path, data: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut out = json!({"camcontrol_settings": 1, "saved": crate::export::now_iso()});
    if let (Some(o), Some(d)) = (out.as_object_mut(), data.as_object()) {
        for (k, v) in d {
            o.insert(k.clone(), v.clone());
        }
    }
    std::fs::write(path, serde_json::to_string_pretty(&out).unwrap()).map_err(|e| e.to_string())
}

pub fn load_settings_file(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let data: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if !data.is_object() || data.get("camcontrol_settings").is_none() {
        return Err("This isn't a CamControl settings file.".into());
    }
    Ok(data)
}

/// Put path first in the recent list (no duplicates, at most MAX_RECENT).
pub fn add_recent(recent: &[String], path: &Path) -> Vec<String> {
    let p = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let p = p.display().to_string().trim_start_matches(r"\\?\").to_string();
    let mut out = vec![p.clone()];
    out.extend(recent.iter().filter(|r| **r != p).take(MAX_RECENT - 1).cloned());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_files_round_trip() {
        let dir = std::env::temp_dir().join(format!("camcontrol-rs-settings-{}", std::process::id()));
        let path = dir.join("a.json");
        save_settings_file(&path, &json!({"output": {"name": "x"}})).unwrap();
        let v = load_settings_file(&path).unwrap();
        assert_eq!(v["output"]["name"], "x");
        assert_eq!(v["camcontrol_settings"], 1);
        std::fs::write(dir.join("b.json"), "{}").unwrap();
        assert!(load_settings_file(&dir.join("b.json")).is_err());
        let r = add_recent(&["q".into(), "r".into()], &path);
        assert_eq!(r.len(), 3);
        std::fs::remove_dir_all(&dir).unwrap();
        let mut rs = RecordState::default();
        rs.merge(&json!({"video_fps": 99, "tl_unit": 2, "video_format": "nope"}));
        assert_eq!((rs.video_fps, rs.tl_unit, rs.video_format.as_str()), (60, 2, "MP4"));
    }
}
