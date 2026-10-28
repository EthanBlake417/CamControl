//! Alignment panel: take n readings of every fiber's slow axis (Space) and
//! add their mean and spread to a table, against a reference line if one is
//! drawn; a region of interest the fiber search keeps to; and saving the
//! table, alone or with the marked-up photo. Also the Fiber count panel:
//! the fibers numbered from the top one's number, up or down, and which one
//! is on (has the brightest center).

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use eframe::egui::{self, RichText};
use image::RgbImage;
use serde_json::{json, Map};

use super::actions::{file_name, timestamp};
use crate::capture::{self, next_capture_path, CaptureOptions};
use super::annotate::{render_markup, signed_angle, Extras, Markup};
use super::fiber_log::axial_stats;
use super::stripes::PanelId;
use super::App;
use crate::export::{self, Cell, Sheet};
use crate::processing::fiber_axis::{brightest_centre, FiberAxis, Region};

/// What the next shape drawn on a view sets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DrawTarget {
    RegionRect,
    RegionCircle,
    Reference,
}

/// Readings being taken of one fiber (the one nearest the middle), one per update.
#[derive(Clone, Debug)]
pub struct Collecting {
    pub pane: u64,
    pub wanted: usize,
    pub readings: Vec<f64>,
    /// Which axis the readings are of ("fast" / "slow"), as last measured.
    pub axis: &'static str,
    /// With a photo: the measured frames added up (width, height, sum), and the fiber last measured.
    pub sum: Option<(u32, u32, Vec<f32>)>,
    pub frames: usize,
    pub fiber: Option<FiberAxis>,
}

/// A take's photo (not compared or printed whole).
#[derive(Clone)]
pub struct Photo(pub Arc<RgbImage>);

impl PartialEq for Photo {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for Photo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Photo({}x{})", self.0.width(), self.0.height())
    }
}

/// One table row: a fiber's fast axis over one set of readings.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub time: String,
    pub fiber: i64,
    pub n: usize,
    /// With a reference line: mean offset of the axis from perpendicular to it; else the mean fast axis from horizontal.
    pub mean: f64,
    pub std: f64,
    /// The reference line's angle the readings were measured from.
    pub reference: Option<f64>,
    /// The axis measured: with a line, the one nearest perpendicular to it; else the fast axis.
    pub axis: &'static str,
    /// The frames measured, averaged, the fiber's number beside it (Take a photo with each reading).
    pub photo: Option<Photo>,
}

pub struct Alignment {
    pub n: usize,
    pub collecting: Option<Collecting>,
    pub rows: Vec<Row>,
    pub region: Option<Region>,
    pub reference: Option<((f64, f64), (f64, f64))>,
    pub drawing: Option<DrawTarget>,
    pub status: String,
    /// The number the next take's fiber gets, and whether numbers count up (else down).
    pub next_number: i64,
    pub ascending: bool,
    pub trend: Trend,
}

/// How many seconds the live trend shows.
pub const TREND_SECONDS: f64 = 30.0;

/// The measured fiber's angle at each update over the last TREND_SECONDS (active view).
pub struct Trend {
    pub start: Instant,
    pub pane: u64,
    pub reference: Option<f64>,
    /// (seconds since start, angle)
    pub points: VecDeque<(f64, f64)>,
    pub axis: &'static str,
}

impl Default for Trend {
    fn default() -> Self {
        Self { start: Instant::now(), pane: 0, reference: None, points: VecDeque::new(), axis: "fast" }
    }
}

impl Trend {
    /// Add a value now; forget those older than TREND_SECONDS. A different view or
    /// reference line starts afresh (the values aren't comparable).
    pub fn add(&mut self, pane: u64, reference: Option<f64>, angle: f64, axis: &'static str) {
        if pane != self.pane || reference != self.reference {
            self.points.clear();
            (self.pane, self.reference) = (pane, reference);
        }
        let now = self.start.elapsed().as_secs_f64();
        self.points.push_back((now, angle));
        self.axis = axis;
        while self.points.front().is_some_and(|p| now - p.0 > TREND_SECONDS) {
            self.points.pop_front();
        }
    }
}

/// The straight line best through the points (least squares, perpendicular distances):
/// its ends where the points' span along it ends, plus `extend` either way.
pub fn fit_line(points: &[(f64, f64)], extend: f64) -> Option<((f64, f64), (f64, f64))> {
    if points.len() < 2 {
        return None;
    }
    let n = points.len() as f64;
    let (mx, my) = (points.iter().map(|p| p.0).sum::<f64>() / n, points.iter().map(|p| p.1).sum::<f64>() / n);
    let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
    for &(x, y) in points {
        sxx += (x - mx) * (x - mx);
        syy += (y - my) * (y - my);
        sxy += (x - mx) * (y - my);
    }
    let theta = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let (dx, dy) = (theta.cos(), theta.sin());
    let ts: Vec<f64> = points.iter().map(|&(x, y)| (x - mx) * dx + (y - my) * dy).collect();
    let (lo, hi) = (ts.iter().cloned().fold(f64::INFINITY, f64::min) - extend, ts.iter().cloned().fold(f64::NEG_INFINITY, f64::max) + extend);
    (hi - lo > 1e-6).then(|| ((mx + lo * dx, my + lo * dy), (mx + hi * dx, my + hi * dy)))
}

pub const HEADERS: [&str; 7] = ["Time", "Fiber", "Readings", "Axis", "Mean (deg)", "Std (deg)", "Reference line (deg)"];

/// The line turned about its middle to `deg` (counter-clockwise from horizontal on screen), its length kept.
pub fn rotate_line(a: (f64, f64), b: (f64, f64), deg: f64) -> ((f64, f64), (f64, f64)) {
    let (mx, my) = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
    let half = ((b.0 - a.0).hypot(b.1 - a.1) / 2.0).max(1.0);
    let (dx, dy) = (deg.to_radians().cos() * half, -deg.to_radians().sin() * half);
    ((mx - dx, my - dy), (mx + dx, my + dy))
}

