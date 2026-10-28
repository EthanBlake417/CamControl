//! Fiber log panel: record the slow and fast axes and the image quality
//! (contrast, focus, noise, clipping) of every fiber at each update; a
//! per-fiber summary (mean, standard deviation) sits above the log, and both
//! can be saved to CSV (or Excel) afterwards.

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
    /// Off perpendicular to the reference line (when there is one).
    pub perp: Option<f64>,
    pub quality: Option<FiberQuality>,
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
    contrast (fiber against background, Michelson: + brighter, - darker; Good 20%+, Fair 8%+),\n\
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
    pub const HEADERS: [&'static str; 11] = ["Time", "Update", "Fiber", "Slow axis (deg)", "Fast axis (deg)", "Off perpendicular (deg)", "Contrast (%)",
                                             "Edge width (px)", "SNR", "Clipped (%)", "Quality"];
    pub const SUMMARY_HEADERS: [&'static str; 6] = ["Fiber", "Count", "Mean slow axis (deg)", "Mean fast axis (deg)", "Std (deg)",
                                                    "Mean contrast (%)"];

    /// Add one update's fibers (numbered from 1, left to right); returns how many entries were added.
    /// `reference`: the reference line's angle, the axes logged from it (else from horizontal).
    pub fn add(&mut self, fibers: &[FiberAxis], reference: Option<f64>) -> usize {
        self.updates += 1;
        let time = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f").to_string();
        for (i, f) in fibers.iter().enumerate() {
            let r = reference.unwrap_or(0.0);
            let (slow, fast) = (signed_angle(f.slow_deg - r), signed_angle(f.fast_deg() - r));
            let perp = reference.and_then(|r| super::annotate::perpendicular_offset(f.slow_deg, r));
            self.entries.push(Entry { time: time.clone(), update: self.updates, fiber: i + 1, slow, fast, perp, quality: f.image_quality });
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
            let mut row = vec![Cell::Text(e.time.clone()), Cell::Num(e.update as f64), Cell::Num(e.fiber as f64), round2(e.slow), round2(e.fast),
                               e.perp.map(round2).unwrap_or(Cell::Empty)];
            row.extend(quality_cells(e.quality.as_ref(), false));
            row
        }).collect();
        Sheet::from_strings("Fiber log", Self::HEADERS.map(String::from).to_vec(), rows)
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

    /// Everything stacked (controls, summary, log), so it fits a side column as well as the bottom.
    pub(crate) fn fiber_log_panel(&mut self, ui: &mut egui::Ui) {
        let has_rows = !self.fiber_log.entries.is_empty();
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.fiber_log.record, "Record each update").on_hover_text(
                "Log the slow and fast axes and image quality of every fiber each time the Fiber axis panel measures\n\
                 (Find axes, or every update with Update live on, live or on a playing video).");
            if ui.add_enabled(has_rows, egui::Button::new("Save to CSV..."))
                .on_hover_text("Save the log, with the summary beside it (..._summary.csv).\nPick Excel in the dialog for one .xlsx file instead.")
                .clicked() {
                self.save_fiber_log();
            }
            if ui.add_enabled(has_rows, egui::Button::new("Clear")).on_hover_text("Forget the recorded rows.").clicked() {
                self.fiber_log.clear();
            }
        });
        let log = &self.fiber_log;
        ui.label(egui::RichText::new(format!("{} rows from {} updates", log.entries.len(), log.updates)).weak());
        ui.add_space(4.0);
        ui.strong("Summary").on_hover_text("Per fiber over the whole log: number of updates, mean (μ) slow and fast axis, their standard deviation \
                                            (σ, the same for both), and mean contrast.");
        let summary = log.summary();
        if summary.is_empty() {
            ui.label(egui::RichText::new("Nothing recorded yet.").weak());
        } else {
            egui::Grid::new("fiber_log_summary").striped(true).num_columns(6).show(ui, |ui| {
                for h in ["Fiber", "n", "μ slow °", "μ fast °", "σ °", "μ contrast"] {
                    ui.strong(h);
                }
                ui.end_row();
                for s in summary {
                    ui.label(s.fiber.to_string());
                    ui.label(s.count.to_string());
                    ui.label(format!("{:+.2}", s.mean_slow));
                    ui.label(format!("{:+.2}", s.mean_fast));
                    ui.label(format!("{:.2}", s.std));
                    ui.label(s.mean_contrast.map(|c| format!("{:+.0}%", 100.0 * c)).unwrap_or_default());
                    ui.end_row();
                }
            });
        }
        ui.add_space(4.0);
        ui.strong("Log").on_hover_text(format!("Each update's angles and image quality, newest at the bottom; # is the update number.\n\n{QUALITY_HELP}"));
        let row_h = ui.text_style_height(&egui::TextStyle::Body) + ui.spacing().item_spacing.y;
        egui::ScrollArea::both().id_salt("fiber_log_rows").stick_to_bottom(true).auto_shrink([false, false])
            .show_rows(ui, row_h, log.entries.len() + 1, |ui, range| {
                egui::Grid::new("fiber_log_rows_grid").striped(true).num_columns(8).start_row(range.start).show(ui, |ui| {
                    for i in range {
                        if i == 0 {
                            for h in ["Time", "#", "Fiber", "Slow °", "Fast °", "From perp.", "Contrast", "Quality"] {
                                let r = ui.strong(h);
                                if h == "Quality" || h == "Contrast" {
                                    r.on_hover_text(QUALITY_HELP);
                                }
                            }
                        } else {
                            let e = &log.entries[i - 1];
                            ui.label(e.time.get(11..).unwrap_or(&e.time));
                            ui.label(e.update.to_string());
                            ui.label(e.fiber.to_string());
                            ui.label(format!("{:+.2}", e.slow));
                            ui.label(format!("{:+.2}", e.fast));
                            ui.label(e.perp.map(|d| format!("{d:+.2}")).unwrap_or_default());
                            let q = e.quality.as_ref();
                            ui.label(fmt_contrast(q)).on_hover_text(quality_hover(q));
                            ui.label(grade_text(q)).on_hover_text(quality_hover(q));
                        }
                        ui.end_row();
                    }
                });
            });
    }
}

#[cfg(test)]
mod tests {
    use super::axial_stats;

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
