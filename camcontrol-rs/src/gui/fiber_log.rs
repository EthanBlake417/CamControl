//! Fiber log panel: record everything the Fiber Axis table shows for every
//! fiber at each update; a per-fiber summary (mean, standard deviation) beside
//! the log, and both can be saved to CSV (or Excel) afterwards.

use eframe::egui;

use super::actions::{file_name, timestamp, with_ext};
use super::annotate::signed_angle;
use super::App;
use crate::export::{self, Cell, Sheet};
use crate::processing::fiber_axis::FiberAxis;
use crate::processing::fiber_quality::{FiberQuality, Grade};

/// One fiber's axes at one update.
#[derive(Clone, Debug)]
pub struct Entry {
    pub time: String,
    pub update: usize,
    pub fiber: usize,
    pub slow: f64,
    pub fast: f64,
    /// With a reference line: the axis nearest perpendicular to it and how far it's off.
    pub perp: Option<f64>,
    pub perp_axis: Option<&'static str>,
    pub quality: Option<FiberQuality>,
    /// The fiber as measured (everything the Fiber Axis table shows), and whether it had the brightest center.
    pub details: FiberAxis,
    pub on: bool,
}

#[derive(Default)]
pub struct FiberLog {
    pub record: bool,
    pub entries: Vec<Entry>,
    pub updates: usize,
}

/// Mean and sample standard deviation of axis angles (degrees, wrapping every 180°):
/// each angle is taken as the nearest turn to the first, so +89.9 and -89.9 average to ±90, not 0.
pub fn axial_stats(angles: &[f64]) -> (f64, f64) {
    let Some(&first) = angles.first() else { return (f64::NAN, f64::NAN) };
    let diffs: Vec<f64> = angles.iter().map(|a| signed_angle(a - first)).collect();
    let n = diffs.len() as f64;
    let mean = diffs.iter().sum::<f64>() / n;
    let std = if diffs.len() > 1 { (diffs.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / (n - 1.0)).sqrt() } else { 0.0 };
    (signed_angle(first + mean), std)
}

fn round2(v: f64) -> Cell {
    if v.is_finite() { Cell::Num((v * 100.0).round() / 100.0) } else { Cell::Empty }
}

fn round1(v: f64) -> Cell {
    if v.is_finite() { Cell::Num((v * 10.0).round() / 10.0) } else { Cell::Empty }
}

/// Export cells for a fiber's image quality: with `levels`, fiber and background
/// level first; then contrast (%), edge width (px), noise (only with `levels`), SNR, clipped (%),
/// bright parts (only with `levels`), grade.
pub(crate) fn quality_cells(q: Option<&FiberQuality>, levels: bool) -> Vec<Cell> {
    let n = if levels { 9 } else { 5 };
    let Some(q) = q else { return vec![Cell::Empty; n] };
    let mut cells = if levels { vec![round1(q.fiber), round1(q.background)] } else { Vec::new() };
    cells.push(round1(100.0 * q.contrast));
    cells.push(q.edge_px.map(round1).unwrap_or(Cell::Empty));
    if levels {
        cells.push(q.noise.map(round1).unwrap_or(Cell::Empty));
    }
    cells.push(q.snr().map(round1).unwrap_or(Cell::Empty));
    cells.push(round1(100.0 * q.clipped));
    if levels {
        cells.push(round1(q.peak));
    }
    cells.push(Cell::Text(q.grade().0.label().into()));
    cells
}

/// Short contrast text for the tables: "+34%".
pub(crate) fn fmt_contrast(q: Option<&FiberQuality>) -> String {
    q.map(|q| format!("{:+.0}%", 100.0 * q.contrast)).unwrap_or_default()
}

/// The grade as colored text for the tables.
pub(crate) fn grade_text(q: Option<&FiberQuality>) -> egui::RichText {
    let Some(q) = q else { return egui::RichText::new("") };
    let g = q.grade().0;
    let color = match g {
        Grade::Good => egui::Color32::from_rgb(40, 150, 70),
        Grade::Fair => egui::Color32::from_rgb(200, 130, 0),
        Grade::Poor => egui::Color32::from_rgb(210, 50, 50),
    };
    egui::RichText::new(g.label()).color(color)
}