/// A line's angle as the fiber axes' (counter-clockwise from horizontal on screen, 0 to 180).
pub fn line_angle(a: (f64, f64), b: (f64, f64)) -> f64 {
    (-(b.1 - a.1)).atan2(b.0 - a.0).to_degrees().rem_euclid(180.0)
}

impl Alignment {
    pub fn new(n: usize, region: Option<Region>, reference: Option<[f64; 4]>, next_number: i64, ascending: bool) -> Self {
        Self {
            n: n.clamp(1, 1000),
            collecting: None,
            rows: Vec::new(),
            region,
            reference: reference.map(|r| ((r[0], r[1]), (r[2], r[3]))),
            drawing: None,
            status: String::new(),
            next_number,
            ascending,
            trend: Trend::default(),
        }
    }

    pub fn reference_deg(&self) -> Option<f64> {
        self.reference.map(|(a, b)| line_angle(a, b))
    }

    /// A row for the fiber from the readings taken, numbered next_number (which then counts
    /// on, up or down, for the next fiber).
    pub fn finish(&mut self, readings: &[f64], axis: &'static str, reference: Option<f64>) -> &Row {
        let time = chrono::Local::now().format("%H:%M:%S").to_string();
        let (mean, std) = axial_stats(readings);
        self.rows.push(Row { time, fiber: self.next_number, n: readings.len(), mean: signed_angle(mean), std, reference, axis, photo: None });
        self.next_number += if self.ascending { 1 } else { -1 };
        self.rows.last().unwrap()
    }

    pub fn sheet(&self) -> Sheet {
        let r2 = |v: f64| Cell::Num((v * 100.0).round() / 100.0);
        let rows = self.rows.iter().map(|r| vec![
            Cell::Text(r.time.clone()), Cell::Text(r.fiber.to_string()), Cell::Text(r.n.to_string()), Cell::Text(r.axis.into()), r2(r.mean), r2(r.std),
            r.reference.map(|v| r2(signed_angle(v))).unwrap_or(Cell::Empty),
        ]).collect();
        Sheet::new("Alignment", &HEADERS, rows)
    }

    /// The table as text lines, for drawing on a photo.
    pub fn table_lines(&self) -> Vec<String> {
        let mut lines = vec!["Fiber\tAxis\tMean\tStd dev\tn".to_string()];
        for r in &self.rows {
            lines.push(format!("{}\t{}\t{:+.2}°\t{:.2}°\t{}", r.fiber, r.axis, r.mean, r.std, r.n));
        }
        lines
    }
}

impl App {
    pub(crate) fn collecting_on(&self, pane: u64) -> bool {
        self.alignment.collecting.as_ref().is_some_and(|c| c.pane == pane)
    }

    /// Space / Take readings: the next n measurements of the active view's fibers (live), or
    /// the fibers measured now (a still image: one reading).
    pub(crate) fn take_readings(&mut self) {
        if self.alignment.collecting.is_some() {
            return;
        }
        let id = self.active;
        let p = self.pane(id);
        if p.image().is_none() {
            self.alignment.status = "No image in the view.".into();
            return;
        }
        let live = p.live;
        self.alignment.collecting = Some(Collecting { pane: id, wanted: if live { self.alignment.n } else { 1 }, readings: Vec::new(), axis: "fast",
                                                      sum: None, frames: 0, fiber: None });
        self.alignment.status = "Taking readings...".into();
        if !live {
            if self.pane(id).fibers.is_empty() {
                self.find_fiber_axes(true);
            } else {
                let image = self.pane(id).image();
                self.alignment_reading(id, image.as_ref());
            }
        }
    }

    /// A new measurement of a view's fibers (in `image`): one reading, if readings are being taken there.
    pub(crate) fn alignment_reading(&mut self, pane: u64, image: Option<&Arc<RgbImage>>) {
        if !self.collecting_on(pane) {
            return;
        }
        let drawing = self.fiber_drawing();
        let Some(f) = self.measured_fiber(pane) else {
            if !self.pane(pane).live {
                self.alignment.collecting = None;
                self.alignment.status = "No fiber found.".into();
            }
            return;
        };
        let angle = drawing.off_perpendicular(&f).unwrap_or_else(|| drawing.shown(f.fast_deg()));
        let axis = drawing.perpendicular_axis(&f).unwrap_or("fast");
        let photos = self.saved.align_photos;
        let c = self.alignment.collecting.as_mut().unwrap();
        c.readings.push(angle);
        c.axis = axis;
        if let (true, Some(img)) = (photos, image) {
            let (w, h) = img.dimensions();
            let sum = c.sum.get_or_insert_with(|| (w, h, vec![0.0; (w * h * 3) as usize]));
            if (sum.0, sum.1) == (w, h) {
                for (s, &v) in sum.2.iter_mut().zip(img.as_raw()) {
                    *s += v as f32;
                }
                c.frames += 1;
            }
        }
        c.fiber = Some(f);
        let (got, wanted) = (c.readings.len(), c.wanted);
        if got < wanted {
            self.alignment.status = format!("Taking readings: {got} of {wanted}");
            return;
        }
        let done = self.alignment.collecting.take().unwrap();
        let number = self.alignment.next_number;
        let photo = done.sum.filter(|_| done.frames > 0).map(|(w, h, sum)| {
            let k = 1.0 / done.frames as f32;
            let avg = RgbImage::from_raw(w, h, sum.iter().map(|v| (v * k).round().clamp(0.0, 255.0) as u8).collect()).unwrap();
            let mut markup = self.markup_of(pane, Vec::new());
            if let Some(f) = &done.fiber {
                markup.extras.labels.push(((f.cx, f.cy, f.radius), number.to_string()));
            }
            Photo(Arc::new(render_markup(&avg, &markup)))
        });
        let row_photo = photo.is_some();
        let fiber = self.alignment.finish(&done.readings, done.axis, drawing.reference).fiber;
        if let Some(p) = photo {
            self.alignment.rows.last_mut().unwrap().photo = Some(p);
        }
        self.alignment.status = format!("Added fiber {fiber} ({got} reading{}{}).", if got == 1 { "" } else { "s" },
                                        if row_photo { ", with a photo" } else { "" });
        if !self.panels.is_open(PanelId::Alignment) {
            self.panels.set_open(PanelId::Alignment, true);
        }
    }

