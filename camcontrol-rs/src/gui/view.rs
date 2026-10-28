//! Zoomable, pannable image display with overlays and measurement drawing
//! (camcontrol/gui/image_view.py). Positions are in image pixels: (0, 0) is
//! the top-left corner of the image, (w, h) the bottom-right.
//!
//! Mouse, no tool selected:   wheel zooms about the cursor, left drag pans.
//! With a measurement tool:   left click adds a point (fixed-point shapes
//!   finish by themselves); Circle (center) can be dragged out from the
//!   center; double-click, right-click or Enter finish a polyline/polygon;
//!   Backspace removes the last point; Esc cancels (again: leaves the tool);
//!   middle drag pans.

use std::sync::Arc;

use eframe::egui::{
    self, Color32, ColorImage, Key, Mesh, PointerButton, Pos2, Rect, Sense, Shape, Stroke, TextureFilter, TextureHandle,
    TextureOptions, Vec2,
};
use image::RgbImage;

use super::annotate::{self, Style, MEASURE_COLOR, PREVIEW_COLOR, SELECTED_COLOR};
use super::style::{CrosshairStyle, GridStyle};
use crate::calibration::Calibration;
use crate::draw::{self, Canvas, Color, Dash, P};
use crate::measure::{kind, Measurement, Point};
use crate::processing::fiber_axis::FiberAxis;

const TILE: usize = 2048; // every GPU egui runs on takes 2048-pixel textures
const ZOOM_STEP: f32 = 1.25;
pub const MIN_ZOOM: f32 = 0.05;
pub const MAX_ZOOM: f32 = 32.0;
const PIXELATED_ZOOM: f32 = 4.0; // at or above this, pixels show as sharp blocks
const DRAG_MIN_PX: f32 = 6.0;
const BACKGROUND: Color32 = Color32::from_rgb(30, 30, 30);

pub fn c32(c: Color) -> Color32 {
    Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])
}

/// draw::Canvas on an egui painter.
pub struct EguiCanvas<'a> {
    pub painter: &'a egui::Painter,
}

fn dashes(dash: Dash, width: f32) -> Option<(f32, f32)> {
    match dash {
        Dash::Solid => None,
        Dash::Dash => Some((4.0 * width.max(1.5), 2.0 * width.max(1.5))),
        Dash::Dot => Some((width.max(1.5), 2.0 * width.max(1.5))),
    }
}

