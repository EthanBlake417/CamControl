//! Alignment panel: take n readings of every fiber's slow axis (Space) and
//! add their mean and spread to a table, against a reference line if one is
//! drawn; a region of interest the fiber search keeps to; and saving the
//! table, alone or with the marked-up photo. Also the Fiber count panel:
//! the fibers numbered from the top one's number, up or down, and which one
//! is on (has the brightest center).

use eframe::egui::{self, RichText};
use serde_json::json;

use super::actions::{file_name, timestamp};
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
        self.rows.push(Row { time, fiber: self.next_number, n: readings.len(), mean: signed_angle(mean), std, reference, axis });
        self.next_number += if self.ascending { 1 } else { -1 };
        self.rows.last().unwrap()
    }

    pub fn sheet(&self) -> Sheet {
        let r2 = |v: f64| Cell::Num((v * 100.0).round() / 100.0);
        let rows = self.rows.iter().map(|r| vec![
            Cell::Text(r.time.clone()), Cell::Num(r.fiber as f64), Cell::Num(r.n as f64), Cell::Text(r.axis.into()), r2(r.mean), r2(r.std),
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
        self.alignment.collecting = Some(Collecting { pane: id, wanted: if live { self.alignment.n } else { 1 }, readings: Vec::new(), axis: "fast" });
        self.alignment.status = "Taking readings...".into();
        if !live {
            if self.pane(id).fibers.is_empty() {
                self.find_fiber_axes(true);
            } else {
                self.alignment_reading(id);
            }
        }
    }

    /// A new measurement of a view's fibers: one reading, if readings are being taken there.
    pub(crate) fn alignment_reading(&mut self, pane: u64) {
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
        let c = self.alignment.collecting.as_mut().unwrap();
        c.readings.push(angle);
        c.axis = axis;
        let (got, wanted) = (c.readings.len(), c.wanted);
        if got < wanted {
            self.alignment.status = format!("Taking readings: {got} of {wanted}");
            return;
        }
        let done = self.alignment.collecting.take().unwrap();
        let fiber = self.alignment.finish(&done.readings, done.axis, drawing.reference).fiber;
        self.alignment.status = format!("Added fiber {fiber} ({got} reading{}).", if got == 1 { "" } else { "s" });
        if !self.panels.is_open(PanelId::Alignment) {
            self.panels.set_open(PanelId::Alignment, true);
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

    pub(crate) fn save_alignment(&mut self, with_photo: bool) {
        if self.alignment.rows.is_empty() {
            self.warn("Save measurement", "No readings yet: press Take readings (Space) first.");
            return;
        }
        let folder = std::path::PathBuf::from(&self.saved.folder);
        let _ = std::fs::create_dir_all(&folder);
        let stem = format!("alignment_{}", timestamp());
        let csv = folder.join(format!("{stem}.csv"));
        if let Err(e) = export::write_tables(&[self.alignment.sheet()], &csv, false) {
            self.warn("Save measurement", format!("Save failed: {e}"));
            return;
        }
        if !with_photo {
            self.message(format!("Saved {}", file_name(&csv)), Some(8.0));
            return;
        }
        let Some(image) = self.active_pane().image() else {
            self.warn("Save measurement with photo", "No image in the view.");
            return;
        };
        let png = folder.join(format!("{stem}.png"));
        let markup = self.markup_of(self.active, self.alignment.table_lines());
        match render_markup(&image, &markup).save(&png) {
            Ok(()) => self.message(format!("Saved {} and {}", file_name(&png), file_name(&csv)), Some(8.0)),
            Err(e) => self.warn("Save measurement with photo", format!("Save failed: {e}")),
        }
    }

    pub(crate) fn alignment_panel(&mut self, ui: &mut egui::Ui) {
        // Side by side when there's room (the bottom edge), else stacked (a side edge).
        let w = ui.available_width();
        if w > 1000.0 {
            ui.columns(3, |c| {
                self.align_readings_box(&mut c[0]);
                self.align_setup_box(&mut c[1]);
                self.align_table(&mut c[2]);
            });
        } else if w > 600.0 {
            ui.columns(2, |c| {
                self.align_readings_box(&mut c[0]);
                self.align_setup_box(&mut c[1]);
            });
            ui.add_space(6.0);
            self.align_table(ui);
        } else {
            self.align_readings_box(ui);
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
            if !self.alignment.status.is_empty() {
                ui.label(RichText::new(&self.alignment.status).weak());
            }
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if ui.button("Save measurement").on_hover_text("Save the table as CSV in the output folder.").clicked() {
                    self.save_alignment(false);
                }
                if ui.button("Save with photo")
                    .on_hover_text("Save the table (CSV) and the view's image with everything drawn on it and the\n\
                                    table (PNG) in the output folder.").clicked() {
                    self.save_alignment(true);
                }
            });
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
            if ui.button("Clear table").clicked() {
                self.alignment.rows.clear();
            }
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
        let folder = std::path::PathBuf::from(&self.saved.folder);
        let _ = std::fs::create_dir_all(&folder);
        let stem = format!("fibers_{}", timestamp());
        let (png, js) = (folder.join(format!("{stem}.png")), folder.join(format!("{stem}.json")));
        let notes = self.count_notes(&fibers);
        let mut markup = self.markup_of(id, notes.clone());
        markup.extras.numbers = true;
        let d = self.fiber_drawing();
        let on = brightest_centre(&fibers).map(|b| b.0);
        let info = json!({
            "image": self.pane(id).path.as_ref().map(|p| p.display().to_string()),
            "count": fibers.len(),
            "top_number": d.first_number,
            "ascending": d.ascending,
            "on": on.map(|i| d.number(i)),
            "notes": notes,
            "fibers": fibers.iter().enumerate().map(|(i, f)| json!({
                "number": d.number(i), "center_px": [f.cx, f.cy], "diameter_px": 2.0 * f.radius,
                "center_brightness": f.centre_brightness, "on": on == Some(i),
                "slow_axis_deg": d.shown(f.slow_deg), "fast_axis_deg": d.shown(f.fast_deg()),
                "axis": d.perpendicular_axis(f), "angle_deg": d.off_perpendicular(f),
                "reference_line_deg": d.reference.map(signed_angle),
            })).collect::<Vec<_>>(),
        });
        let r = render_markup(&image, &markup).save(&png).map_err(|e| e.to_string())
            .and_then(|_| std::fs::write(&js, serde_json::to_string_pretty(&info).unwrap()).map_err(|e| e.to_string()));
        match r {
            Ok(()) => self.message(format!("Saved {} and {}", file_name(&png), file_name(&js)), Some(8.0)),
            Err(e) => self.warn("Save fiber count", format!("Save failed: {e}")),
        }
    }

    pub(crate) fn fiber_count_panel(&mut self, ui: &mut egui::Ui) {
        let w = ui.available_width();
        if w > 1000.0 {
            ui.columns(3, |c| {
                self.count_find_box(&mut c[0]);
                self.count_numbering_box(&mut c[1]);
                self.count_table(&mut c[2]);
            });
        } else if w > 600.0 {
            ui.columns(2, |c| {
                self.count_find_box(&mut c[0]);
                self.count_numbering_box(&mut c[1]);
            });
            ui.add_space(6.0);
            self.count_table(ui);
        } else {
            self.count_find_box(ui);
            self.count_numbering_box(ui);
            self.count_table(ui);
        }
    }

    fn count_find_box(&mut self, ui: &mut egui::Ui) {
        let fibers = self.active_pane().fibers.clone();
        let d = self.fiber_drawing();
        section(ui, "Fibers", None, |ui| {
            let button = egui::Button::new(RichText::new("Find fibers").size(20.0).strong().color(egui::Color32::WHITE))
                .fill(egui::Color32::from_rgb(0x2f, 0x6f, 0xd0)).min_size(egui::vec2(ui.available_width(), 44.0));
            if ui.add(button).on_hover_text("Find and number all the fibers in the view (turns off Only the center fiber).").clicked() {
                self.find_all_fibers();
            }
            ui.add_space(4.0);
            ui.label(RichText::new(match fibers.len() {
                0 => "No fibers found yet.".to_string(),
                1 => "1 fiber".to_string(),
                n => format!("{n} fibers"),
            }).size(16.0).strong());
            match brightest_centre(&fibers) {
                Some((i, _)) => {
                    ui.label(RichText::new(format!("Fiber {} is on", d.number(i))).size(20.0).strong().color(egui::Color32::from_rgb(20, 150, 40)))
                        .on_hover_text("The fiber with the brightest center (light in its core).");
                }
                None if fibers.len() == 1 => {
                    ui.label(RichText::new("One fiber: nothing to compare its brightness with.").weak());
                }
                None => {}
            }
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
            ui.add_space(4.0);
            if ui.button("Save image with information").on_hover_text("Save the view's image with the numbers and which fiber is on\n\
                                                                     (PNG), and the details (JSON), in the output folder.").clicked() {
                self.save_fiber_count();
            }
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

/// A framed group with a title (and a color swatch: what it looks like on the image).
fn section<R>(ui: &mut egui::Ui, title: &str, swatch: Option<egui::Color32>, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::group(ui.style()).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            if let Some(c) = swatch {
                let (r, _) = ui.allocate_exact_size(egui::vec2(18.0, 6.0), egui::Sense::hover());
                ui.painter().rect_filled(r, 2.0, c);
            }
            ui.label(RichText::new(title).strong().size(15.0));
        });
        ui.add_space(4.0);
        add(ui)
    }).inner
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
