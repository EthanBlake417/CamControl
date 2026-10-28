//! How the View > Grid and View > Crosshair overlays look (overlay_settings.py).
//! Saved with the same keys as the Python app, so settings files work in both.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::draw::{Color, Dash};

/// name, label
pub const LINE_STYLES: [(&str, &str); 3] = [("solid", "Solid"), ("dash", "Dashed"), ("dot", "Dotted")];
pub const CROSSHAIR_ANCHORS: [(&str, &str); 3] = [
    ("image", "Image center (moves with the image)"),
    ("window", "Window center (stays put when zooming and panning)"),
    ("fiber", "Fiber center (follows the selected fiber, else fiber 1)"),
];

pub fn parse_color(s: &str) -> Option<[u8; 3]> {
    let h = s.strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

pub fn color_hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GridStyle {
    pub width: f64,
    pub color: String,
    pub opacity: i64,
    pub line: String,
    pub columns: i64,
    pub rows: i64,
    pub square: bool,
}

impl Default for GridStyle {
    fn default() -> Self {
        Self { width: 1.0, color: "#ffff00".into(), opacity: 100, line: "solid".into(), columns: 8, rows: 8, square: false }
    }
}

/// An axis colored by how close its angle is to 0: at most limits[i] degrees either way
/// (the smallest limit that fits) gives colors[i]; beyond them all, the axis's own color.
/// "Close" is to `aim`: 0° or 90°, either way (|angle| near it): usually the fast axis
/// aims at 0° and the slow axis at 90°.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AngleColors {
    pub on: bool,
    pub aim: f64,
    pub limits: [f64; 3],
    pub colors: [String; 3],
}

impl Default for AngleColors {
    fn default() -> Self {
        Self { on: true, aim: 0.0, limits: [0.5, 1.0, 2.0], colors: ["#00ff00".into(), "#ffff00".into(), "#ff8000".into()] }
    }
}

/// An axis's colors by angle, ready to draw: aim (degrees) and (limit, color), smallest limit first.
pub type AngleBands = (f64, [(f64, Color); 3]);

impl AngleColors {
    /// The defaults for the slow axis (aiming at 90°) and the fast axis (at 0°).
    pub fn for_axis(slow: bool) -> Self {
        Self { aim: if slow { 90.0 } else { 0.0 }, ..Self::default() }
    }

    /// Field by field, so one bad value doesn't lose the others. Saved before there was
    /// an aim (all off then): this axis's defaults.
    fn from_value(data: &Value, slow: bool) -> Self {
        let mut s = Self::for_axis(slow);
        if !data.is_object() || data.get("aim").is_none() {
            return s;
        }
        if let Some(a) = data["aim"].as_f64() {
            s.aim = if a >= 45.0 { 90.0 } else { 0.0 };
        }
        if let Some(on) = data["on"].as_bool() {
            s.on = on;
        }
        for i in 0..3 {
            if let Some(v) = data["limits"][i].as_f64().filter(|v| v.is_finite()) {
                s.limits[i] = v.clamp(0.0, 90.0);
            }
            if let Some(c) = data["colors"][i].as_str().filter(|c| parse_color(c).is_some()) {
                s.colors[i] = c.into();
            }
        }
        s
    }

    /// The aim and (limit, color with this opacity), smallest limit first; None when off.
    pub fn bands(&self, opacity: i64) -> Option<AngleBands> {
        let mut b: [(f64, Color); 3] = std::array::from_fn(|i| (self.limits[i], rgba(&self.colors[i], opacity)));
        b.sort_by(|x, y| x.0.total_cmp(&y.0));
        self.on.then_some((self.aim, b))
    }
}

/// name, label: which fiber axes are shown.
pub const SHOW_AXES: [(&str, &str); 3] = [("slow", "Slow"), ("fast", "Fast"), ("both", "Both")];

/// Colors of the fibers' slow and fast axis lines (and of their angles in the readout).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FiberLineStyle {
    /// Which axes are drawn (and read out): "both", "slow" or "fast".
    pub show_axes: String,
    pub slow_color: String,
    pub slow_opacity: i64,
    pub fast_color: String,
    pub fast_opacity: i64,
    pub slow_by_angle: AngleColors,
    pub fast_by_angle: AngleColors,
    /// The angles box in the view's top-left corner: text size on screen (px), "sans" or "mono", bold.
    pub readout_size: f64,
    pub readout_font: String,
    pub readout_bold: bool,
}