    /// Each measurement of the active view: the measured fiber's angle onto the live trend.
    pub(crate) fn alignment_trend(&mut self, pane: u64) {
        if pane != self.active {
            return;
        }
        let drawing = self.fiber_drawing();
        let Some(f) = self.measured_fiber(pane) else { return };
        let angle = drawing.off_perpendicular(&f).unwrap_or_else(|| drawing.shown(f.fast_deg()));
        let axis = drawing.perpendicular_axis(&f).unwrap_or("fast");
        self.alignment.trend.add(pane, drawing.reference, angle, axis);
    }

    /// A reference line fitted through the centers of the active view's fibers.
    pub(crate) fn fit_reference_to_fibers(&mut self) {
        let fibers = &self.active_pane().fibers;
        let centers: Vec<(f64, f64)> = fibers.iter().map(|f| (f.cx, f.cy)).collect();
        let extend = fibers.iter().map(|f| f.radius).fold(0.0, f64::max);
        match fit_line(&centers, extend) {
            Some(line) => {
                self.alignment.reference = Some(line);
                self.message(format!("Reference line through {} fiber centers.", centers.len()), Some(6.0));
            }
            None => self.warn("Fit to fibers", "Needs at least two fibers: press Find axes (Fiber axis) first."),
        }
    }

    /// The fiber measured for alignment: one at a time, so the one nearest the middle of the
    /// region of interest (or of the image) when several are found.
    fn measured_fiber(&self, pane: u64) -> Option<FiberAxis> {
        let p = self.pane(pane);
        let (mx, my) = match self.alignment.region {
            Some(r) => {
                let (x0, y0, x1, y1) = r.bounds();
                ((x0 + x1) / 2.0, (y0 + y1) / 2.0)
            }
            None => p.image().map(|i| (i.width() as f64 / 2.0, i.height() as f64 / 2.0)).unwrap_or((0.0, 0.0)),
        };
        p.fibers.iter().min_by(|a, b| (a.cx - mx).hypot(a.cy - my).total_cmp(&(b.cx - mx).hypot(b.cy - my))).cloned()
    }

    /// A shape drawn for the region of interest or the reference line.
    pub(crate) fn alignment_drawn(&mut self, kind: &str, points: &[(f64, f64)]) {
        let target = self.alignment.drawing.take();
        self.tool = None;
        if points.len() < 2 {
            return;
        }
        let (a, b) = (points[0], points[1]);
        match (target, kind) {
            (Some(DrawTarget::RegionRect), "rectangle") => self.alignment.region = Some(Region::rect(a, b)),
            (Some(DrawTarget::RegionCircle), "circle_centre") => {
                self.alignment.region = Some(Region::Circle { cx: a.0, cy: a.1, r: (b.0 - a.0).hypot(b.1 - a.1) })
            }
            (Some(DrawTarget::Reference), "line") => self.alignment.reference = Some((a, b)),
            _ => return,
        }
        if matches!(target, Some(DrawTarget::RegionRect | DrawTarget::RegionCircle)) {
            self.region_changed();
        }
    }

    /// Search again (only inside the region, if there is one).
    pub(crate) fn region_changed(&mut self) {
        for p in &mut self.panes {
            p.fiber_locked = false;
        }
        if self.active_pane().image().is_some() && (self.fiber.live || !self.active_pane().fibers.is_empty()) {
            self.find_fiber_axes(true);
        }
    }

    fn start_drawing(&mut self, target: DrawTarget) {
        self.picking_rod = false;
        self.alignment.drawing = Some(target);
        self.tool = Some(match target {
            DrawTarget::RegionRect => "rectangle",
            DrawTarget::RegionCircle => "circle_centre",
            DrawTarget::Reference => "line",
        }.into());
    }

    /// What's drawn over the views besides measurements and fiber axes.
    pub(crate) fn view_extras(&self) -> Extras {
        let clean = self.counting_clean();
        Extras {
            region: self.alignment.region.filter(|_| !clean),
            reference: self.alignment.reference.filter(|_| !clean),
            numbers: self.panels.is_open(PanelId::FiberCount),
            notes: Vec::new(),
            labels: Vec::new(),
            region_look: Some(self.saved.fiber_lines.region_look()),
            reference_look: Some(self.saved.fiber_lines.reference_look()),
        }
    }

    /// Everything shown on a view, to draw on a saved copy.
    pub(crate) fn markup_of(&self, id: u64, notes: Vec<String>) -> Markup {
        let p = self.pane(id);
        Markup {
            axes: p.fibers.clone(),
            drawing: self.fiber_drawing(),
            measurements: p.measurements.clone(),
            calibration: Some(self.calibration.clone()),
            extras: Extras { notes, ..self.view_extras() },
        }
    }

