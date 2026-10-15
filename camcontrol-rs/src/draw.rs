//! Drawing shapes and labels: on screen (the GUI implements Canvas with egui)
//! and onto images for export (SkiaCanvas, anti-aliased with tiny-skia). The
//! measurement, counting and fiber drawings are written once against Canvas.

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use image::{Rgb, RgbImage};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Stroke, StrokeDash, Transform};

pub type P = (f32, f32);
/// RGBA, not premultiplied.
pub type Color = [u8; 4];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Dash {
    Solid,
    Dash,
    Dot,
}

pub trait Canvas {
    fn polyline(&mut self, pts: &[P], closed: bool, color: Color, width: f32, dash: Dash, fill: Option<Color>);
    fn ellipse(&mut self, c: P, rx: f32, ry: f32, color: Option<Color>, width: f32, dash: Dash, fill: Option<Color>);
    fn fill_rect(&mut self, min: P, max: P, color: Color);
    /// Text with its top-left corner at pos.
    fn text(&mut self, pos: P, text: &str, px: f32, color: Color);
    fn text_width(&self, text: &str, px: f32) -> f32;

    fn line(&mut self, a: P, b: P, color: Color, width: f32, dash: Dash) {
        self.polyline(&[a, b], false, color, width, dash, None);
    }
}

/// Text in a dark box just below-right of pos, readable on any image.
pub fn label(c: &mut dyn Canvas, pos: P, text: &str, color: Color, font_px: f32) {
    let pad = font_px * 0.25;
    let offset = font_px * 0.5;
    let (w, h) = (c.text_width(text, font_px) + 2.0 * pad, font_px * 1.2 + 2.0 * pad);
    let min = (pos.0 + offset, pos.1 + offset);
    c.fill_rect(min, (min.0 + w, min.1 + h), [0, 0, 0, 170]);
    c.text((min.0 + pad, min.1 + pad + font_px * 0.1), text, font_px, color);
}

pub fn font() -> FontRef<'static> {
    FontRef::try_from_slice(epaint_default_fonts::UBUNTU_LIGHT).expect("built-in font")
}

pub fn measure_text(text: &str, px: f32) -> f32 {
    let f = font();
    let s = f.as_scaled(PxScale::from(px));
    let mut w = 0.0;
    let mut last = None;
    for ch in text.chars() {
        let id = s.glyph_id(ch);
        if let Some(prev) = last {
            w += s.kern(prev, id);
        }
        w += s.h_advance(id);
        last = Some(id);
    }
    w
}

/// Draws onto a copy of an image, for exports.
pub struct SkiaCanvas {
    pub pixmap: Pixmap,
}

impl SkiaCanvas {
    pub fn from_image(img: &RgbImage) -> Self {
        let mut pixmap = Pixmap::new(img.width().max(1), img.height().max(1)).unwrap();
        for (d, s) in pixmap.data_mut().chunks_exact_mut(4).zip(img.pixels()) {
            d.copy_from_slice(&[s.0[0], s.0[1], s.0[2], 255]);
        }
        Self { pixmap }
    }

    pub fn to_image(&self) -> RgbImage {
        let (w, h) = (self.pixmap.width(), self.pixmap.height());
        let mut out = RgbImage::new(w, h);
        for (d, s) in out.pixels_mut().zip(self.pixmap.data().chunks_exact(4)) {
            *d = Rgb([s[0], s[1], s[2]]); // opaque, so not premultiplied in effect
        }
        out
    }

    fn paint(color: Color) -> Paint<'static> {
        let mut p = Paint::default();
        p.set_color_rgba8(color[0], color[1], color[2], color[3]);
        p.anti_alias = true;
        p
    }

    fn stroke(width: f32, dash: Dash) -> Stroke {
        let mut s = Stroke { width, ..Stroke::default() };
        s.dash = match dash {
            Dash::Solid => None,
            Dash::Dash => StrokeDash::new(vec![4.0 * width, 2.0 * width], 0.0),
            Dash::Dot => StrokeDash::new(vec![width, 2.0 * width], 0.0),
        };
        s
    }
}

impl Canvas for SkiaCanvas {
    fn polyline(&mut self, pts: &[P], closed: bool, color: Color, width: f32, dash: Dash, fill: Option<Color>) {
        if pts.len() < 2 {
            return;
        }
        let mut pb = PathBuilder::new();
        pb.move_to(pts[0].0, pts[0].1);
        for p in &pts[1..] {
            pb.line_to(p.0, p.1);
        }
        if closed {
            pb.close();
        }
        let Some(path) = pb.finish() else { return };
        if let Some(f) = fill {
            self.pixmap.fill_path(&path, &Self::paint(f), FillRule::Winding, Transform::identity(), None);
        }
        self.pixmap.stroke_path(&path, &Self::paint(color), &Self::stroke(width, dash), Transform::identity(), None);
    }