/// Triangles filling a simple polygon (ear clipping), for concave outlines.
fn fill_polygon(points: &[Pos2], color: Color32) -> Mesh {
    let mut mesh = Mesh::default();
    let n = points.len();
    if n < 3 {
        return mesh;
    }
    for p in points {
        mesh.colored_vertex(*p, color);
    }
    let area: f32 = (0..n).map(|i| points[i].x * points[(i + 1) % n].y - points[(i + 1) % n].x * points[i].y).sum();
    let ccw = area > 0.0;
    let mut idx: Vec<usize> = (0..n).collect();
    let cross = |a: Pos2, b: Pos2, c: Pos2| (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    let mut guard = 0;
    while idx.len() > 3 && guard < n * n {
        guard += 1;
        let m = idx.len();
        let mut clipped = false;
        for i in 0..m {
            let (a, b, c) = (idx[(i + m - 1) % m], idx[i], idx[(i + 1) % m]);
            let turn = cross(points[a], points[b], points[c]);
            if (turn > 0.0) != ccw || turn == 0.0 {
                continue;
            }
            let inside = idx.iter().any(|&k| {
                if k == a || k == b || k == c {
                    return false;
                }
                let p = points[k];
                let (d1, d2, d3) = (cross(points[a], points[b], p), cross(points[b], points[c], p), cross(points[c], points[a], p));
                if ccw { d1 > 0.0 && d2 > 0.0 && d3 > 0.0 } else { d1 < 0.0 && d2 < 0.0 && d3 < 0.0 }
            });
            if inside {
                continue;
            }
            mesh.add_triangle(a as u32, b as u32, c as u32);
            idx.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            break; // self-crossing: leave the rest unfilled
        }
    }
    if idx.len() == 3 {
        mesh.add_triangle(idx[0] as u32, idx[1] as u32, idx[2] as u32);
    }
    mesh
}

impl Canvas for EguiCanvas<'_> {
    fn polyline(&mut self, pts: &[P], closed: bool, color: Color, width: f32, dash: Dash, fill: Option<Color>) {
        let mut points: Vec<Pos2> = pts.iter().map(|p| Pos2::new(p.0, p.1)).collect();
        if let Some(f) = fill {
            self.painter.add(fill_polygon(&points, c32(f)));
        }
        if closed && points.len() > 2 {
            points.push(points[0]);
        }
        let stroke = Stroke::new(width, c32(color));
        match dashes(dash, width) {
            None => {
                self.painter.add(Shape::line(points, stroke));
            }
            Some((d, g)) => {
                self.painter.extend(Shape::dashed_line(&points, stroke, d, g));
            }
        }
    }

    fn ellipse(&mut self, c: P, rx: f32, ry: f32, color: Option<Color>, width: f32, dash: Dash, fill: Option<Color>) {
        let center = Pos2::new(c.0, c.1);
        if let Some(f) = fill {
            self.painter.add(Shape::ellipse_filled(center, Vec2::new(rx, ry), c32(f)));
        }
        let Some(col) = color else { return };
        match dashes(dash, width) {
            None => {
                self.painter.add(Shape::ellipse_stroke(center, Vec2::new(rx, ry), Stroke::new(width, c32(col))));
            }
            Some((d, g)) => {
                let n = ((rx.max(ry) * 0.5) as usize).clamp(24, 720);
                let pts: Vec<Pos2> = (0..=n)
                    .map(|i| {
                        let a = i as f32 / n as f32 * std::f32::consts::TAU;
                        Pos2::new(center.x + rx * a.cos(), center.y + ry * a.sin())
                    })
                    .collect();
                self.painter.extend(Shape::dashed_line(&pts, Stroke::new(width, c32(col)), d, g));
            }
        }
    }

    fn fill_rect(&mut self, min: P, max: P, color: Color) {
        self.painter.rect_filled(Rect::from_min_max(Pos2::new(min.0, min.1), Pos2::new(max.0, max.1)), 0.0, c32(color));
    }

    fn text(&mut self, pos: P, text: &str, px: f32, color: Color) {
        self.painter.text(Pos2::new(pos.0, pos.1 - px * 0.05), egui::Align2::LEFT_TOP, text, egui::FontId::proportional(px), c32(color));
    }

    fn text_width(&self, text: &str, px: f32) -> f32 {
        self.painter.layout_no_wrap(text.into(), egui::FontId::proportional(px), Color32::WHITE).size().x
    }

    fn styled_text(&mut self, pos: P, text: &str, px: f32, color: Color, mono: bool, bold: bool) {
        let id = if mono { egui::FontId::monospace(px) } else { egui::FontId::proportional(px) };
        let offsets: &[f32] = if bold { &[0.0, draw::BOLD_OFFSET] } else { &[0.0] };
        for dx in offsets {
            self.painter.text(Pos2::new(pos.0 + dx * px, pos.1 - px * 0.05), egui::Align2::LEFT_TOP, text, id.clone(), c32(color));
        }
    }

    fn styled_width(&self, text: &str, px: f32, mono: bool, bold: bool) -> f32 {
        let id = if mono { egui::FontId::monospace(px) } else { egui::FontId::proportional(px) };
        self.painter.layout_no_wrap(text.into(), id, Color32::WHITE).size().x + if bold { px * draw::BOLD_OFFSET } else { 0.0 }
    }
}

