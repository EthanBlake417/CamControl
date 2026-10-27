//! Drawing measurements, counting marks and fiber axes through draw::Canvas,
//! so the same code draws on screen and on exported images
//! (measure_draw.py, count_panel.draw_marks, fiber_panel.draw_fiber_axes).
//! `to_out` maps image pixels to wherever we're drawing.

use image::RgbImage;

use crate::calibration::Calibration;
use super::style::AngleBands;
use crate::draw::{label, Canvas, Color, Dash, SkiaCanvas, P};
use crate::measure::{compute, is_circle, summary, Measurement};
use crate::processing::count::{Counter, CLASS_COLORS};
use crate::processing::fiber_axis::{brightest_centre, FiberAxis, Region};

pub const MEASURE_COLOR: Color = [0, 255, 0, 255];
pub const SELECTED_COLOR: Color = [255, 140, 0, 255];
pub const PREVIEW_COLOR: Color = [0, 200, 255, 255];
pub const SLOW_COLOR: Color = [255, 0, 200, 255]; // magenta: shows on blue, white and black
pub const FAST_COLOR: Color = [255, 200, 0, 255];
pub const READOUT_COLOR: Color = [0, 255, 0, 255];
pub const REGION_COLOR: Color = [255, 220, 0, 255];
pub const REFERENCE_COLOR: Color = [0, 220, 255, 255];
pub const NUMBER_COLOR: Color = [255, 255, 255, 255];
const LINE_LENGTH: f64 = 1.25; // axis lines reach this many radii from the center
/// Readout text height on screen (px); exports scale it with the image.
pub const READOUT_PX: f32 = 26.0;

/// How fiber axes are drawn (on screen, on exported copies and on videos).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FiberDrawing {
    pub show_parts: bool,
    /// Axis lines only outside the fiber, from its edge to the picture's edges, to compare with
    /// the crosshair, grid or the image's own lines without covering the fiber.
    pub long_lines: bool,
    /// Line colors (with opacity); the readout writes each axis's angle in its color.
    pub slow: Color,
    pub fast: Color,
    /// Which axes are drawn and read out.
    pub show_slow: bool,
    pub show_fast: bool,
    /// Colors by angle (see style::AngleColors): aim, then (limit in degrees, color), smallest first.
    pub slow_bands: Option<AngleBands>,
    pub fast_bands: Option<AngleBands>,
    /// Numbering: the first fiber (top, or left in a row) gets this number, the next ones count up or down.
    pub first_number: i64,
    pub ascending: bool,
    /// A reference line's angle: every angle shown is then measured from it.
    pub reference: Option<f64>,
}

impl Default for FiberDrawing {
    fn default() -> Self {
        Self { show_parts: false, long_lines: false, slow: SLOW_COLOR, fast: FAST_COLOR, show_slow: true, show_fast: true, slow_bands: None,
               fast_bands: None, first_number: 1, ascending: true, reference: None }
    }
}

impl FiberDrawing {
    /// An axis angle as shown: from the reference line if there is one, else from horizontal (-90 to +90).
    pub fn shown(&self, deg: f64) -> f64 {
        signed_angle(deg - self.reference.unwrap_or(0.0))
    }

    /// The number of the i-th fiber (in the order they're kept: top to bottom, then left to right).
    pub fn number(&self, i: usize) -> i64 {
        if self.ascending { self.first_number + i as i64 } else { self.first_number - i as i64 }
    }

    fn by_angle(base: Color, bands: &Option<AngleBands>, deg: f64) -> Color {
        let Some((aim, bands)) = bands else { return base };
        let off = (signed_angle(deg).abs() - aim).abs(); // from the aim, either way
        bands.iter().find(|(limit, _)| off <= *limit).map(|b| b.1).unwrap_or(base)
    }

    /// This fiber's slow axis color (its own, or by its angle).
    pub fn slow_color(&self, f: &FiberAxis) -> Color {
        Self::by_angle(self.slow, &self.slow_bands, self.shown(f.slow_deg))
    }

    pub fn fast_color(&self, f: &FiberAxis) -> Color {
        Self::by_angle(self.fast, &self.fast_bands, self.shown(f.fast_deg()))
    }
}

