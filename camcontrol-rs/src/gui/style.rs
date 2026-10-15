//! How the View > Grid and View > Crosshair overlays look (overlay_settings.py).
//! Saved with the same keys as the Python app, so settings files work in both.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::draw::{Color, Dash};

/// name, label
pub const LINE_STYLES: [(&str, &str); 3] = [("solid", "Solid"), ("dash", "Dashed"), ("dot", "Dotted")];
pub const CROSSHAIR_ANCHORS: [(&str, &str); 2] = [
    ("image", "Image centre (moves with the image)"),
    ("window", "Window centre (stays put when zooming and panning)"),
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
    }
}