struct Tile {
    texture: TextureHandle,
    rect: Rect, // image pixels
    options: TextureOptions,
}

/// How close (screen px) to a reference line's end a press grabs it.
const GRAB_PX: f32 = 9.0;

/// What the view should draw on top of the image.
pub struct Overlays<'a> {
    pub grid: Option<&'a GridStyle>,
    pub crosshair: Option<&'a CrosshairStyle>,
    pub measurements: &'a [Measurement],
    pub calibration: &'a Calibration,
    pub selected: Option<usize>,
    pub fibers: &'a [FiberAxis],
    pub fiber_selected: Option<usize>,
    pub fiber_drawing: annotate::FiberDrawing,
    /// Region of interest, reference line, fiber numbers.
    pub extras: &'a annotate::Extras,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ViewEvent {
    Activated,
    Drawn(String, Vec<Point>),
    /// The reference line's ends after one was dragged.
    ReferenceMoved(Point, Point),
    ToolExit,
}

/// Zoom and position, for Link views: (fitting, zoom, center as a fraction of the image).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewState {
    pub fitting: bool,
    pub zoom: f32,
    pub center: (f32, f32),
    pub image_size: (u32, u32),
}

pub struct ImageView {
    tiles: Vec<Tile>,
    image: Option<Arc<RgbImage>>,
    nearest: bool,
    pub zoom: f32,
    center: Pos2,
    fitting: bool,
    pub tool: Option<String>,
    points: Vec<Point>,
    mouse: Option<Point>,
    press_pos: Option<Pos2>,
    /// Which end of the reference line is being dragged.
    grab: Option<usize>,
    /// Image pixel under the mouse and its RGB.
    pub hover: Option<(u32, u32, [u8; 3])>,
    /// Zoom or position changed by the user this frame (for Link views).
    pub moved: bool,
    /// The view's area as last shown (for its right-click menu).
    pub response: Option<egui::Response>,
    rect: Rect,
}

impl Default for ImageView {
    fn default() -> Self {
        Self {
            tiles: Vec::new(),
            image: None,
            nearest: false,
            zoom: 1.0,
            center: Pos2::ZERO,
            fitting: true,
            response: None,
            tool: None,
            points: Vec::new(),
            mouse: None,
            press_pos: None,
            grab: None,
            hover: None,
            moved: false,
            rect: Rect::NOTHING,
        }
    }
}

fn tile_pixels(image: &RgbImage, x0: usize, y0: usize, w: usize, h: usize) -> ColorImage {
    let stride = image.width() as usize * 3;
    let mut pixels = Vec::with_capacity(w * h);
    for y in y0..y0 + h {
        let start = y * stride + x0 * 3;
        pixels.extend(image.as_raw()[start..start + w * 3].chunks_exact(3).map(|p| Color32::from_rgb(p[0], p[1], p[2])));
    }
    ColorImage { size: [w, h], pixels }
}

/// The image cut into texture tiles, ready to upload (camera threads make
/// these, so the GUI thread doesn't convert every live frame).
pub fn texture_tiles(image: &RgbImage) -> Vec<Arc<ColorImage>> {
    let (width, height) = (image.width() as usize, image.height() as usize);
    let mut tiles = Vec::new();
    for y0 in (0..height).step_by(TILE) {
        for x0 in (0..width).step_by(TILE) {
            tiles.push(Arc::new(tile_pixels(image, x0, y0, TILE.min(width - x0), TILE.min(height - y0))));
        }
    }
    tiles
}

impl ImageView {
    pub fn image(&self) -> Option<&Arc<RgbImage>> {
        self.image.as_ref()
    }

    fn options(&self) -> TextureOptions {
        let mut o = TextureOptions::LINEAR;
        if self.nearest {
            o.magnification = TextureFilter::Nearest;
        }
        o
    }

    pub fn set_image(&mut self, ctx: &egui::Context, image: Arc<RgbImage>) {
        let tiles = texture_tiles(&image);
        self.set_image_tiles(ctx, image, tiles);
    }