/// The parts of the line a..b outside every circle (center, radius).
pub fn outside_circles(a: P, b: P, circles: &[(P, f32)]) -> Vec<(P, P)> {
    let d = (b.0 - a.0, b.1 - a.1);
    let len2 = d.0 * d.0 + d.1 * d.1;
    if len2 <= 0.0 {
        return Vec::new();
    }
    // Where along a..b (0 to 1) each circle covers it.
    let mut covered: Vec<(f32, f32)> = circles
        .iter()
        .filter_map(|&((cx, cy), r)| {
            let (ox, oy) = (a.0 - cx, a.1 - cy);
            let half_b = (ox * d.0 + oy * d.1) / len2;
            let disc = half_b * half_b - (ox * ox + oy * oy - r * r) / len2;
            (disc > 0.0).then(|| (-half_b - disc.sqrt(), -half_b + disc.sqrt()))
        })
        .collect();
    covered.sort_by(|x, y| x.0.total_cmp(&y.0));
    let at = |t: f32| (a.0 + d.0 * t, a.1 + d.1 * t);
    let mut pieces = Vec::new();
    let mut from = 0.0f32;
    for (t0, t1) in covered {
        if t0 > from {
            pieces.push((at(from), at(t0.min(1.0))));
        }
        from = from.max(t1);
        if from >= 1.0 {
            break;
        }
    }
    if from < 1.0 {
        pieces.push((at(from), at(1.0)));
    }
    pieces.retain(|(p, q)| (q.0 - p.0).hypot(q.1 - p.1) > 0.5);
    pieces
}

/// The part of the line a..b inside the rectangle min..max (Liang-Barsky), if any.
fn clip_to_rect(a: P, b: P, min: P, max: P) -> Option<(P, P)> {
    let d = (b.0 - a.0, b.1 - a.1);
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for (p, q) in [(-d.0, a.0 - min.0), (d.0, max.0 - a.0), (-d.1, a.1 - min.1), (d.1, max.1 - a.1)] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let t = q / p;
            if p < 0.0 { t0 = t0.max(t) } else { t1 = t1.min(t) }
        }
    }
    (t0 < t1).then(|| ((a.0 + d.0 * t0, a.1 + d.1 * t0), (a.0 + d.0 * t1, a.1 + d.1 * t1)))
}