    /// Save the table as CSV (in the Alignment panel's folder, as its name); with `photos`, also
    /// each take's photo as PNG (..._fiber17.png).
    pub(crate) fn save_alignment(&mut self, photos: bool) {
        if self.alignment.rows.is_empty() {
            self.warn("Save measurement", "No readings yet: press Take readings (Space) first.");
            return;
        }
        let folder = PathBuf::from(if self.saved.align_folder.is_empty() { &self.saved.folder } else { &self.saved.align_folder });
        let _ = std::fs::create_dir_all(&folder);
        let csv = if self.saved.align_name.trim().is_empty() {
            folder.join(format!("alignment_{}.csv", timestamp()))
        } else {
            next_capture_path(&folder, &self.saved.align_name, "csv", "alignment")
        };
        if let Err(e) = export::write_tables(&[self.alignment.sheet()], &csv, false) {
            self.warn("Save measurement", format!("Save failed: {e}"));
            return;
        }
        if !photos {
            self.message(format!("Saved {}", file_name(&csv)), Some(8.0));
            return;
        }
        let stem = csv.file_stem().unwrap_or_default().to_string_lossy().to_string();
        let mut saved = 0;
        for r in self.alignment.rows.iter().filter(|r| r.photo.is_some()) {
            let mut path = folder.join(format!("{stem}_fiber{}.png", r.fiber));
            let mut k = 2;
            while path.exists() {
                path = folder.join(format!("{stem}_fiber{}_{k}.png", r.fiber));
                k += 1;
            }
            if let Err(e) = r.photo.as_ref().unwrap().0.save(&path) {
                self.warn("Save with photos", format!("Save failed: {e}"));
                return;
            }
            saved += 1;
        }
        self.message(format!("Saved {} and {saved} photo{}", file_name(&csv), if saved == 1 { "" } else { "s" }), Some(8.0));
    }

    pub(crate) fn alignment_panel(&mut self, ui: &mut egui::Ui) {
        // Side by side when there's room (the bottom edge), else stacked (a side edge).
        let w = ui.available_width();
        if w > 1000.0 {
            ui.columns(3, |c| {
                self.align_readings_box(&mut c[0]);
                c[0].add_space(6.0);
                self.align_trend_box(&mut c[0]);
                self.align_setup_box(&mut c[1]);
                self.align_table(&mut c[2]);
            });
        } else if w > 600.0 {
            ui.columns(2, |c| {
                self.align_readings_box(&mut c[0]);
                c[0].add_space(6.0);
                self.align_trend_box(&mut c[0]);
                self.align_setup_box(&mut c[1]);
                c[1].add_space(6.0);
                self.align_table(&mut c[1]);
            });
        } else {
            self.align_readings_box(ui);
            self.align_trend_box(ui);
            self.align_setup_box(ui);
            self.align_table(ui);
        }
    }

    fn align_readings_box(&mut self, ui: &mut egui::Ui) {
        let busy = self.alignment.collecting.is_some();
        section(ui, "Readings", None, |ui| {
            let text = if busy { "Taking readings..." } else { "Take readings  (Space)" };
            let (fill, color) = if busy { (ui.visuals().widgets.inactive.weak_bg_fill, ui.visuals().weak_text_color()) }
                                else { (egui::Color32::from_rgb(0x2f, 0x6f, 0xd0), egui::Color32::WHITE) };
            let button = egui::Button::new(RichText::new(text).size(20.0).strong().color(color)).fill(fill)
                .min_size(egui::vec2(ui.available_width(), 44.0));
            if ui.add_enabled(!busy, button)
                .on_hover_text("Measure the fiber (the one nearest the middle) this many times (live: the next\n\
                                updates) and add the mean and standard deviation to the table as fiber #: with a\n\
                                reference line, how far its axis nearest perpendicular to the line is from perpendicular;\n\
                                without, its fast axis from horizontal.").clicked() {
                self.take_readings();
            }
            ui.add_space(4.0);
            egui::Grid::new("align_inputs").num_columns(2).spacing(egui::vec2(10.0, 6.0)).show(ui, |ui| {
                ui.label("Fiber #");
                ui.horizontal(|ui| {
                    ui.add_enabled(!busy, egui::DragValue::new(&mut self.alignment.next_number).range(-99999..=99999))
                        .on_hover_text("The number the next fiber measured gets; it then counts up or down by one for each take.");
                    ui.radio_value(&mut self.alignment.ascending, true, "Up");
                    ui.radio_value(&mut self.alignment.ascending, false, "Down");
                });
                ui.end_row();
                ui.label("Readings");
                ui.horizontal(|ui| {
                    ui.add_enabled(!busy, egui::DragValue::new(&mut self.alignment.n).range(1..=1000))
                        .on_hover_text("How many measurements each take averages.");
                    if busy && ui.button("Cancel").clicked() {
                        self.alignment.collecting = None;
                        self.alignment.status = "Cancelled.".into();
                    }
                });
                ui.end_row();
            });
            ui.checkbox(&mut self.saved.align_photos, "Take a photo with each reading")
                .on_hover_text("Keep a picture with each take: the frames measured, averaged, with what's shown on the view\n\
                                and the fiber's number beside it. Save with photos writes them as PNG next to the table.");
            if !self.alignment.status.is_empty() {
                ui.label(RichText::new(&self.alignment.status).weak());
            }
        });
    }