    /// set_image with the tiles already made (texture_tiles).
    pub fn set_image_tiles(&mut self, ctx: &egui::Context, image: Arc<RgbImage>, tiles: Vec<Arc<ColorImage>>) {
        let width = image.width() as usize;
        let new_size = self.image.as_ref().map(|i| i.dimensions()) != Some(image.dimensions());
        if new_size {
            self.tiles.clear();
        }
        let options = self.options();
        let columns = width.div_ceil(TILE);
        for (n, pixels) in tiles.into_iter().enumerate() {
            match self.tiles.get_mut(n) {
                // Same size and filter: write into the texture (a full set makes OpenGL reallocate it every frame).
                Some(t) if t.options == options => t.texture.set_partial([0, 0], pixels, options),
                Some(t) => {
                    t.texture.set(pixels, options);
                    t.options = options;
                }
                None => {
                    let (x0, y0) = ((n % columns) * TILE, (n / columns) * TILE);
                    let size = Vec2::new(pixels.size[0] as f32, pixels.size[1] as f32);
                    self.tiles.push(Tile {
                        texture: ctx.load_texture(format!("image-{n}"), pixels, options),
                        rect: Rect::from_min_size(Pos2::new(x0 as f32, y0 as f32), size),
                        options,
                    });
                }
            }
        }
        self.image = Some(image);
        if new_size {
            self.fit();
        }
    }

    pub fn clear_image(&mut self) {
        self.tiles.clear();
        self.image = None;
    }

    pub fn fit(&mut self) {
        self.fitting = true;
        self.moved = true;
    }

    pub fn actual_size(&mut self) {
        self.set_zoom(1.0);
    }

    pub fn set_zoom(&mut self, z: f32) {
        self.fitting = false;
        self.zoom = z.clamp(MIN_ZOOM, MAX_ZOOM);
        self.moved = true;
    }

    pub fn zoom_in(&mut self) {
        self.set_zoom(self.zoom * ZOOM_STEP);
    }

    pub fn zoom_out(&mut self) {
        self.set_zoom(self.zoom / ZOOM_STEP);
    }

    pub fn state(&self) -> Option<ViewState> {
        let img = self.image.as_ref()?;
        let (w, h) = (img.width() as f32, img.height() as f32);
        Some(ViewState { fitting: self.fitting, zoom: self.zoom, center: (self.center.x / w, self.center.y / h), image_size: img.dimensions() })
    }

    /// Show the same part of this image as src shows of its own (Link views).
    pub fn follow(&mut self, src: &ViewState) {
        let Some(img) = &self.image else { return };
        let (w, h) = (img.width() as f32, img.height() as f32);
        if src.fitting {
            self.fitting = true;
        } else {
            self.fitting = false;
            self.zoom = (src.zoom * src.image_size.0 as f32 / w).clamp(MIN_ZOOM, MAX_ZOOM); // same share of the picture
        }
        self.center = Pos2::new(src.center.0 * w, src.center.1 * h);
    }

    pub fn set_tool(&mut self, tool: Option<String>) {
        self.tool = tool;
        self.points.clear();
    }

    /// Where an image point is on screen (as last drawn).
    pub fn to_screen(&self, x: f64, y: f64) -> Pos2 {
        self.rect.center() + (Pos2::new(x as f32, y as f32) - self.center) * self.zoom
    }

    fn to_image(&self, p: Pos2) -> Point {
        let (w, h) = self.image.as_ref().map(|i| (i.width() as f64, i.height() as f64)).unwrap_or((0.0, 0.0));
        let q = self.center + (p - self.rect.center()) / self.zoom;
        ((q.x as f64).clamp(0.0, w), (q.y as f64).clamp(0.0, h))
    }