/// Hover text for a fiber's quality cells.
pub(crate) fn quality_hover(q: Option<&FiberQuality>) -> String {
    q.map(|q| q.details()).unwrap_or_else(|| "Not measured: too little of the fiber or its surroundings on the image.".into())
}

pub(crate) const QUALITY_HELP: &str = "Image quality round the fiber, the worst of:\n\
    background contrast (fiber against the background round it, Michelson: + brighter, - darker; Good 20%+, Fair 8%+),\n\
    focus (how many pixels the cladding edge takes; Good up to 5% of the radius, Fair 10%),\n\
    noise (contrast step over the background noise, SNR; Good 20+, Fair 8+; not on a black background),\n\
    exposure (fiber pixels clipped at full white: Good up to 1%, Fair 10%; and its bright parts: Good 60+, Fair 30+).\n\
    Hover a value for the numbers.";

/// One fiber over the whole log. The fast axis is always 90° from the slow one,
/// so both have the same spread.
#[derive(Clone, Copy, Debug)]
pub struct FiberSummary {
    pub fiber: usize,
    pub count: usize,
    pub mean_slow: f64,
    pub mean_fast: f64,
    pub std: f64,
    /// Mean contrast (-1 to 1) of the updates where it was measured.
    pub mean_contrast: Option<f64>,
}

impl FiberLog {
    /// The log's columns: the update, the axis against the reference line, then all the Fiber Axis details.
    pub fn headers() -> Vec<String> {
        let mut h: Vec<String> = ["Time", "Update", "Fiber", "Axis", "Angle (deg)"].iter().map(|s| s.to_string()).collect();
        h.extend(super::alignment::DETAIL_HEADERS.iter().map(|s| s.to_string()));
        h.push("On".into());
        h
    }
    pub const SUMMARY_HEADERS: [&'static str; 6] = ["Fiber", "Count", "Mean slow axis (deg)", "Mean fast axis (deg)", "Std (deg)",
                                                    "Mean background contrast (%)"];

    /// Add one update's fibers (numbered from 1, left to right); returns how many entries were added.
    /// `reference`: the reference line's angle (for the Angle column; the axes are from horizontal).
    pub fn add(&mut self, fibers: &[FiberAxis], reference: Option<f64>) -> usize {
        self.updates += 1;
        let time = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f").to_string();
        let on = crate::processing::fiber_axis::brightest_centre(fibers).map(|b| b.0);
        for (i, f) in fibers.iter().enumerate() {
            let (slow, fast) = (signed_angle(f.slow_deg), signed_angle(f.fast_deg()));
            let drawing = super::annotate::FiberDrawing { reference, ..Default::default() };
            let (perp, perp_axis) = (drawing.off_perpendicular(f), drawing.perpendicular_axis(f));
            self.entries.push(Entry { time: time.clone(), update: self.updates, fiber: i + 1, slow, fast, perp, perp_axis, quality: f.image_quality,
                                      details: f.clone(), on: on == Some(i) });
        }
        fibers.len()
    }

    pub fn summary(&self) -> Vec<FiberSummary> {
        let mut fibers: Vec<usize> = self.entries.iter().map(|e| e.fiber).collect();
        fibers.sort_unstable();
        fibers.dedup();
        fibers.into_iter().map(|fiber| {
            let angles: Vec<f64> = self.entries.iter().filter(|e| e.fiber == fiber).map(|e| e.slow).collect();
            let (mean_slow, std) = axial_stats(&angles);
            let contrasts: Vec<f64> = self.entries.iter().filter(|e| e.fiber == fiber).filter_map(|e| e.quality.map(|q| q.contrast)).collect();
            let mean_contrast = (!contrasts.is_empty()).then(|| contrasts.iter().sum::<f64>() / contrasts.len() as f64);
            FiberSummary { fiber, count: angles.len(), mean_slow, mean_fast: signed_angle(mean_slow + 90.0), std, mean_contrast }
        }).collect()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.updates = 0;
    }