    fn align_trend_box(&mut self, ui: &mut egui::Ui) {
        let t = &self.alignment.trend;
        let current = t.points.back().map(|p| p.1);
        section(ui, "Live trend", None, |ui| {
            ui.horizontal(|ui| {
                match current {
                    Some(v) => ui.label(RichText::new(format!("{} {v:+.2}°", t.axis)).size(20.0).strong()),
                    None => ui.label(RichText::new("No measurements yet").weak()),
                };
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(format!("last {TREND_SECONDS:.0} s")).weak());
                });
            }).response.on_hover_text("The measured fiber's angle at each update (turn on Update live in Fiber axis to follow it):
                                       with a reference line, its axis against perpendicular to the line; else its fast axis.");
            trend_plot(ui, t, 80.0);
        });
    }

    fn align_setup_box(&mut self, ui: &mut egui::Ui) {
        let drawing = self.alignment.drawing;
        section(ui, "Reference line", Some(c32(self.saved.fiber_lines.reference_look().0)), |ui| {
            ui.horizontal(|ui| {
                if big_toggle(ui, drawing == Some(DrawTarget::Reference), if self.alignment.reference.is_some() { "Draw again" } else { "Draw line" })
                    .on_hover_text("Drag a line on the image along the fibers' edge (e.g. the groove): the readout and\n\
                                    the readings then give each fiber's axis against perpendicular to it.").clicked() {
                    self.start_drawing(DrawTarget::Reference);
                }
                let fibers = self.active_pane().fibers.len();
                if ui.add_enabled(fibers >= 2, egui::Button::new("Fit to fibers").min_size(egui::vec2(0.0, 30.0)))
                    .on_hover_text("Put the line through the centers of the fibers found (e.g. a groove full of fibers).
                                    Needs two or more: press Find axes first.").clicked() {
                    self.fit_reference_to_fibers();
                }
                if ui.add_enabled(self.alignment.reference.is_some(), egui::Button::new("Clear").min_size(egui::vec2(0.0, 30.0))).clicked() {
                    self.alignment.reference = None;
                }
            });
            match self.alignment.reference {
                Some((a, b)) => {
                    ui.horizontal(|ui| {
                        let mut deg = signed_angle(line_angle(a, b));
                        ui.label("Angle");
                        let mut changed = ui.add(egui::DragValue::new(&mut deg).range(-90.0..=90.0).speed(0.01).fixed_decimals(2).suffix("°"))
                            .on_hover_text("Type or drag the line's angle (counter-clockwise from horizontal): it turns about its middle.").changed();
                        if ui.button("-0.1°").clicked() {
                            deg -= 0.1;
                            changed = true;
                        }
                        if ui.button("+0.1°").clicked() {
                            deg += 0.1;
                            changed = true;
                        }
                        if changed {
                            self.alignment.reference = Some(rotate_line(a, b, deg));
                        }
                    });
                    ui.label(RichText::new("Drag its ends on the image to move it.").weak());
                }
                None => {
                    ui.label(RichText::new("None: angles from horizontal.").weak());
                }
            }
        });
        ui.add_space(6.0);
        section(ui, "Region of interest", Some(c32(self.saved.fiber_lines.region_look().0)), |ui| {
            ui.horizontal(|ui| {
                if big_toggle(ui, drawing == Some(DrawTarget::RegionRect), "Rectangle")
                    .on_hover_text("Drag a rectangle on the image: fibers are only looked for inside it.").clicked() {
                    self.start_drawing(DrawTarget::RegionRect);
                }
                if big_toggle(ui, drawing == Some(DrawTarget::RegionCircle), "Circle")
                    .on_hover_text("Drag from the center outwards: fibers are only looked for inside it.").clicked() {
                    self.start_drawing(DrawTarget::RegionCircle);
                }
                if ui.add_enabled(self.alignment.region.is_some(), egui::Button::new("Clear").min_size(egui::vec2(0.0, 30.0))).clicked() {
                    self.alignment.region = None;
                    self.region_changed();
                }
            });
            ui.label(RichText::new(match self.alignment.region {
                None => "None: the whole image is searched.",
                Some(Region::Rect { .. }) => "Fibers are looked for inside the rectangle.",
                Some(Region::Circle { .. }) => "Fibers are looked for inside the circle.",
            }).weak());
        });
    }

    fn align_table(&mut self, ui: &mut egui::Ui) {
        section(ui, "Results", None, |ui| {
            if self.alignment.rows.is_empty() {
                ui.label(RichText::new("No readings yet: press Take readings (Space).").weak());
                return;
            }
            egui::ScrollArea::vertical().id_salt("align_rows").max_height(ui.available_height().max(120.0) - 30.0)
                .stick_to_bottom(true).auto_shrink([false, true]).show(ui, |ui| {
                egui::Grid::new("align_grid").striped(true).num_columns(6).spacing(egui::vec2(16.0, 4.0)).show(ui, |ui| {
                    for h in ["Fiber #", "Axis", "Mean", "Std dev", "n", "Time"] {
                        ui.strong(h);
                    }
                    ui.end_row();
                    for r in &self.alignment.rows {
                        ui.label(RichText::new(r.fiber.to_string()).strong());
                        ui.label(r.axis);
                        ui.label(RichText::new(format!("{:+.2}°", r.mean)).strong());
                        ui.label(format!("{:.2}°", r.std));
                        ui.label(r.n.to_string());
                        ui.label(RichText::new(&r.time).weak());
                        ui.end_row();
                    }
                });
            });
            ui.add_space(4.0);
            output_rows(ui, "align_output", &mut self.saved.align_folder, &mut self.saved.align_name, &self.saved.folder);
            let with_photos = self.alignment.rows.iter().any(|r| r.photo.is_some());
            ui.horizontal_wrapped(|ui| {
                if ui.button("Save").on_hover_text("Save the table as CSV.").clicked() {
                    self.save_alignment(false);
                }
                if ui.add_enabled(with_photos, egui::Button::new("Save with photos"))
                    .on_hover_text("Save the table (CSV) and each take's photo (PNG, ..._fiber17.png).\n\
                                    Tick Take a photo with each reading to take them.").clicked() {
                    self.save_alignment(true);
                }
                if ui.button("Clear table").clicked() {
                    self.alignment.rows.clear();
                }
            });
        });
    }

    // --- fiber count -----------------------------------------------------------------------------------

    fn count_notes(&self, fibers: &[FiberAxis]) -> Vec<String> {
        let d = self.fiber_drawing();
        let mut notes = Vec::new();
        if let (Some(_), Some(_)) = (fibers.first(), fibers.last()) {
            let dir = if d.ascending { "up" } else { "down" };
            notes.push(format!("{} fibers, numbered {} to {} from the top (counting {dir})",
                               fibers.len(), d.number(0), d.number(fibers.len() - 1)));
        }
        if let Some((i, _)) = brightest_centre(fibers) {
            notes.push(format!("Fiber {} is on", d.number(i)));
        }
        notes
    }

    /// Find every fiber (not just the center one).
    pub(crate) fn find_all_fibers(&mut self) {
        self.fiber.centre_only = false;
        self.find_fiber_axes(false);
    }

    /// Save the view's image with the numbers (PNG or TIFF, with the details inside) and a CSV
    /// table of the fibers beside it, in the Fiber count panel's folder, as its name.
    pub(crate) fn save_fiber_count(&mut self) {
        let id = self.active;
        let Some(image) = self.pane(id).image() else {
            self.warn("Save fiber count", "No image in the view.");
            return;
        };
        let fibers = self.pane(id).fibers.clone();
        if fibers.is_empty() {
            self.warn("Save fiber count", "No fibers found: press Find fibers first.");
            return;
        }
        let notes = self.count_notes(&fibers);
        let mut markup = self.markup_of(id, notes.clone());
        markup.extras.numbers = true;
        let d = self.fiber_drawing();
        let on = brightest_centre(&fibers).map(|b| b.0);
        let mut extra = Map::new();
        extra.insert("fiber_count".into(), json!({
            "count": fibers.len(), "top_number": d.first_number, "ascending": d.ascending,
            "on": on.map(|i| d.number(i)), "notes": notes,
        }));
        let options = CaptureOptions {
            exposure: None,
            exposure_seconds: None,
            gain: None,
            frames_averaged: 1,
            scale: 1.0,
            fmt: COUNT_FORMATS[self.saved.count_format.min(1)].1.into(),
            folder: PathBuf::from(if self.saved.count_folder.is_empty() { &self.saved.folder } else { &self.saved.count_folder }),
            name: self.saved.count_name.clone(),
            prefix: "fibers".into(),
            extra,
        };
        let path = match capture::save_capture(&render_markup(&image, &markup), &options) {
            Ok(p) => p,
            Err(e) => {
                self.warn("Save fiber count", format!("Save failed: {e}"));
                return;
            }
        };
        let r1 = |v: f64| Cell::Num((v * 10.0).round() / 10.0);
        let rows = fibers.iter().enumerate().map(|(i, f)| vec![
            Cell::Text(d.number(i).to_string()), r1(f.cx), r1(f.cy), r1(2.0 * f.radius),
            f.centre_brightness.map(r1).unwrap_or(Cell::Empty), Cell::Text(if on == Some(i) { "on".into() } else { String::new() }),
            r1(d.shown(f.slow_deg)), r1(d.shown(f.fast_deg())),
        ]).collect();
        let sheet = Sheet::new("Fibers", &["Fiber", "Center x (px)", "Center y (px)", "Diameter (px)", "Center brightness (0-255)", "On",
                                           "Slow axis (deg)", "Fast axis (deg)"], rows);
        let csv = path.with_extension("csv");
        match export::write_tables(&[sheet], &csv, false) {
            Ok(()) => self.message(format!("Saved {} and {}", file_name(&path), file_name(&csv)), Some(8.0)),
            Err(e) => self.warn("Save fiber count", format!("Save failed: {e}")),
        }
    }

    pub(crate) fn fiber_count_panel(&mut self, ui: &mut egui::Ui) {
        let w = ui.available_width();
        if w > 1000.0 {
            ui.columns(3, |c| {
                self.count_find_box(&mut c[0]);
                c[0].add_space(6.0);
                self.count_numbering_box(&mut c[0]);
                self.count_save_box(&mut c[1]);
                self.count_table(&mut c[2]);
            });
        } else if w > 600.0 {
            ui.columns(2, |c| {
                self.count_find_box(&mut c[0]);
                c[0].add_space(6.0);
                self.count_numbering_box(&mut c[0]);
                self.count_save_box(&mut c[1]);
                c[1].add_space(6.0);
                self.count_table(&mut c[1]);
            });
        } else {
            self.count_find_box(ui);
            self.count_numbering_box(ui);
            self.count_save_box(ui);
            self.count_table(ui);
        }
    }

    fn count_find_box(&mut self, ui: &mut egui::Ui) {
        let fibers = self.active_pane().fibers.clone();
        let d = self.fiber_drawing();
        section(ui, "", None, |ui| {
            let button = egui::Button::new(RichText::new("Find fibers").size(20.0).strong().color(egui::Color32::WHITE))
                .fill(egui::Color32::from_rgb(0x2f, 0x6f, 0xd0)).min_size(egui::vec2(ui.available_width(), 44.0));
            if ui.add(button).on_hover_text("Find and number all the fibers in the view (turns off Only the center fiber).").clicked() {
                self.find_all_fibers();
            }
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(match fibers.len() {
                    0 => "No fibers found yet.".to_string(),
                    1 => "1 fiber".to_string(),
                    n => format!("{n} fibers"),
                }).size(18.0).strong());
                if let Some((i, _)) = brightest_centre(&fibers) {
                    ui.add_space(12.0);
                    ui.label(RichText::new(format!("Fiber {} is on", d.number(i))).size(18.0).strong().color(egui::Color32::from_rgb(20, 150, 40)))
                        .on_hover_text("The fiber with the brightest center (light in its core).");
                }
            });
        });
    }

    fn count_save_box(&mut self, ui: &mut egui::Ui) {
        section(ui, "Save", None, |ui| {
            output_rows(ui, "count_output", &mut self.saved.count_folder, &mut self.saved.count_name, &self.saved.folder);
            ui.horizontal(|ui| {
                ui.label("Format");
                egui::ComboBox::from_id_salt("count_format").selected_text(COUNT_FORMATS[self.saved.count_format.min(1)].0).show_ui(ui, |ui| {
                    for (i, (label, _)) in COUNT_FORMATS.iter().enumerate() {
                        ui.selectable_value(&mut self.saved.count_format, i, *label);
                    }
                });
                if ui.button("Save image with information").on_hover_text("Save the view's image with the numbers and which fiber is on\n\
                                                                         (details inside it), and a CSV table of the fibers beside it.").clicked() {
                    self.save_fiber_count();
                }
            });
        });
    }

    fn count_numbering_box(&mut self, ui: &mut egui::Ui) {
        section(ui, "Numbering", None, |ui| {
            egui::Grid::new("count_inputs").num_columns(2).spacing(egui::vec2(10.0, 6.0)).show(ui, |ui| {
                ui.label("Top fiber #");
                ui.add(egui::DragValue::new(&mut self.saved.count_first).range(-99999..=99999))
                    .on_hover_text("The top fiber's number (side by side: the left one's).");
                ui.end_row();
                ui.label("Counting");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.saved.count_ascending, true, "Up");
                    ui.radio_value(&mut self.saved.count_ascending, false, "Down");
                });
                ui.end_row();
            });
            ui.checkbox(&mut self.saved.count_show_axes, "Show axes, angles and region")
                .on_hover_text("While this panel is open, also show the fibers' axis lines, stress parts, the angles box,\n\
                                the region of interest and the reference line (on the view and the saved image).");
        });
    }

    fn count_table(&mut self, ui: &mut egui::Ui) {
        let fibers = self.active_pane().fibers.clone();
        if fibers.is_empty() {
            return;
        }
        let d = self.fiber_drawing();
        let on = brightest_centre(&fibers).map(|b| b.0);
        section(ui, "Each fiber", None, |ui| {
            egui::ScrollArea::vertical().id_salt("count_rows").max_height(ui.available_height().max(120.0)).auto_shrink([false, true]).show(ui, |ui| {
                egui::Grid::new("count_grid").striped(true).num_columns(4).spacing(egui::vec2(16.0, 4.0)).show(ui, |ui| {
                    for h in ["Number", "Center (px)", "Center brightness", ""] {
                        ui.strong(h);
                    }
                    ui.end_row();
                    for (i, f) in fibers.iter().enumerate() {
                        ui.label(RichText::new(d.number(i).to_string()).strong());
                        ui.label(format!("{:.0}, {:.0}", f.cx, f.cy));
                        ui.label(f.centre_brightness.map(|b| format!("{b:.0}")).unwrap_or_default());
                        ui.label(if on == Some(i) { RichText::new("on").strong().color(egui::Color32::from_rgb(20, 150, 40)) } else { RichText::new("") });
                        ui.end_row();
                    }
                });
            });
        });
    }
}