/// How far from `p` along `d` the rectangle min..max ends (0 if p is outside it).
#[allow(dead_code)]
fn reach_in_rect(p: P, d: (f32, f32), min: P, max: P) -> f32 {
    let mut t = f32::INFINITY;
    for (pv, dv, lo, hi) in [(p.0, d.0, min.0, max.0), (p.1, d.1, min.1, max.1)] {
        if pv < lo || pv > hi {
            return 0.0;
        }
        if dv > 1e-6 {
            t = t.min((hi - pv) / dv);
        } else if dv < -1e-6 {
            t = t.min((lo - pv) / dv);
        }
    }
    if t.is_finite() { t } else { 0.0 }
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
                let center = to_out(cx, cy);
                // With non-square pixels the circle is an ellipse in pixels.
                let rad = r.radius.unwrap();
                c.ellipse(center, (rad / cal.x * zoom) as f32, (rad / cal.y * zoom) as f32, Some(color), w, Dash::Solid, None);
                let k = style.marker * 2.0;
                c.line((center.0 - k, center.1), (center.0 + k, center.1), color, w, Dash::Solid);
                c.line((center.0, center.1 - k), (center.0, center.1 + k), color, w, Dash::Solid);
                anchor = center;
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

/// `area`: corners (output units) of where long lines end: the view on screen, the picture on exports.
pub fn draw_fiber_axes(c: &mut dyn Canvas, axes: &[FiberAxis], to_out: &dyn Fn(f64, f64) -> P, zoom: f64, style: &Style,
                       selected: Option<usize>, drawing: FiberDrawing, area: (P, P)) {
    let show_parts = drawing.show_parts;
    let brightest = brightest_centre(axes).map(|b| b.0);
    let circles: Vec<(P, f32)> = axes.iter().map(|f| (to_out(f.cx, f.cy), (f.radius * zoom) as f32)).collect();
    for (i, f) in axes.iter().enumerate() {
        let center = to_out(f.cx, f.cy);
        let (ax, ay) = f.axis_point();
        let pivot = to_out(ax, ay);
        let r = (f.radius * zoom) as f32;
        let width = style.line_width * if Some(i) == selected { 1.6 } else { 1.0 };
        if show_parts {
            let mut fill = drawing.slow;
            fill[3] = 45;
            for outline in &f.parts {
                let pts: Vec<P> = outline.iter().map(|&(x, y)| to_out(x, y)).collect();
                c.polyline(&pts, true, drawing.slow, (width / 2.0).max(1.0), Dash::Dot, Some(fill));
            }
            let k = style.marker * 1.5;
            for &(x, y) in &f.part_centres {
                let p = to_out(x, y);
                c.line((p.0 - k, p.1 - k), (p.0 + k, p.1 + k), drawing.slow, width, Dash::Solid);
                c.line((p.0 - k, p.1 + k), (p.0 + k, p.1 - k), drawing.slow, width, Dash::Solid);
            }
        }
        c.ellipse(center, r, r, Some([255, 255, 255, 160]), (width / 2.0).max(1.0), Dash::Dot, None);
        if brightest == Some(i) {
            let k = (r * 0.25).max(style.marker * 3.0);
            c.ellipse(center, k, k, Some(READOUT_COLOR), width, Dash::Solid, None);
        }
        let shown = [(drawing.show_fast, f.fast_deg(), drawing.fast_color(f), Dash::Dash), (drawing.show_slow, f.slow_deg, drawing.slow_color(f), Dash::Solid)];
        for (deg, color, dash) in shown.into_iter().filter(|a| a.0).map(|a| (a.1, a.2, a.3)) {
            let a = deg.to_radians();
            if drawing.long_lines {
                // Through the pivot to the edges of the area (as the crosshair), never over a fiber
                // (this one or its neighbours); still drawn when the pivot itself is out of view.
                let far = (area.1 .0 - area.0 .0).abs() + (area.1 .1 - area.0 .1).abs() + (pivot.0 - area.0 .0).abs() + (pivot.1 - area.0 .1).abs();
                let u = (a.cos() as f32 * far, -a.sin() as f32 * far);
                if let Some((p0, p1)) = clip_to_rect((pivot.0 - u.0, pivot.1 - u.1), (pivot.0 + u.0, pivot.1 + u.1), area.0, area.1) {
                    for (p, q) in outside_circles(p0, p1, &circles) {
                        c.line(p, q, color, width, dash);
                    }
                }
            } else {
                let (dx, dy) = ((a.cos() * r as f64 * LINE_LENGTH) as f32, (-a.sin() * r as f64 * LINE_LENGTH) as f32);
                c.line((pivot.0 - dx, pivot.1 - dy), (pivot.0 + dx, pivot.1 + dy), color, width, dash);
            }
        }
        if show_parts {
            c.ellipse(pivot, style.marker * 1.5, style.marker * 1.5, Some([255, 255, 255, 255]), (width / 2.0).max(1.0), Dash::Solid, None);
        }
        // No tag on the fiber: they're numbered left to right in the readout (draw_fiber_readout).
    }
}

/// The readout's lines as colored pieces: per fiber (numbered left to right) the slow and fast
/// axis angles in their line colors (opaque, to stay readable), "(unclear)" when unsure;
/// with several fibers, a last line saying which has the brightest center.
pub fn fiber_readout_pieces(axes: &[FiberAxis], drawing: &FiberDrawing) -> Vec<Vec<(String, Color)>> {
    let opaque = |c: Color| [c[0], c[1], c[2], 255];
    let mut lines: Vec<Vec<(String, Color)>> = axes
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let mut line = vec![
                (format!("slow {}°", fmt_angle(drawing.shown(f.slow_deg))), opaque(drawing.slow_color(f))),
                (format!("fast {}°", fmt_angle(drawing.shown(f.fast_deg()))), opaque(drawing.fast_color(f))),
            ];
            // Only the axes shown, the number in the first one's color.
            line.retain(|p| if p.0.starts_with("slow") { drawing.show_slow } else { drawing.show_fast });
            if line.len() == 2 {
                line[1].0.insert_str(0, "  ");
            }
            let first = line.first().map(|p| p.1).unwrap_or(READOUT_COLOR);
            line.insert(0, (format!("{}: ", drawing.number(i)), first));
            if !f.clear() {
                line.push((" (unclear)".into(), [230, 230, 230, 255]));
            }
            line
        })
        .collect();
    if let Some((i, _)) = brightest_centre(axes) {
        lines.push(vec![(format!("Fiber {} is on (brightest center)", drawing.number(i)), READOUT_COLOR)]);
    }
    if drawing.reference.is_some() && !axes.is_empty() {
        lines.push(vec![("Angles from the reference line".into(), REFERENCE_COLOR)]);
    }
    lines
}