    fn finish(&mut self, events: &mut Vec<ViewEvent>) {
        if let Some(tool) = &self.tool {
            if self.points.len() >= kind(tool).min_points {
                events.push(ViewEvent::Drawn(tool.clone(), self.points.clone()));
            }
        }
        self.points.clear();
    }

    /// The shape being drawn, with the point under the mouse.
    fn preview(&self) -> Option<Measurement> {
        let tool = self.tool.as_ref()?;
        if self.points.is_empty() {
            return None;
        }
        let mut points = self.points.clone();
        let n = kind(tool).n_points;
        if let Some(m) = self.mouse {
            if n.is_none_or(|n| points.len() < n) {
                points.push(m);
            }
        }
        Some(Measurement { id: 0, kind: tool.clone(), points })
    }

    /// Draw and handle input. `keys`: this view takes Esc / Enter / Backspace.
    pub fn show(&mut self, ui: &mut egui::Ui, overlays: &Overlays, keys: bool) -> Vec<ViewEvent> {
        let mut events = Vec::new();
        self.moved = false;
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        self.rect = rect;
        self.response = Some(response.clone());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, BACKGROUND);
        let Some(image) = self.image.clone() else {
            self.hover = None;
            if response.clicked() || response.secondary_clicked() {
                events.push(ViewEvent::Activated);
            }
            return events;
        };
        let size = Vec2::new(image.width() as f32, image.height() as f32);
        if self.fitting {
            self.zoom = (rect.width() / size.x).min(rect.height() / size.y).clamp(MIN_ZOOM, MAX_ZOOM);
            self.center = (size / 2.0).to_pos2();
        }

