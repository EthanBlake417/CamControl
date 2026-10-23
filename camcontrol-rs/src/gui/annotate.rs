//! Drawing measurements, counting marks and fiber axes through draw::Canvas,
//! so the same code draws on screen and on exported images
//! (measure_draw.py, count_panel.draw_marks, fiber_panel.draw_fiber_axes).
//! `to_out` maps image pixels to wherever we're drawing.

use image::RgbImage;

use crate::calibration::Calibration;
use crate::draw::{label, Canvas, Color, Dash, SkiaCanvas, P};
use crate::measure::{compute, is_circle, summary, Measurement};
use crate::processing::count::{Counter, CLASS_COLORS};
use crate::processing::fiber_axis::{brightest_centre, FiberAxis};

pub const MEASURE_COLOR: Color = [0, 255, 0, 255];
pub const SELECTED_COLOR: Color = [255, 140, 0, 255];
pub const PREVIEW_COLOR: Color = [0, 200, 255, 255];
pub const SLOW_COLOR: Color = [255, 0, 200, 255]; // magenta: shows on blue, white and black
pub const FAST_COLOR: Color = [255, 200, 0, 255];
pub const READOUT_COLOR: Color = [0, 255, 0, 255];
const LINE_LENGTH: f64 = 1.25; // axis lines reach this many radii from the centre
/// Readout text height on screen (px); exports scale it with the image.
pub const READOUT_PX: f32 = 26.0;

/// How fiber axes are drawn (on screen, on exported copies and on videos).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FiberDrawing {
    pub show_parts: bool,
    /// Axis lines carried on (thinner) to the edges, to compare with the crosshair, grid or the image's own lines.
    pub long_lines: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct Style {
    pub line_width: f32,
    pub font_px: f32,
    /// Half-size of the square drawn at each clicked point.
    pub marker: f32,
}

impl Default for Style {
    fn default() -> Self {
        Self { line_width: 2.0, font_px: 14.0, marker: 3.0 }
    }
}

pub fn draw_measurement(c: &mut dyn Canvas, m: &Measurement, cal: &Calibration, to_out: &dyn Fn(f64, f64) -> P, zoom: f64,
                        color: Color, style: &Style) {
    let pts: Vec<P> = m.points.iter().map(|&(x, y)| to_out(x, y)).collect();
    if pts.is_empty() {
        return;
    }
    let results = compute(m, cal).ok();
    let w = style.line_width;
    let mut anchor = pts[0];
    match m.kind.as_str() {
        "line" | "polyline" | "angle" => {
            if pts.len() >= 2 {
                c.polyline(&pts, false, color, w, Dash::Solid, None);
            }
            if m.kind == "line" && pts.len() >= 2 {
                anchor = ((pts[0].0 + pts[1].0) / 2.0, (pts[0].1 + pts[1].1) / 2.0);
            } else if m.kind == "angle" && pts.len() >= 2 {
                anchor = pts[1];
            }
        }
        "polygon" => {
            if pts.len() >= 3 {
                c.polyline(&pts, true, color, w, Dash::Solid, None);
            } else if pts.len() == 2 {
                c.polyline(&pts, false, color, w, Dash::Solid, None);
            }
        }
        "rectangle" if pts.len() == 2 => {
            let ((x0, y0), (x1, y1)) = (m.points[0], m.points[1]);
            let corners: Vec<P> = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)].iter().map(|&(x, y)| to_out(x, y)).collect();
            c.polyline(&corners, true, color, w, Dash::Solid, None);
            anchor = (corners.iter().map(|p| p.0).fold(f32::INFINITY, f32::min), corners.iter().map(|p| p.1).fold(f32::INFINITY, f32::min));
        }
        k if is_circle(k) => {
            if k == "circle_centre" && pts.len() == 2 {
                c.line(pts[0], pts[1], color, w, Dash::Solid); // the radius being drawn out
            }
            if let Some(r) = &results {
                let (cx, cy) = r.center_px.unwrap();
                let centre = to_out(cx, cy);
                // With non-square pixels the circle is an ellipse in pixels.
                let rad = r.radius.unwrap();
                c.ellipse(centre, (rad / cal.x * zoom) as f32, (rad / cal.y * zoom) as f32, Some(color), w, Dash::Solid, None);
                let k = style.marker * 2.0;
                c.line((centre.0 - k, centre.1), (centre.0 + k, centre.1), color, w, Dash::Solid);
                c.line((centre.0, centre.1 - k), (centre.0, centre.1 + k), color, w, Dash::Solid);
                anchor = centre;
            } else if pts.len() >= 2 {
                c.polyline(&pts, false, color, w, Dash::Solid, None);
            }
        }
        _ => {}
    }
    let k = style.marker;
    for p in &pts {
        let sq = [(p.0 - k, p.1 - k), (p.0 + k, p.1 - k), (p.0 + k, p.1 + k), (p.0 - k, p.1 + k)];
        c.polyline(&sq, true, color, w, Dash::Solid, None);
    }
    if let Some(r) = &results {
        let mut text = summary(m, r, cal);
        if m.id > 0 {
            text = format!("{}: {text}", m.id);
        }
        label(c, anchor, &text, color, style.font_px);
    }
}