/// The readout's lines as plain text.
pub fn fiber_readout_lines(axes: &[FiberAxis]) -> Vec<String> {
    fiber_readout_lines_with(axes, &FiberDrawing::default())
}

pub fn fiber_readout_lines_with(axes: &[FiberAxis], drawing: &FiberDrawing) -> Vec<String> {
    fiber_readout_pieces(axes, drawing).into_iter().map(|l| l.into_iter().map(|p| p.0).collect()).collect()
}

/// The angles in a dark box with its top-left corner at `corner`.
pub fn draw_fiber_readout(c: &mut dyn Canvas, axes: &[FiberAxis], drawing: &FiberDrawing, corner: P, font_px: f32) {
    let lines = fiber_readout_pieces(axes, drawing);
    if lines.is_empty() {
        return;
    }
    let pad = font_px * 0.4;
    let step = font_px * 1.3;
    let widths: Vec<Vec<f32>> = lines.iter().map(|l| l.iter().map(|p| c.text_width(&p.0, font_px)).collect()).collect();
    let w = widths.iter().map(|l| l.iter().sum::<f32>()).fold(0.0, f32::max) + 2.0 * pad;
    let h = step * lines.len() as f32 + 2.0 * pad - (step - font_px * 1.15);
    c.fill_rect(corner, (corner.0 + w, corner.1 + h), [0, 0, 0, 170]);
    for (k, (line, ws)) in lines.iter().zip(&widths).enumerate() {
        let mut x = corner.0 + pad;
        for ((text, color), w) in line.iter().zip(ws) {
            c.text((x, corner.1 + pad + k as f32 * step), text, font_px, *color);
            x += w;
        }
    }
}

/// What's drawn besides measurements and fiber axes: the region fibers are looked for in,
/// a reference line, the fibers' numbers, and notes (on saved copies, bottom left).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Extras {
    pub region: Option<Region>,
    pub reference: Option<((f64, f64), (f64, f64))>,
    pub numbers: bool,
    pub notes: Vec<String>,
}

pub fn draw_extras(c: &mut dyn Canvas, extras: &Extras, axes: &[FiberAxis], drawing: &FiberDrawing, to_out: &dyn Fn(f64, f64) -> P,
                   zoom: f64, style: &Style, area: (P, P)) {
    let w = style.line_width;
    match extras.region {
        Some(Region::Rect { x0, y0, x1, y1 }) => {
            let pts = [to_out(x0, y0), to_out(x1, y0), to_out(x1, y1), to_out(x0, y1)];
            c.polyline(&pts, true, REGION_COLOR, w, Dash::Dash, None);
        }
        Some(Region::Circle { cx, cy, r }) => {
            let r = (r * zoom) as f32;
            c.ellipse(to_out(cx, cy), r, r, Some(REGION_COLOR), w, Dash::Dash, None);
        }
        None => {}
    }
    if let Some((a, b)) = extras.reference {
        // Dashed across the whole picture, solid where it was drawn.
        let (p, q) = (to_out(a.0, a.1), to_out(b.0, b.1));
        let len = (q.0 - p.0).hypot(q.1 - p.1).max(1e-3);
        let far = (area.1 .0 - area.0 .0).abs() + (area.1 .1 - area.0 .1).abs() + (p.0 - area.0 .0).abs() + (p.1 - area.0 .1).abs();
        let u = ((q.0 - p.0) / len * far, (q.1 - p.1) / len * far);
        if let Some((s, e)) = clip_to_rect((p.0 - u.0, p.1 - u.1), (p.0 + u.0, p.1 + u.1), area.0, area.1) {
            c.line(s, e, REFERENCE_COLOR, w, Dash::Dash);
        }
        c.line(p, q, REFERENCE_COLOR, w * 1.5, Dash::Solid);
    }
    if extras.numbers {
        let on = brightest_centre(axes).map(|b| b.0);
        for (i, f) in axes.iter().enumerate() {
            let k = std::f64::consts::FRAC_1_SQRT_2 * f.radius;
            let (text, color) = if on == Some(i) { (format!("{} on", drawing.number(i)), READOUT_COLOR) } else { (drawing.number(i).to_string(), NUMBER_COLOR) };
            label(c, to_out(f.cx + k, f.cy - k), &text, color, style.font_px * 1.4);
        }
    }
}