        let (pressed, released, hover_pos, scroll, double) = ui.input(|i| {
            let p = &i.pointer;
            (
                [PointerButton::Primary, PointerButton::Secondary, PointerButton::Middle].map(|b| p.button_pressed(b)),
                [PointerButton::Primary, PointerButton::Middle].map(|b| p.button_released(b)),
                p.hover_pos(),
                i.raw_scroll_delta.y,
                p.button_double_clicked(PointerButton::Primary),
            )
        });
        let over = hover_pos.filter(|p| rect.contains(*p) && response.hovered());
        if (over.is_some() && pressed.iter().any(|&b| b)) || response.secondary_clicked() {
            events.push(ViewEvent::Activated); // a right-click too: its menu acts on this view
        }
        // Wheel: zoom about the point under the cursor.
        if let Some(cursor) = over {
            if scroll != 0.0 {
                let under = self.center + (cursor - rect.center()) / self.zoom;
                self.fitting = false;
                self.zoom = (self.zoom * ZOOM_STEP.powf(scroll / 50.0)).clamp(MIN_ZOOM, MAX_ZOOM);
                self.center = under - (cursor - rect.center()) / self.zoom;
                self.moved = true;
            }
        }
        let clicking = self.tool.is_some();
        // The reference line's ends can be dragged (when no tool is in use).
        if let Some((a, b)) = overlays.extras.reference.filter(|_| !clicking) {
            let to_scr = |p: Point| rect.center() + (Pos2::new(p.0 as f32, p.1 as f32) - self.center) * self.zoom;
            if let Some(cursor) = over {
                let near = [a, b].iter().position(|&p| (to_scr(p) - cursor).length() <= GRAB_PX);
                if near.is_some() || self.grab.is_some() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                }
                if pressed[0] {
                    self.grab = near;
                }
            }
            if let (Some(k), Some(pos)) = (self.grab, hover_pos) {
                if response.dragged_by(PointerButton::Primary) {
                    let mut ends = [a, b];
                    ends[k] = self.to_image(pos);
                    if ends[k] != [a, b][k] {
                        events.push(ViewEvent::ReferenceMoved(ends[0], ends[1]));
                    }
                }
            }
        }
        if released[0] {
            self.grab = None;
        }
        let pan = response.dragged_by(PointerButton::Middle) || (!clicking && self.grab.is_none() && response.dragged_by(PointerButton::Primary));
        if pan {
            let d = response.drag_delta();
            if d != Vec2::ZERO {
                self.fitting = false;
                self.center -= d / self.zoom;
                self.moved = true;
            }
        }
        if let Some(cursor) = over {
            if let Some(tool) = self.tool.clone() {
                let k = kind(&tool);
                if pressed[0] {
                    self.points.push(self.to_image(cursor));
                    self.press_pos = Some(cursor);
                    if Some(self.points.len()) == k.n_points {
                        self.finish(&mut events);
                    }
                } else if pressed[1] {
                    self.finish(&mut events); // finishes an open shape; otherwise just clears it
                }
                if double && k.n_points.is_none() && !self.points.is_empty() {
                    // The second click of the double-click added a point again: take it off, then finish.
                    if self.points.len() >= 2 {
                        let (a, b) = (self.points[self.points.len() - 1], self.points[self.points.len() - 2]);
                        if (a.0 - b.0).hypot(a.1 - b.1) * self.zoom as f64 <= DRAG_MIN_PX as f64 {
                            self.points.pop();
                        }
                    }
                    self.finish(&mut events);
                }
            }
        }
        if released[0] {
            // Circle (center) dragged out from the center: the release point is the edge.
            if let (Some(press), Some(pos), Some("circle_centre")) = (self.press_pos, hover_pos, self.tool.as_deref()) {
                if self.points.len() == 1 && (pos - press).length() > DRAG_MIN_PX {
                    self.points.push(self.to_image(pos));
                    self.finish(&mut events);
                }
            }
            self.press_pos = None;
        }
        if keys {
            let (esc, enter, back) = ui.input(|i| (i.key_pressed(Key::Escape), i.key_pressed(Key::Enter), i.key_pressed(Key::Backspace)));
            if self.tool.is_some() {
                if esc {
                    if self.points.is_empty() { events.push(ViewEvent::ToolExit) } else { self.points.clear() }
                }
                if enter {
                    self.finish(&mut events);
                }
                if back {
                    self.points.pop();
                }
            }
        }

        // Sharp pixels when zoomed far in; smooth below.
        let nearest = self.zoom >= PIXELATED_ZOOM;
        if nearest != self.nearest {
            self.nearest = nearest;
            self.set_image(ui.ctx(), image.clone());
        }

        let center = self.center;
        let zoom = self.zoom;
        let to_screen = |x: f64, y: f64| -> P {
            let p = rect.center() + (Pos2::new(x as f32, y as f32) - center) * zoom;
            (p.x, p.y)
        };
        let whole = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        for t in &self.tiles {
            let a = to_screen(t.rect.min.x as f64, t.rect.min.y as f64);
            let b = to_screen(t.rect.max.x as f64, t.rect.max.y as f64);
            painter.image(t.texture.id(), Rect::from_min_max(Pos2::new(a.0, a.1), Pos2::new(b.0, b.1)), whole, Color32::WHITE);
        }
        let mut canvas = EguiCanvas { painter: &painter };
        let (iw, ih) = (image.width(), image.height());
        if let Some(g) = overlays.grid {
            let (cols, rows) = (g.columns.max(1), g.rows_for(iw, ih).max(1));
            for i in 1..cols {
                let x = iw as f64 * i as f64 / cols as f64;
                canvas.line(to_screen(x, 0.0), to_screen(x, ih as f64), g.rgba(), g.width as f32, g.dash());
            }
            for i in 1..rows {
                let y = ih as f64 * i as f64 / rows as f64;
                canvas.line(to_screen(0.0, y), to_screen(iw as f64, y), g.rgba(), g.width as f32, g.dash());
            }
        }
        let style = Style::default();
        for m in overlays.measurements {
            let color = if Some(m.id) == overlays.selected { SELECTED_COLOR } else { MEASURE_COLOR };
            annotate::draw_measurement(&mut canvas, m, overlays.calibration, &to_screen, zoom as f64, color, &style);
        }
        if let Some(p) = self.preview() {
            annotate::draw_measurement(&mut canvas, &p, overlays.calibration, &to_screen, zoom as f64, PREVIEW_COLOR, &style);
        }
        if !overlays.fibers.is_empty() {
            let area = ((rect.left(), rect.top()), (rect.right(), rect.bottom())); // long lines to the view's edges, as the crosshair
            annotate::draw_fiber_axes(&mut canvas, overlays.fibers, &to_screen, zoom as f64, &style, overlays.fiber_selected, overlays.fiber_drawing, area);
        }
        let area = ((rect.left(), rect.top()), (rect.right(), rect.bottom()));
        annotate::draw_extras(&mut canvas, overlays.extras, overlays.fibers, &overlays.fiber_drawing, &to_screen, zoom as f64, &style, area);
        if let Some(c) = overlays.crosshair {
            // On a fiber: where its axis lines cross, with the lines across the whole view to compare with its axes.
            let fiber = if c.anchor == "fiber" { overlays.fibers.get(overlays.fiber_selected.unwrap_or(0)) } else { None };
            let (area, mid) = if let Some(f) = fiber {
                let (ax, ay) = f.axis_point();
                let p = to_screen(ax, ay);
                (rect, Pos2::new(p.0, p.1))
            } else if c.anchor == "window" {
                (rect, rect.center())
            } else {
                let (a, b) = (to_screen(0.0, 0.0), to_screen(iw as f64, ih as f64));
                let area = Rect::from_min_max(Pos2::new(a.0, a.1), Pos2::new(b.0, b.1));
                (area, area.center())
            };
            let (col, w, d) = (c.rgba(), c.width as f32, c.dash());
            // With the axis lines outside the fiber, the crosshair on it stays outside too (no ring).
            if fiber.is_some() && overlays.fiber_drawing.long_lines {
                // Its lines skip every fiber, as the axis lines do.
                let circles: Vec<_> = overlays.fibers.iter().map(|f| (to_screen(f.cx, f.cy), (f.radius * zoom as f64) as f32)).collect();
                for (a, b) in [((mid.x, area.top()), (mid.x, area.bottom())), ((area.left(), mid.y), (area.right(), mid.y))] {
                    for (p, q) in annotate::outside_circles(a, b, &circles) {
                        canvas.line(p, q, col, w, d);
                    }
                }
            } else {
                canvas.line((mid.x, area.top()), (mid.x, area.bottom()), col, w, d);
                canvas.line((area.left(), mid.y), (area.right(), mid.y), col, w, d);
                if c.circle > 0 {
                    canvas.ellipse((mid.x, mid.y), c.circle as f32, c.circle as f32, Some(col), w, d, None);
                }
            }
        }
        // The angles stay in the view's top-left corner, whatever the zoom.
        annotate::draw_fiber_readout(&mut canvas, overlays.fibers, &overlays.fiber_drawing, (rect.left() + 8.0, rect.top() + 8.0),
                                     overlays.fiber_drawing.readout_px);

        self.mouse = over.map(|p| self.to_image(p));
        self.hover = over.and_then(|cursor| {
            let p = self.center + (cursor - rect.center()) / self.zoom;
            let (x, y) = (p.x.floor(), p.y.floor());
            if x < 0.0 || y < 0.0 || x >= size.x || y >= size.y {
                return None;
            }
            Some((x as u32, y as u32, image.get_pixel(x as u32, y as u32).0))
        });
        if clicking && over.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        events
    }
}

/// Draw a label like the measurements' on any painter (used by the views' grid image too).
pub fn screen_label(painter: &egui::Painter, pos: Pos2, text: &str, color: Color) {
    let mut c = EguiCanvas { painter };
    draw::label(&mut c, (pos.x, pos.y), text, color, 14.0);
}