    fn ellipse(&mut self, c: P, rx: f32, ry: f32, color: Option<Color>, width: f32, dash: Dash, fill: Option<Color>) {
        let Some(rect) = tiny_skia::Rect::from_xywh(c.0 - rx, c.1 - ry, 2.0 * rx, 2.0 * ry) else { return };
        let Some(path) = PathBuilder::from_oval(rect) else { return };
        if let Some(f) = fill {
            self.pixmap.fill_path(&path, &Self::paint(f), FillRule::Winding, Transform::identity(), None);
        }
        if let Some(col) = color {
            self.pixmap.stroke_path(&path, &Self::paint(col), &Self::stroke(width, dash), Transform::identity(), None);
        }
    }

    fn fill_rect(&mut self, min: P, max: P, color: Color) {
        if let Some(r) = tiny_skia::Rect::from_ltrb(min.0, min.1, max.0, max.1) {
            self.pixmap.fill_rect(r, &Self::paint(color), Transform::identity(), None);
        }
    }

    fn text(&mut self, pos: P, text: &str, px: f32, color: Color) {
        let f = font();
        let s = f.as_scaled(PxScale::from(px));
        let (w, h) = (self.pixmap.width() as i32, self.pixmap.height() as i32);
        let data = self.pixmap.data_mut();
        let mut x = pos.0;
        let baseline = pos.1 + s.ascent();
        let mut last = None;
        for ch in text.chars() {
            let id = s.glyph_id(ch);
            if let Some(prev) = last {
                x += s.kern(prev, id);
            }
            let glyph = id.with_scale_and_position(PxScale::from(px), ab_glyph::point(x, baseline));
            if let Some(outline) = f.outline_glyph(glyph) {
                let b = outline.px_bounds();
                outline.draw(|gx, gy, cov| {
                    let (px_, py_) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                    if px_ < 0 || py_ < 0 || px_ >= w || py_ >= h {
                        return;
                    }
                    let i = (py_ as usize * w as usize + px_ as usize) * 4;
                    let a = cov.clamp(0.0, 1.0) * color[3] as f32 / 255.0;
                    for k in 0..3 {
                        data[i + k] = (data[i + k] as f32 * (1.0 - a) + color[k] as f32 * a).round() as u8;
                    }
                });
            }
            x += s.h_advance(id);
            last = Some(id);
        }
    }

    fn text_width(&self, text: &str, px: f32) -> f32 {
        measure_text(text, px)
    }
}

// --- plain pixel drawing (no anti-aliasing), as cv2.circle with thickness -1 ----------

pub fn fill_circle_px(img: &mut RgbImage, cx: i64, cy: i64, r: i64, color: [u8; 3]) {
    for y in (cy - r).max(0)..=(cy + r).min(img.height() as i64 - 1) {
        for x in (cx - r).max(0)..=(cx + r).min(img.width() as i64 - 1) {
            if (x - cx).pow(2) + (y - cy).pow(2) <= r * r {
                img.put_pixel(x as u32, y as u32, Rgb(color));
            }
        }
    }
}

/// A ring of the given thickness (cv2.circle with thickness > 0, roughly).
pub fn ring_px(img: &mut RgbImage, cx: i64, cy: i64, r: i64, thickness: i64, color: [u8; 3]) {
    let (lo, hi) = (r as f64 - thickness as f64 / 2.0, r as f64 + thickness as f64 / 2.0);
    let reach = r + thickness;
    for y in (cy - reach).max(0)..=(cy + reach).min(img.height() as i64 - 1) {
        for x in (cx - reach).max(0)..=(cx + reach).min(img.width() as i64 - 1) {
            let d = (((x - cx).pow(2) + (y - cy).pow(2)) as f64).sqrt();
            if d >= lo && d <= hi {
                img.put_pixel(x as u32, y as u32, Rgb(color));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_a_label_on_an_image() {
        let img = RgbImage::from_pixel(200, 60, Rgb([255, 255, 255]));
        let mut c = SkiaCanvas::from_image(&img);
        label(&mut c, (0.0, 0.0), "12.34 px", [0, 255, 0, 255], 20.0);
        let out = c.to_image();
        let green = out.pixels().filter(|p| p.0[1] > 150 && p.0[0] < 100).count();
        assert!(green > 20, "{green}");
        assert!(measure_text("12.34 px", 20.0) > 40.0);
    }
}