/// Image formats for the Fiber count picture: (label, extension).
const COUNT_FORMATS: [(&str, &str); 2] = [("PNG", "png"), ("TIFF", "tif")];

/// Folder ("..." to choose; empty: `fallback`, the output folder) and name (blank: date and time), as in Controls > Output.
fn output_rows(ui: &mut egui::Ui, id: &str, folder: &mut String, name: &mut String, fallback: &str) {
    egui::Grid::new(id).num_columns(2).spacing(egui::vec2(10.0, 6.0)).show(ui, |ui| {
        ui.label("Folder");
        ui.horizontal(|ui| {
            let shown = if folder.is_empty() { fallback.to_string() } else { folder.clone() };
            if ui.button("...").on_hover_text("Choose the folder (else the output folder in Controls).").clicked() {
                if let Some(f) = rfd::FileDialog::new().set_title("Folder").set_directory(&shown).pick_folder() {
                    *folder = f.display().to_string();
                }
            }
            ui.add(egui::Label::new(&shown).truncate()).on_hover_text(&shown);
        });
        ui.end_row();
        ui.label("Name");
        ui.add(egui::TextEdit::singleline(name).hint_text("blank = date and time").desired_width(180.0))
            .on_hover_text("Saved as name-001, name-002, ... (nothing is overwritten).");
        ui.end_row();
    });
}