/// Notes in a dark box with its bottom-left corner at `corner`; tab-separated parts of a
/// line line up in columns (a table).
pub fn draw_notes(c: &mut dyn Canvas, notes: &[String], corner: P, font_px: f32) {
    if notes.is_empty() {
        return;
    }
    let pad = font_px * 0.4;
    let step = font_px * 1.3;
    let gap = font_px * 1.2;
    let rows: Vec<Vec<&str>> = notes.iter().map(|l| l.split('\t').collect()).collect();
    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(1);
    let mut widths = vec![0f32; cols];
    for r in rows.iter().filter(|r| r.len() > 1) {
        for (k, cell) in r.iter().enumerate() {
            widths[k] = widths[k].max(c.text_width(cell, font_px));
        }
    }
    let table: f32 = widths.iter().sum::<f32>() + gap * (cols as f32 - 1.0);
    let single = rows.iter().filter(|r| r.len() == 1).map(|r| c.text_width(r[0], font_px)).fold(0.0, f32::max);
    let w = table.max(single) + 2.0 * pad;
    let h = step * rows.len() as f32 + 2.0 * pad - (step - font_px * 1.15);
    let top = corner.1 - h;
    c.fill_rect((corner.0, top), (corner.0 + w, corner.1), [0, 0, 0, 190]);
    for (k, r) in rows.iter().enumerate() {
        let y = top + pad + k as f32 * step;
        let mut x = corner.0 + pad;
        for (j, cell) in r.iter().enumerate() {
            c.text((x, y), cell, font_px, [255, 255, 255, 255]);
            x += widths[j].max(if r.len() == 1 { 0.0 } else { widths[j] }) + gap;
        }
    }
}

/// Everything drawn on a view, to burn into a saved copy or a recording.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Markup {
    pub axes: Vec<FiberAxis>,
    pub drawing: FiberDrawing,
    pub measurements: Vec<Measurement>,
    pub calibration: Option<Calibration>,
    pub extras: Extras,
}

impl Markup {
    pub fn is_empty(&self) -> bool {
        self.axes.is_empty() && self.measurements.is_empty() && self.extras.region.is_none() && self.extras.reference.is_none()
            && self.extras.notes.is_empty()
    }
}