pub fn class_color(cls: usize) -> Color {
    let c = CLASS_COLORS[cls % CLASS_COLORS.len()];
    [c[0], c[1], c[2], 255]
}

/// A filled dot per mark, in its class color with a dark outline.
pub fn draw_marks(c: &mut dyn Canvas, counter: &Counter, to_out: &dyn Fn(f64, f64) -> P, radius: f32) {
    for m in &counter.marks {
        c.ellipse(to_out(m.x, m.y), radius, radius, Some([0, 0, 0, 255]), (radius / 4.0).max(1.0), Dash::Solid, Some(class_color(m.cls)));
    }
}

pub fn signed_angle(deg: f64) -> f64 {
    let a = (deg + 90.0).rem_euclid(180.0) - 90.0;
    if a == -90.0 { 90.0 } else { a }
}

pub fn fmt_angle(deg: f64) -> String {
    // Python's round(): halves go to the even neighbour (-6.25 -> -6.2).
    let x = signed_angle(deg) * 10.0;
    let r = if (x - x.trunc()).abs() == 0.5 { 2.0 * (x / 2.0).round() } else { x.round() };
    let a = r / 10.0;
    format!("{:+.1}", if a == 0.0 { 0.0 } else { a }) // no "-0.0"
}

/// `reach`: how far long lines go from the pivot (in output units; past the edges is fine).
pub fn draw_fiber_axes(c: &mut dyn Canvas, axes: &[FiberAxis], to_out: &dyn Fn(f64, f64) -> P, zoom: f64, style: &Style,
                       selected: Option<usize>, drawing: FiberDrawing, reach: f32) {
    let show_parts = drawing.show_parts;
    let brightest = brightest_centre(axes).map(|b| b.0);
    for (i, f) in axes.iter().enumerate() {
        let centre = to_out(f.cx, f.cy);
        let (ax, ay) = f.axis_point();
        let pivot = to_out(ax, ay);
        let r = (f.radius * zoom) as f32;
        let width = style.line_width * if Some(i) == selected { 1.6 } else { 1.0 };
        if show_parts {
            let mut fill = SLOW_COLOR;
            fill[3] = 45;
            for outline in &f.parts {
                let pts: Vec<P> = outline.iter().map(|&(x, y)| to_out(x, y)).collect();
                c.polyline(&pts, true, SLOW_COLOR, (width / 2.0).max(1.0), Dash::Dot, Some(fill));
            }
            let k = style.marker * 1.5;
            for &(x, y) in &f.part_centres {
                let p = to_out(x, y);
                c.line((p.0 - k, p.1 - k), (p.0 + k, p.1 + k), SLOW_COLOR, width, Dash::Solid);
                c.line((p.0 - k, p.1 + k), (p.0 + k, p.1 - k), SLOW_COLOR, width, Dash::Solid);
            }
        }
        c.ellipse(centre, r, r, Some([255, 255, 255, 160]), (width / 2.0).max(1.0), Dash::Dot, None);
        if drawing.long_lines {
            for (deg, mut color) in [(f.fast_deg(), FAST_COLOR), (f.slow_deg, SLOW_COLOR)] {
                color[3] = 170;
                let a = deg.to_radians();
                let (dx, dy) = (a.cos() as f32 * reach, -a.sin() as f32 * reach);
                c.line((pivot.0 - dx, pivot.1 - dy), (pivot.0 + dx, pivot.1 + dy), color, (width / 2.0).max(1.0), Dash::Solid);
            }
        }
        if brightest == Some(i) {
            let k = (r * 0.25).max(style.marker * 3.0);
            c.ellipse(centre, k, k, Some(READOUT_COLOR), width, Dash::Solid, None);
        }
        for (deg, color, dash) in [(f.fast_deg(), FAST_COLOR, Dash::Dash), (f.slow_deg, SLOW_COLOR, Dash::Solid)] {
            let a = deg.to_radians();
            let (dx, dy) = ((a.cos() * r as f64 * LINE_LENGTH) as f32, (-a.sin() * r as f64 * LINE_LENGTH) as f32);
            c.line((pivot.0 - dx, pivot.1 - dy), (pivot.0 + dx, pivot.1 + dy), color, width, dash);
        }
        if show_parts {
            c.ellipse(pivot, style.marker * 1.5, style.marker * 1.5, Some([255, 255, 255, 255]), (width / 2.0).max(1.0), Dash::Solid, None);
        }
        // No tag on the fiber: they're numbered left to right in the readout (draw_fiber_readout).
    }
}