impl Default for FiberLineStyle {
    fn default() -> Self {
        Self {
            show_axes: "both".into(),
            slow_color: "#ff00c8".into(),
            slow_opacity: 100,
            fast_color: "#ffc800".into(),
            fast_opacity: 100,
            slow_by_angle: AngleColors::for_axis(true),
            fast_by_angle: AngleColors::for_axis(false),
            readout_size: 26.0,
            readout_font: "sans".into(),
            readout_bold: false,
        }
    }
}

impl FiberLineStyle {
    pub fn from_value(data: &Value) -> Self {
        let mut v = serde_json::to_value(Self::default()).unwrap();
        merge_fields(&mut v, data);
        for k in ["slow_opacity", "fast_opacity"] {
            if let Some(n) = v[k].as_f64() {
                v[k] = (n as i64).into();
            }
        }
        let mut s: Self = serde_json::from_value(v).unwrap_or_default();
        let d = Self::default();
        if parse_color(&s.slow_color).is_none() {
            s.slow_color = d.slow_color;
        }
        if parse_color(&s.fast_color).is_none() {
            s.fast_color = d.fast_color;
        }
        if !s.readout_size.is_finite() {
            s.readout_size = d.readout_size;
        }
        s.readout_size = s.readout_size.clamp(10.0, 80.0);
        if !["sans", "mono"].contains(&s.readout_font.as_str()) {
            s.readout_font = d.readout_font.clone();
        }
        s.slow_opacity = s.slow_opacity.clamp(5, 100);
        s.fast_opacity = s.fast_opacity.clamp(5, 100);
        if !SHOW_AXES.iter().any(|a| a.0 == s.show_axes) {
            s.show_axes = d.show_axes.clone();
        }
        s.slow_by_angle = AngleColors::from_value(&data["slow_by_angle"], true);
        s.fast_by_angle = AngleColors::from_value(&data["fast_by_angle"], false);
        s
    }

    pub fn slow(&self) -> Color {
        rgba(&self.slow_color, self.slow_opacity)
    }