/// A framed group with a title (and a color swatch: what it looks like on the image).
pub(crate) fn section<R>(ui: &mut egui::Ui, title: &str, swatch: Option<egui::Color32>, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::group(ui.style()).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        if !title.is_empty() {
            ui.horizontal(|ui| {
                if let Some(c) = swatch {
                    let (r, _) = ui.allocate_exact_size(egui::vec2(18.0, 6.0), egui::Sense::hover());
                    ui.painter().rect_filled(r, 2.0, c);
                }
                ui.label(RichText::new(title).strong().size(15.0));
            });
            ui.add_space(4.0);
        }
        add(ui)
    }).inner
}

/// The trend as a line over the last TREND_SECONDS, zero in the middle (perpendicular to the
/// reference line, or horizontal), the scale fitting the values (at least ±0.5°).
fn trend_plot(ui: &mut egui::Ui, t: &Trend, height: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
    let mid = rect.center().y;
    painter.line_segment([egui::pos2(rect.left(), mid), egui::pos2(rect.right(), mid)], egui::Stroke::new(1.0, ui.visuals().weak_text_color()));
    let Some(&(now, _)) = t.points.back() else { return };
    let span = t.points.iter().map(|p| p.1.abs()).fold(0.5, f64::max) * 1.2;
    let font = egui::FontId::proportional(11.0);
    let weak = ui.visuals().weak_text_color();
    painter.text(rect.left_top() + egui::vec2(4.0, 2.0), egui::Align2::LEFT_TOP, format!("+{span:.1}°"), font.clone(), weak);
    painter.text(rect.left_bottom() + egui::vec2(4.0, -2.0), egui::Align2::LEFT_BOTTOM, format!("-{span:.1}°"), font, weak);
    let to = |p: &(f64, f64)| egui::pos2(
        rect.right() - ((now - p.0) / TREND_SECONDS) as f32 * rect.width(),
        mid - (p.1 / span) as f32 * (rect.height() / 2.0 - 4.0),
    );
    let pts: Vec<egui::Pos2> = t.points.iter().map(to).collect();
    if pts.len() >= 2 {
        painter.add(egui::Shape::line(pts.clone(), egui::Stroke::new(2.0, egui::Color32::from_rgb(0x2f, 0x6f, 0xd0))));
    }
    if let Some(last) = pts.last() {
        painter.circle_filled(*last, 3.5, egui::Color32::from_rgb(0x2f, 0x6f, 0xd0));
    }
}