    pub fn sheet(&self) -> Sheet {
        let rows = self.entries.iter().map(|e| {
            let mut row = vec![Cell::Text(e.time.clone()), Cell::Text(e.update.to_string()), Cell::Text(e.fiber.to_string()),
                               e.perp_axis.map(|a| Cell::Text(a.into())).unwrap_or(Cell::Empty), e.perp.map(round2).unwrap_or(Cell::Empty)];
            row.extend(super::alignment::detail_cells(&e.details, e.details.rod_edge));
            row.push(Cell::Text(if e.on { "on".into() } else { String::new() }));
            row
        }).collect();
        Sheet::from_strings("Fiber log", Self::headers(), rows)
    }

    pub fn summary_sheet(&self) -> Sheet {
        let rows = self.summary().into_iter().map(|s| {
            vec![Cell::Num(s.fiber as f64), Cell::Num(s.count as f64), round2(s.mean_slow), round2(s.mean_fast), round2(s.std),
                 s.mean_contrast.map(|c| round1(100.0 * c)).unwrap_or(Cell::Empty)]
        }).collect();
        Sheet::from_strings("Summary", Self::SUMMARY_HEADERS.map(String::from).to_vec(), rows)
    }
}

impl App {
    /// Record each update: log this view's fibers.
    pub(crate) fn record_fibers(&mut self, id: u64) {
        if self.fiber_log.record {
            let fibers = self.pane(id).fibers.clone();
            let reference = self.alignment.reference_deg();
            self.fiber_log.add(&fibers, reference);
        }
    }