/// A full-resolution copy of the image with the markup drawn on it.
pub fn render_markup(img: &RgbImage, m: &Markup) -> RgbImage {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let style = Style { line_width: (w / 600.0).max(2.0), font_px: (w / 60.0).round().max(14.0), marker: 3.0 };
    let mut c = SkiaCanvas::from_image(img);
    let cal = m.calibration.clone().unwrap_or_else(Calibration::pixels);
    for meas in &m.measurements {
        draw_measurement(&mut c, meas, &cal, &identity, 1.0, MEASURE_COLOR, &style);
    }
    let area = ((0.0, 0.0), (w, h));
    if !m.axes.is_empty() {
        draw_fiber_axes(&mut c, &m.axes, &identity, 1.0, &style, None, m.drawing, area);
    }
    draw_extras(&mut c, &m.extras, &m.axes, &m.drawing, &identity, 1.0, &style, area);
    let font = (w / 1000.0 * READOUT_PX).round().max(18.0);
    let margin = font * 0.4;
    draw_fiber_readout(&mut c, &m.axes, &m.drawing, (margin, margin), font);
    draw_notes(&mut c, &m.extras.notes, (margin, h - margin), font * 0.8);
    c.to_image()
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
    draw_fiber_axes(&mut c, axes, &identity, 1.0, &style, None, drawing, ((0.0, 0.0), (w, img.height() as f32)));
    // Big, as on screen: READOUT_PX on a view about 1000 px wide.
    let font = (w / 1000.0 * READOUT_PX).round().max(18.0);
    let margin = font * 0.4;
    draw_fiber_readout(&mut c, axes, &drawing, (margin, margin), font);
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

    /// Lines outside the fiber: none inside its circle, out to the picture's edges, in the chosen color.
    #[test]
    fn axis_lines_outside_the_fiber() {
        let img = RgbImage::new(600, 400);
        let mut f = FiberAxis::new(300.0, 200.0, 100.0, 0.0, 2.0, "Panda", 2);
        f.axis_x = Some(310.0); // pivot a little off the circle's center
        let blue = [0, 0, 255, 255];
        let drawing = FiberDrawing { long_lines: true, slow: blue, ..Default::default() };
        let mut neighbour = FiberAxis::new(520.0, 200.0, 50.0, 45.0, 2.0, "Panda", 2);
        neighbour.axis_x = Some(520.0);
        let out = render_fiber_axes(&img, &[f, neighbour], drawing);
        let is_blue = |x: u32, y: u32| { let p = out.get_pixel(x, y).0; p[2] > 200 && p[0] < 60 };
        assert!(!(205..=395).any(|x| is_blue(x, 200)), "slow axis line inside the fiber");
        assert!(!(475..=565).any(|x| is_blue(x, 200)), "slow axis line across the neighbour");
        assert!(is_blue(410, 200) && is_blue(598, 200) && is_blue(190, 200) && is_blue(1, 200));
        // The neighbour's own slow axis (45°): outside it only.
        assert!(is_blue(520 + 60, 200 - 60) && !is_blue(520 + 20, 200 - 20));
    }

    #[test]
    fn axis_colors_by_angle() {
        let bands = Some((0.0, [(0.5, [0, 255, 0, 255]), (1.0, [255, 255, 0, 255]), (2.0, [255, 128, 0, 255])]));
        let d = FiberDrawing { slow_bands: bands, ..Default::default() };
        let at = |deg: f64| FiberAxis::new(0.0, 0.0, 10.0, deg, 2.0, "Panda", 2);
        assert_eq!(d.slow_color(&at(0.4)), [0, 255, 0, 255]);
        assert_eq!(d.slow_color(&at(179.6)), [0, 255, 0, 255]); // -0.4°
        assert_eq!(d.slow_color(&at(1.0)), [255, 255, 0, 255]);
        assert_eq!(d.slow_color(&at(178.5)), [255, 128, 0, 255]); // -1.5°
        assert_eq!(d.slow_color(&at(5.0)), SLOW_COLOR);
        assert_eq!(d.fast_color(&at(0.4)), FAST_COLOR, "the fast axis has its own (off)");
        let pieces = fiber_readout_pieces(&[at(0.4)], &d);
        assert_eq!(pieces[0][1].1, [0, 255, 0, 255], "the readout's slow angle in the same color");
        assert_eq!(pieces[0][0].1, pieces[0][1].1, "the fiber's number too");
        let only_fast = FiberDrawing { show_slow: false, ..d };
        let text: String = fiber_readout_pieces(&[at(0.4)], &only_fast)[0].iter().map(|p| p.0.clone()).collect();
        assert_eq!(text, "1: fast -89.6°");
        assert_eq!(fiber_readout_lines(&[at(0.4)]), vec!["1: slow +0.4°  fast -89.6°".to_string()]);
        // Aiming at 90°: -89.6° and +89.6° are 0.4° off; 88.5° is 1.5° off.
        let d = FiberDrawing { slow_bands: bands.map(|(_, b)| (90.0, b)), ..Default::default() };
        assert_eq!(d.slow_color(&at(90.4)), [0, 255, 0, 255]);
        assert_eq!(d.slow_color(&at(89.6)), [0, 255, 0, 255]);
        assert_eq!(d.slow_color(&at(88.5)), [255, 128, 0, 255]);
        assert_eq!(d.slow_color(&at(0.2)), SLOW_COLOR);
    }

    #[test]
    fn annotated_export_draws_something() {
        let img = RgbImage::new(400, 300);
        let m = Measurement { id: 1, kind: "circle_centre".into(), points: vec![(200.0, 150.0), (260.0, 150.0)] };
        let out = render_annotated(&img, &[m], &Calibration::pixels());
        assert!(out.pixels().filter(|p| p.0[1] > 200).count() > 100);
    }
}