    pub fn fast(&self) -> Color {
        rgba(&self.fast_color, self.fast_opacity)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CrosshairStyle {
    pub width: f64,
    pub color: String,
    pub opacity: i64,
    pub line: String,
    pub circle: i64,
    pub anchor: String,
}

impl Default for CrosshairStyle {
    fn default() -> Self {
        Self { width: 2.0, color: "#ff0000".into(), opacity: 100, line: "solid".into(), circle: 20, anchor: "image".into() }
    }
}

fn rgba(color: &str, opacity: i64) -> Color {
    let c = parse_color(color).unwrap_or([255, 255, 255]);
    [c[0], c[1], c[2], (opacity.clamp(0, 100) as f64 * 2.55).round() as u8]
}

fn dash(line: &str) -> Dash {
    match line {
        "dash" => Dash::Dash,
        "dot" => Dash::Dot,
        _ => Dash::Solid,
    }
}

/// Read fields that are there and valid; keep defaults for the rest.
fn merge_fields(base: &mut Value, data: &Value) {
    if let (Some(b), Some(d)) = (base.as_object_mut(), data.as_object()) {
        for (k, v) in d {
            if let Some(slot) = b.get_mut(k) {
                let ok = match slot {
                    Value::Number(_) => v.is_number(),
                    Value::String(_) => v.is_string(),
                    Value::Bool(_) => v.is_boolean(),
                    _ => false,
                };
                if ok {
                    *slot = v.clone();
                }
            }
        }
    }
}

impl GridStyle {
    pub fn from_value(data: &Value) -> Self {
        let mut v = serde_json::to_value(Self::default()).unwrap();
        merge_fields(&mut v, data);
        if let Some(n) = v["columns"].as_f64() {
            v["columns"] = (n as i64).into();
        }
        if let Some(n) = v["rows"].as_f64() {
            v["rows"] = (n as i64).into();
        }
        if let Some(n) = v["opacity"].as_f64() {
            v["opacity"] = (n as i64).into();
        }
        let mut s: Self = serde_json::from_value(v).unwrap_or_default();
        let d = Self::default();
        if !LINE_STYLES.iter().any(|l| l.0 == s.line) {
            s.line = d.line;
        }
        if parse_color(&s.color).is_none() {
            s.color = d.color;
        }
        s.width = s.width.clamp(0.5, 10.0);
        s.opacity = s.opacity.clamp(5, 100);
        s.columns = s.columns.clamp(1, 100);
        s.rows = s.rows.clamp(1, 100);
        s
    }

    pub fn rows_for(&self, w: u32, h: u32) -> i64 {
        if self.square { ((self.columns as f64 * h as f64 / w as f64).round() as i64).max(1) } else { self.rows }
    }

    pub fn rgba(&self) -> Color {
        rgba(&self.color, self.opacity)
    }

    pub fn dash(&self) -> Dash {
        dash(&self.line)
    }
}

impl CrosshairStyle {
    pub fn from_value(data: &Value) -> Self {
        let mut v = serde_json::to_value(Self::default()).unwrap();
        merge_fields(&mut v, data);
        for k in ["circle", "opacity"] {
            if let Some(n) = v[k].as_f64() {
                v[k] = (n as i64).into();
            }
        }
        let mut s: Self = serde_json::from_value(v).unwrap_or_default();
        let d = Self::default();
        if !LINE_STYLES.iter().any(|l| l.0 == s.line) {
            s.line = d.line;
        }
        if parse_color(&s.color).is_none() {
            s.color = d.color;
        }
        if !CROSSHAIR_ANCHORS.iter().any(|a| a.0 == s.anchor) {
            s.anchor = d.anchor;
        }
        s.width = s.width.clamp(0.5, 10.0);
        s.opacity = s.opacity.clamp(5, 100);
        s.circle = s.circle.clamp(0, 500);
        s
    }

    pub fn rgba(&self) -> Color {
        rgba(&self.color, self.opacity)
    }

    pub fn dash(&self) -> Dash {
        dash(&self.line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles_from_saved_settings() {
        let g = GridStyle::from_value(&serde_json::json!({"columns": 12.0, "color": "#00ff00", "line": "bogus", "width": 99}));
        assert_eq!((g.columns, g.color.as_str(), g.line.as_str(), g.width), (12, "#00ff00", "solid", 10.0));
        assert_eq!(GridStyle { square: true, columns: 16, ..Default::default() }.rows_for(1920, 1080), 9);
        let c = CrosshairStyle::from_value(&serde_json::json!({"anchor": "window", "circle": -5}));
        assert_eq!((c.anchor.as_str(), c.circle), ("window", 0));
        let f = FiberLineStyle::from_value(&serde_json::json!({"slow_color": "#0000ff", "fast_color": "red", "fast_opacity": 50.0}));
        assert_eq!((f.slow(), f.fast()), ([0, 0, 255, 255], [255, 200, 0, 127]));
        // On by default: the slow axis aiming at 90°, the fast at 0°.
        assert_eq!(f.slow_by_angle.bands(100).map(|b| b.0), Some(90.0));
        assert_eq!(f.fast_by_angle.bands(100).map(|b| b.0), Some(0.0));
        let saved = serde_json::to_value(FiberLineStyle {
            fast_by_angle: AngleColors { on: true, aim: 0.0, limits: [3.0, 0.2, f64::NAN], colors: ["#010203".into(), "bad".into(), "#ffffff".into()] },
            slow_by_angle: AngleColors { on: false, ..AngleColors::for_axis(true) },
            ..Default::default()
        }).unwrap();
        let back = FiberLineStyle::from_value(&saved);
        assert_eq!(back.fast_by_angle.limits, [3.0, 0.2, 2.0]);
        let (aim, b) = back.fast_by_angle.bands(50).unwrap();
        assert_eq!((aim, b.map(|x| x.0)), (0.0, [0.2, 2.0, 3.0]));
        assert_eq!(b[0].1, [255, 255, 0, 127]); // the bad color back to its default
        assert!(back.slow_by_angle.bands(100).is_none(), "turned off stays off");
        assert_eq!(FiberLineStyle::from_value(&serde_json::json!({"show_axes": "fast"})).show_axes, "fast");
        assert_eq!(FiberLineStyle::from_value(&serde_json::json!({"show_axes": "sideways"})).show_axes, "both");
        // Saved by the version before aims (off, no aim): the new defaults.
        let old = FiberLineStyle::from_value(&serde_json::json!({"slow_by_angle": {"on": false, "limits": [1, 2, 3]}}));
        assert_eq!(old.slow_by_angle, AngleColors::for_axis(true));
    }
}