    /// Save the log as CSV (the summary beside it as ..._summary.csv), or both sheets in one Excel file.
    fn save_fiber_log(&mut self) {
        if self.fiber_log.entries.is_empty() {
            self.warn("Save fiber log", "Nothing recorded yet. Tick Record each update first.");
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .set_title("Save fiber log")
            .add_filter("CSV", &["csv"])
            .add_filter("Excel", &["xlsx"])
            .set_directory(&self.saved.folder)
            .set_file_name(format!("fiber_log_{}.csv", timestamp()))
            .save_file()
        else {
            return;
        };
        let path = with_ext(path, &["csv", "xlsx"], "csv");
        let log = &self.fiber_log;
        match export::write_tables(&[log.sheet(), log.summary_sheet()], &path, true) {
            Ok(()) => self.message(format!("Saved {} rows to {}", self.fiber_log.entries.len(), file_name(&path)), Some(8.0)),
            Err(e) => self.warn("Save fiber log", format!("Save failed: {e}")),
        }
    }

    /// Recording and the summary beside the log when there's room (the bottom edge), else stacked.
    pub(crate) fn fiber_log_panel(&mut self, ui: &mut egui::Ui) {
        if ui.available_width() > 800.0 {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(330.0);
                    self.log_recording_box(ui);
                    ui.add_space(6.0);
                    self.log_summary_box(ui);
                });
                ui.vertical(|ui| self.log_rows_box(ui));
            });
        } else {
            self.log_recording_box(ui);
            ui.add_space(6.0);
            self.log_summary_box(ui);
            ui.add_space(6.0);
            self.log_rows_box(ui);
        }
    }

    fn log_recording_box(&mut self, ui: &mut egui::Ui) {
        let has_rows = !self.fiber_log.entries.is_empty();
        super::alignment::section(ui, "Recording", None, |ui| {
            ui.checkbox(&mut self.fiber_log.record, "Record each update").on_hover_text(
                "Log everything the Fiber Axis table shows for every fiber each time it measures\n\
                 (Find axes, or every update with Update live on, live or on a playing video).");
            ui.horizontal(|ui| {
                if ui.add_enabled(has_rows, egui::Button::new("Save to CSV..."))
                    .on_hover_text("Save the log (every Fiber Axis column), with the summary beside it (..._summary.csv).\n\
                                    Pick Excel in the dialog for one .xlsx file instead.")
                    .clicked() {
                    self.save_fiber_log();
                }
                if ui.add_enabled(has_rows, egui::Button::new("Clear")).on_hover_text("Forget the recorded rows.").clicked() {
                    self.fiber_log.clear();
                }
                let log = &self.fiber_log;
                ui.label(egui::RichText::new(format!("{} rows, {} updates", log.entries.len(), log.updates)).weak());
            });
        });
    }

    fn log_summary_box(&mut self, ui: &mut egui::Ui) {
        let log = &self.fiber_log;
        super::alignment::section(ui, "Summary", None, |ui| {
            if log.entries.is_empty() {
                ui.label(egui::RichText::new("Nothing recorded yet: tick Record each update, then measure.").weak());
                return;
            }
            egui::Grid::new("fiber_log_summary").striped(true).num_columns(6).spacing(egui::vec2(12.0, 4.0)).show(ui, |ui| {
                for (h, tip) in [("Fiber", ""), ("n", "Updates logged"), ("μ slow", "Mean slow axis (°)"), ("μ fast", "Mean fast axis (°)"),
                                 ("σ", "Standard deviation (°), the same for both axes"), ("μ bg contrast", "Mean background contrast")] {
                    let r = ui.strong(h);
                    if !tip.is_empty() {
                        r.on_hover_text(tip);
                    }
                }
                ui.end_row();
                for s in log.summary() {
                    ui.label(s.fiber.to_string());
                    ui.label(s.count.to_string());
                    ui.label(format!("{:+.2}", s.mean_slow));
                    ui.label(format!("{:+.2}", s.mean_fast));
                    ui.label(format!("{:.2}", s.std));
                    ui.label(s.mean_contrast.map(|c| format!("{:+.0}%", 100.0 * c)).unwrap_or_default());
                    ui.end_row();
                }
            });
        });
    }

    /// Every update's fibers, every Fiber Axis column; the headings stay put, the rows scroll
    /// (and everything sideways when it's wider than the panel).
    fn log_rows_box(&mut self, ui: &mut egui::Ui) {
        let log = &self.fiber_log;
        if log.entries.is_empty() {
            return;
        }
        let with_angle = log.entries.iter().any(|e| e.perp.is_some());
        let cols: Vec<(&str, f32, &str)> = [
            ("Time", 64.0, ""),
            ("#", 30.0, "Update number"),
            ("Fiber", 38.0, ""),
            ("Type", 64.0, ""),
            ("Slow", 50.0, "Slow axis (°) from horizontal"),
            ("Fast", 50.0, "Fast axis (°) from horizontal"),
            ("Angle", 90.0, "With a reference line: the axis nearest perpendicular to it, and how far off (°)."),
            ("Clarity", 64.0, "How sure the axis is (rod outline found, or elongation)."),
            ("Rod edge", 62.0, "Focus: the stress rods' edge width (px). Smaller is sharper."),
            ("Rod contrast", 80.0, "How clearly the stress rods stand out from the cladding round them."),
            ("Bg contrast", 76.0, "Background contrast: the fiber against the background round it."),
            ("Quality", 50.0, QUALITY_HELP),
            ("Method", 58.0, ""),
            ("Center (px)", 80.0, ""),
            ("Ø (px)", 44.0, "Diameter in image pixels"),
            ("Brightness", 72.0, "Brightest spot near the center (0-255); on: the brightest fiber."),
        ].into_iter().filter(|c| c.0 != "Angle" || with_angle).collect();
        let cell = |ui: &mut egui::Ui, w: f32, text: egui::RichText| {
            ui.add_sized([w, 18.0], egui::Label::new(text).truncate())
        };
        super::alignment::section(ui, "Log", None, |ui| {
            egui::ScrollArea::horizontal().id_salt("fiber_log_wide").auto_shrink([false, true]).show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        for &(h, w, tip) in &cols {
                            let r = cell(ui, w, egui::RichText::new(h).strong());
                            if !tip.is_empty() {
                                r.on_hover_text(tip);
                            }
                        }
                    });
                    ui.separator();
                    let row_h = 18.0 + ui.spacing().item_spacing.y;
                    egui::ScrollArea::vertical().id_salt("fiber_log_rows").stick_to_bottom(true).auto_shrink([true, false])
                        .show_rows(ui, row_h, log.entries.len(), |ui, range| {
                            for i in range {
                                let e = &log.entries[i];
                                let f = &e.details;
                                let q = e.quality.as_ref();
                                let clarity = match f.coverage {
                                    Some(c) => format!("{:.0}%", c * 100.0),
                                    None => format!("{:.2}", f.elongation),
                                } + if f.clear() { "" } else { " unclear" };
                                let texts: Vec<(&str, egui::RichText)> = vec![
                                    ("Time", e.time.get(11..19).unwrap_or(&e.time).into()),
                                    ("#", e.update.to_string().into()),
                                    ("Fiber", e.fiber.to_string().into()),
                                    ("Type", f.fiber_type.clone().into()),
                                    ("Slow", format!("{:+.2}", e.slow).into()),
                                    ("Fast", format!("{:+.2}", e.fast).into()),
                                    ("Angle", e.perp.zip(e.perp_axis).map(|(d, a)| format!("{a} {d:+.2}")).unwrap_or_default().into()),
                                    ("Clarity", clarity.into()),
                                    ("Rod edge", f.rod_edge.map(|v| format!("{v:.1} px")).unwrap_or_default().into()),
                                    ("Rod contrast", f.rod_contrast.map(|c| format!("{:+.0}%", 100.0 * c)).unwrap_or_default().into()),
                                    ("Bg contrast", fmt_contrast(q).into()),
                                    ("Quality", grade_text(q)),
                                    ("Method", super::panels::short_method(&f.method).into()),
                                    ("Center (px)", format!("{:.0}, {:.0}", f.cx, f.cy).into()),
                                    ("Ø (px)", format!("{:.0}", 2.0 * f.radius).into()),
                                    ("Brightness", f.centre_brightness.map(|b| format!("{b:.0}{}", if e.on { " on" } else { "" })).unwrap_or_default().into()),
                                ];
                                ui.horizontal(|ui| {
                                    for (name, text) in texts {
                                        let Some(&(_, w, _)) = cols.iter().find(|c| c.0 == name) else { continue };
                                        let r = cell(ui, w, text);
                                        if name == "Quality" || name == "Bg contrast" {
                                            r.on_hover_text(quality_hover(q));
                                        }
                                    }
                                });
                            }
                        });
                });
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::axial_stats;

    #[test]
    fn the_log_keeps_every_fiber_axis_column() {
        let mut f = crate::processing::fiber_axis::FiberAxis::new(100.0, 120.0, 50.0, 30.0, 1.5, "Panda", 2);
        f.rod_edge = Some(2.5);
        f.rod_contrast = Some(-0.4);
        let mut log = super::FiberLog::default();
        log.add(&[f.clone(), f], None);
        let sheet = log.sheet();
        assert_eq!(sheet.headers.len(), super::FiberLog::headers().len());
        assert!(sheet.headers.iter().any(|h| h == "Rod contrast (%)") && sheet.headers.iter().any(|h| h == "Rod edge (px)"));
        assert_eq!(sheet.rows.len(), 2);
        assert!(sheet.rows.iter().all(|r| r.len() == sheet.headers.len()));
    }

    #[test]
    fn stats_wrap_at_90() {
        let (m, s) = axial_stats(&[89.0, -89.0]);
        assert!((m.abs() - 90.0).abs() < 1e-9, "{m}");
        assert!((s - 2f64.sqrt()).abs() < 1e-9, "{s}");
        let (m, s) = axial_stats(&[1.0, 2.0, 3.0]);
        assert!((m - 2.0).abs() < 1e-9 && (s - 1.0).abs() < 1e-9);
        assert_eq!(axial_stats(&[5.0]), (5.0, 0.0));
    }
}
