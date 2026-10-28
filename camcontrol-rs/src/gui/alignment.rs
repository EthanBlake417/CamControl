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

/// Readings being taken: the slow axis of each fiber at each update.
#[derive(Clone, Debug)]
pub struct Collecting {
    pub pane: u64,
    pub wanted: usize,
    pub readings: Vec<Vec<f64>>,
    /// Which axis each fiber's reading is of ("fast" / "slow"), as last measured.
    pub axes: Vec<&'static str>,
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
    pub fn new(n: usize, region: Option<Region>, reference: Option<[f64; 4]>) -> Self {
        Self {
            n: n.clamp(1, 1000),
            collecting: None,
            rows: Vec::new(),
            region,
            reference: reference.map(|r| ((r[0], r[1]), (r[2], r[3]))),
            drawing: None,
            status: String::new(),
        }
    }

    pub fn reference_deg(&self) -> Option<f64> {
        self.reference.map(|(a, b)| line_angle(a, b))
    }

    /// Rows for each fiber (numbered from 1) from the readings taken (fast axes, from `reference` if given).
    pub fn finish(&mut self, readings: &[Vec<f64>], axes: &[&'static str], reference: Option<f64>) -> usize {
        let time = chrono::Local::now().format("%H:%M:%S").to_string();
        let fibers = readings.iter().map(|r| r.len()).max().unwrap_or(0);
        for i in 0..fibers {
            let angles: Vec<f64> = readings.iter().filter_map(|r| r.get(i).copied()).collect();
            let (mean, std) = axial_stats(&angles);
            let axis = axes.get(i).copied().unwrap_or("fast");
            self.rows.push(Row { time: time.clone(), fiber: i as i64 + 1, n: angles.len(), mean: signed_angle(mean), std, reference, axis });
        }
        fibers
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
        self.alignment.collecting = Some(Collecting { pane: id, wanted: if live { self.alignment.n } else { 1 }, readings: Vec::new(), axes: Vec::new() });
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
        let angles: Vec<f64> = self.pane(pane).fibers.iter()
            .map(|f| drawing.off_perpendicular(f).unwrap_or_else(|| drawing.shown(f.fast_deg()))).collect();
        let axes: Vec<&'static str> = self.pane(pane).fibers.iter().map(|f| drawing.perpendicular_axis(f).unwrap_or("fast")).collect();
        let c = self.alignment.collecting.as_mut().unwrap();
        if angles.is_empty() {
            if !self.pane(pane).live {
                self.alignment.collecting = None;
                self.alignment.status = "No fibers found.".into();
            }
            return;
        }
        c.readings.push(angles);
        c.axes = axes;
        let (got, wanted) = (c.readings.len(), c.wanted);
        if got < wanted {
            self.alignment.status = format!("Taking readings: {got} of {wanted}");
            return;
        }
        let done = self.alignment.collecting.take().unwrap();
        let n = self.alignment.finish(&done.readings, &done.axes, drawing.reference);
        self.alignment.status = format!("Added {n} fibers ({got} reading{} each).", if got == 1 { "" } else { "s" });
        if !self.panels.is_open(PanelId::Alignment) {
            self.panels.set_open(PanelId::Alignment, true);
        }
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
        Extras {
            region: self.alignment.region,
            reference: self.alignment.reference,
            numbers: self.panels.is_open(PanelId::FiberCount),
            notes: Vec::new(),
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
        let busy = self.alignment.collecting.is_some();
        ui.horizontal_wrapped(|ui| {
            let text = if busy { "Taking readings..." } else { "Take readings  (Space)" };
            let (fill, color) = if busy { (ui.visuals().widgets.inactive.weak_bg_fill, ui.visuals().weak_text_color()) }
                                else { (egui::Color32::from_rgb(0x2f, 0x6f, 0xd0), egui::Color32::WHITE) };
            if ui.add_enabled(!busy, egui::Button::new(RichText::new(text).size(20.0).strong().color(color)).fill(fill).min_size(egui::vec2(300.0, 52.0)))
                .on_hover_text("Measure the fiber this many times (live: the next updates) and add the mean and\n\
                                standard deviation to the table: with a reference line, how far its axis is from\n\
                                perpendicular to the line; without, its fast axis from horizontal.").clicked() {
                self.take_readings();
            }
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label("Readings:");
                    ui.add_enabled(!busy, egui::DragValue::new(&mut self.alignment.n).range(1..=1000));
                    if busy && ui.button("Cancel").clicked() {
                        self.alignment.collecting = None;
                        self.alignment.status = "Cancelled.".into();
                    }
                });
                ui.label(RichText::new(&self.alignment.status).weak());
            });
        });
        ui.horizontal_wrapped(|ui| {
            if ui.button("Save measurement").on_hover_text("Save the table as CSV in the output folder.").clicked() {
                self.save_alignment(false);
            }
            if ui.button("Save measurement with photo")
                .on_hover_text("Also save the view's image with everything drawn on it (fiber axes, readout,\n\
                                region, reference line, measurements) and the table, as PNG.").clicked() {
                self.save_alignment(true);
            }
            if ui.add_enabled(!self.alignment.rows.is_empty(), egui::Button::new("Clear table")).clicked() {
                self.alignment.rows.clear();
            }
        });
        ui.separator();
        egui::ScrollArea::vertical().id_salt("align_rows").max_height(150.0).stick_to_bottom(true).auto_shrink([false, true]).show(ui, |ui| {
            egui::Grid::new("align_grid").striped(true).num_columns(6).show(ui, |ui| {
                for h in ["Fiber #", "Axis", "Mean", "Std dev", "n", "Time"] {
                    ui.strong(h);
                }
                ui.end_row();
                for r in &self.alignment.rows {
                    ui.label(r.fiber.to_string());
                    ui.label(r.axis);
                    ui.label(format!("{:+.2}°", r.mean));
                    ui.label(format!("{:.2}°", r.std));
                    ui.label(r.n.to_string());
                    ui.label(RichText::new(&r.time).weak());
                    ui.end_row();
                }
            });
        });
        ui.separator();
        // Bottom half: region of interest and reference line.
        let drawing = self.alignment.drawing;
        ui.horizontal_wrapped(|ui| {
            ui.strong("Region of interest:");
            if super::panels::toggle(ui, drawing == Some(DrawTarget::RegionRect), "Draw rectangle")
                .on_hover_text("Drag a rectangle on the image: fibers are only looked for inside it.").clicked() {
                self.start_drawing(DrawTarget::RegionRect);
            }
            if super::panels::toggle(ui, drawing == Some(DrawTarget::RegionCircle), "Draw circle")
                .on_hover_text("Drag from the center outwards: fibers are only looked for inside it.").clicked() {
                self.start_drawing(DrawTarget::RegionCircle);
            }
            if ui.add_enabled(self.alignment.region.is_some(), egui::Button::new("Clear")).clicked() {
                self.alignment.region = None;
                self.region_changed();
            }
            ui.label(RichText::new(match self.alignment.region {
                None => "whole image",
                Some(Region::Rect { .. }) => "inside the rectangle",
                Some(Region::Circle { .. }) => "inside the circle",
            }).weak());
        });
        ui.horizontal_wrapped(|ui| {
            ui.strong("Reference line:");
            if super::panels::toggle(ui, drawing == Some(DrawTarget::Reference), "Draw line")
                .on_hover_text("Drag a line on the image along the fibers' edge (e.g. the groove): the readout and\n\
                                the readings then give how far each fiber's axis is from perpendicular to it.").clicked() {
                self.start_drawing(DrawTarget::Reference);
            }
            if ui.add_enabled(self.alignment.reference.is_some(), egui::Button::new("Clear")).clicked() {
                self.alignment.reference = None;
            }
            if let Some((a, b)) = self.alignment.reference {
                let mut deg = signed_angle(line_angle(a, b));
                ui.label("at");
                let mut changed = ui.add(egui::DragValue::new(&mut deg).range(-90.0..=90.0).speed(0.01).fixed_decimals(2).suffix("°"))
                    .on_hover_text("Type or drag the line's angle (counter-clockwise from horizontal): it turns about\n\
                                    its middle. Its ends can also be dragged on the image.").changed();
                if ui.small_button("-0.1°").clicked() {
                    deg -= 0.1;
                    changed = true;
                }
                if ui.small_button("+0.1°").clicked() {
                    deg += 0.1;
                    changed = true;
                }
                if changed {
                    self.alignment.reference = Some(rotate_line(a, b, deg));
                }
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
        let fibers = self.active_pane().fibers.clone();
        ui.horizontal_wrapped(|ui| {
            if super::panels::primary(ui, "Find fibers", true, egui::vec2(120.0, 26.0))
                .on_hover_text("Find and number all the fibers in the view (turns off Only the center fiber).").clicked() {
                self.find_all_fibers();
            }
            ui.label(RichText::new(match fibers.len() {
                0 => "No fibers found yet.".to_string(),
                1 => "1 fiber".to_string(),
                n => format!("{n} fibers"),
            }).strong());
            if ui.button("Save image with information").on_hover_text("Save the view's image with the numbers and which fiber is on\n\
                                                                     (PNG), and the details (JSON), in the output folder.").clicked() {
                self.save_fiber_count();
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Top fiber is number");
            ui.add(egui::DragValue::new(&mut self.saved.count_first).range(-99999..=99999));
            ui.radio_value(&mut self.saved.count_ascending, true, "Ascending");
            ui.radio_value(&mut self.saved.count_ascending, false, "Descending");
            ui.label(RichText::new("(down from the top fiber; side by side: from the left)").weak());
        });
        let d = self.fiber_drawing();
        match brightest_centre(&fibers) {
            Some((i, _)) => {
                ui.label(RichText::new(format!("Fiber {} is on", d.number(i))).size(18.0).color(egui::Color32::from_rgb(20, 150, 40)))
                    .on_hover_text("The fiber with the brightest center (light in its core).");
            }
            None if fibers.len() == 1 => {
                ui.label(RichText::new("One fiber: nothing to compare its brightness with.").weak());
            }
            None => {}
        }
        if !fibers.is_empty() {
            let on = brightest_centre(&fibers).map(|b| b.0);
            egui::ScrollArea::vertical().id_salt("count_rows").max_height(160.0).auto_shrink([false, true]).show(ui, |ui| {
                egui::Grid::new("count_grid").striped(true).num_columns(4).show(ui, |ui| {
                    for h in ["Number", "Center (px)", "Center brightness", ""] {
                        ui.strong(h);
                    }
                    ui.end_row();
                    for (i, f) in fibers.iter().enumerate() {
                        ui.label(d.number(i).to_string());
                        ui.label(format!("{:.0}, {:.0}", f.cx, f.cy));
                        ui.label(f.centre_brightness.map(|b| format!("{b:.0}")).unwrap_or_default());
                        ui.label(if on == Some(i) { RichText::new("on").strong().color(egui::Color32::from_rgb(20, 150, 40)) } else { RichText::new("") });
                        ui.end_row();
                    }
                });
            });
        }
    }
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
        let mut a = Alignment::new(3, None, None);
        let n = a.finish(&[vec![44.0, 89.0], vec![46.0, -89.0], vec![45.0, 90.0]], &["slow", "fast"], Some(30.0));
        assert_eq!(n, 2);
        assert_eq!((a.rows[0].fiber, a.rows[1].fiber), (1, 2));
        assert!((a.rows[0].mean - 45.0).abs() < 1e-9 && (a.rows[0].std - 1.0).abs() < 1e-9);
        assert!((a.rows[1].mean.abs() - 90.0).abs() < 1e-6, "wraps at 90: {}", a.rows[1].mean);
        assert_eq!(a.rows[0].reference, Some(30.0));
        assert_eq!(a.table_lines().len(), 3);
        assert_eq!((a.rows[0].axis, a.rows[1].axis), ("slow", "fast"));
        assert!(a.table_lines()[1].starts_with("1\tslow\t+45.00°"), "{:?}", a.table_lines());
    }
}