/// A larger on/off button (for drawing on the image).
fn big_toggle(ui: &mut egui::Ui, on: bool, text: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).size(14.0)).selected(on).min_size(egui::vec2(0.0, 30.0)))
}

fn c32(c: crate::draw::Color) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_perpendicular_either_axis() {
        use super::super::annotate::perpendicular_offset as off;
        let near = |a: Option<f64>, b: f64| (a.unwrap() - b).abs() < 1e-9;
        assert!(near(off(90.5, 0.0), 0.5), "axis at 90.5 on a level line");
        assert!(near(off(0.5, 0.0), 0.5), "the other axis: same answer");
        assert!(near(off(89.0, 0.0), -1.0));
        assert!(near(off(122.0, 30.0), 2.0) && near(off(32.0, 30.0), 2.0));
        assert!(near(off(44.0, 0.0), 44.0) && near(off(46.0, 0.0), -44.0), "nearest axis past 45");
    }

    #[test]
    fn line_through_fiber_centers() {
        // A column, leaning a little: the line along it, reaching past the ends.
        let pts: Vec<(f64, f64)> = (0..5).map(|k| (100.0 + 2.0 * k as f64, 50.0 + 100.0 * k as f64)).collect();
        let (a, b) = fit_line(&pts, 30.0).unwrap();
        let deg = signed_angle(line_angle(a, b));
        assert!((deg.abs() - (90.0 - (2.0f64 / 100.0).atan().to_degrees())).abs() < 1e-6, "{deg}");
        assert!(((b.0 - a.0).hypot(b.1 - a.1) - (400.0f64.hypot(8.0) + 60.0)).abs() < 1e-6);
        assert!(fit_line(&pts[..1], 0.0).is_none());
    }

    #[test]
    fn trend_keeps_the_last_seconds() {
        let mut t = Trend::default();
        t.add(1, None, 0.5, "fast");
        t.add(1, None, 0.7, "fast");
        assert_eq!(t.points.len(), 2);
        t.add(1, Some(10.0), 0.1, "slow");
        assert_eq!((t.points.len(), t.axis), (1, "slow"), "a new reference line starts afresh");
        t.points.push_front((-100.0, 9.0));
        t.add(1, Some(10.0), 0.2, "slow");
        assert!(t.points.iter().all(|p| p.0 > -1.0), "older than {TREND_SECONDS} s dropped");
    }

    #[test]
    fn turning_the_reference_line() {
        let (a, b) = rotate_line((0.0, 100.0), (200.0, 100.0), 30.0);
        assert!((line_angle(a, b) - 30.0).abs() < 1e-9);
        assert!(((a.0 + b.0) / 2.0 - 100.0).abs() < 1e-9 && ((a.1 + b.1) / 2.0 - 100.0).abs() < 1e-9, "about its middle");
        assert!(((b.0 - a.0).hypot(b.1 - a.1) - 200.0).abs() < 1e-9, "same length");
        let (a, b) = rotate_line(a, b, -0.5);
        assert!((signed_angle(line_angle(a, b)) + 0.5).abs() < 1e-9);
    }

    #[test]
    fn line_angles_like_fiber_axes() {
        assert!((line_angle((0.0, 0.0), (10.0, 0.0)) - 0.0).abs() < 1e-9);
        assert!((line_angle((0.0, 0.0), (10.0, -10.0)) - 45.0).abs() < 1e-9, "up and right on screen = +45");
        assert!((line_angle((10.0, -10.0), (0.0, 0.0)) - 45.0).abs() < 1e-9, "either way round");
        assert!((line_angle((0.0, 0.0), (0.0, 10.0)) - 90.0).abs() < 1e-9);
    }

    #[test]
    fn rows_from_readings() {
        let mut a = Alignment::new(3, None, None, 17, false);
        a.finish(&[44.0, 46.0, 45.0], "slow", Some(30.0));
        a.finish(&[89.0, -89.0, 90.0], "fast", Some(30.0));
        assert_eq!((a.rows[0].fiber, a.rows[1].fiber, a.next_number), (17, 16, 15), "counting down from 17");
        assert!((a.rows[0].mean - 45.0).abs() < 1e-9 && (a.rows[0].std - 1.0).abs() < 1e-9);
        assert!((a.rows[1].mean.abs() - 90.0).abs() < 1e-6, "wraps at 90: {}", a.rows[1].mean);
        assert_eq!(a.rows[0].reference, Some(30.0));
        assert_eq!(a.table_lines().len(), 3);
        assert_eq!((a.rows[0].axis, a.rows[1].axis), ("slow", "fast"));
        assert!(a.table_lines()[1].starts_with("17\tslow\t+45.00°"), "{:?}", a.table_lines());
    }
}