/// One readout line per fiber (numbered left to right): slow and fast axis angles, and "(unclear)" when unsure;
/// with several fibers, a last line saying which has the brightest centre.
pub fn fiber_readout_lines(axes: &[FiberAxis]) -> Vec<String> {
    let mut lines: Vec<String> = axes
        .iter()
        .enumerate()
        .map(|(i, f)| format!("{}: slow {}°  fast {}°{}", i + 1, fmt_angle(f.slow_deg), fmt_angle(f.fast_deg()), if f.clear() { "" } else { " (unclear)" }))
        .collect();
    if let Some((i, position)) = brightest_centre(axes) {
        lines.push(format!("Brightest centre: position {position} from right (fiber {})", i + 1));
    }
    lines
}

/// The angles in green, in a dark box with its top-left corner at `corner`.
pub fn draw_fiber_readout(c: &mut dyn Canvas, axes: &[FiberAxis], corner: P, font_px: f32) {
    let lines = fiber_readout_lines(axes);
    if lines.is_empty() {
        return;
    }
    let pad = font_px * 0.4;
    let step = font_px * 1.3;
    let w = lines.iter().map(|l| c.text_width(l, font_px)).fold(0.0, f32::max) + 2.0 * pad;
    let h = step * lines.len() as f32 + 2.0 * pad - (step - font_px * 1.15);
    c.fill_rect(corner, (corner.0 + w, corner.1 + h), [0, 0, 0, 170]);
    for (k, line) in lines.iter().enumerate() {
        c.text((corner.0 + pad, corner.1 + pad + k as f32 * step), line, font_px, READOUT_COLOR);
    }
}

fn identity(x: f64, y: f64) -> P {
    (x as f32, y as f32)
}

/// A full-resolution copy of the image with the measurements drawn on it.
pub fn render_annotated(img: &RgbImage, measurements: &[Measurement], cal: &Calibration) -> RgbImage {
    let w = img.width() as f32;
    let style = Style { line_width: (w / 800.0).max(2.0), font_px: (w / 70.0).round().max(14.0), marker: (w / 500.0).max(3.0) };
    let mut c = SkiaCanvas::from_image(img);
    for m in measurements {
        draw_measurement(&mut c, m, cal, &identity, 1.0, MEASURE_COLOR, &style);
    }
    c.to_image()
}

pub fn render_marks(img: &RgbImage, counter: &Counter) -> RgbImage {
    let mut c = SkiaCanvas::from_image(img);
    draw_marks(&mut c, counter, &identity, (img.width() as f32 / 300.0).max(5.0));
    c.to_image()
}

pub fn render_fiber_axes(img: &RgbImage, axes: &[FiberAxis], drawing: FiberDrawing) -> RgbImage {
    let w = img.width() as f32;
    let style = Style { line_width: (w / 600.0).max(2.0), font_px: (w / 60.0).round().max(14.0), marker: 3.0 };
    let mut c = SkiaCanvas::from_image(img);
    draw_fiber_axes(&mut c, axes, &identity, 1.0, &style, None, drawing, w + img.height() as f32);
    // Big, as on screen: READOUT_PX on a view about 1000 px wide.
    let font = (w / 1000.0 * READOUT_PX).round().max(18.0);
    let margin = font * 0.4;
    draw_fiber_readout(&mut c, axes, (margin, margin), font);
    c.to_image()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angles_read_as_signed() {
        assert_eq!(fmt_angle(179.96), "+0.0");
        assert_eq!(fmt_angle(173.75), "-6.2");
        assert_eq!(fmt_angle(90.0), "+90.0");
    }

    #[test]
    fn annotated_export_draws_something() {
        let img = RgbImage::new(400, 300);
        let m = Measurement { id: 1, kind: "circle_centre".into(), points: vec![(200.0, 150.0), (260.0, 150.0)] };
        let out = render_annotated(&img, &[m], &Calibration::pixels());
        assert!(out.pixels().filter(|p| p.0[1] > 200).count() > 100);
    }
}
