//! Histogram panel: the share of the active view's pixels at each brightness
//! (0-255), as a percentage or as its logarithm (log10 of the percentage, which
//! shows the few pixels at the ends, e.g. clipped ones, beside the many).
//! Brightness is luma (0.299 R + 0.587 G + 0.114 B); R, G and B can be shown too.

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke};
use image::RgbImage;

use super::actions::file_name;
use super::App;
use crate::export::{self, Cell, Sheet};

const LIVE_INTERVAL: Duration = Duration::from_millis(200); // recount live frames at most this often
const LOG_FLOOR: f64 = -6.0; // log10 of the smallest percentage drawn
const CHANNELS: [&str; 4] = ["Brightness", "Red", "Green", "Blue"];
const CHANNEL_COLORS: [Color32; 4] = [
    Color32::from_rgb(90, 90, 90),
    Color32::from_rgb(220, 40, 40),
    Color32::from_rgb(30, 160, 30),
    Color32::from_rgb(40, 80, 230),
];

/// Pixel counts per value: [brightness, red, green, blue][0..256].
pub type Counts = [[u64; 256]; 4];

pub fn count(img: &RgbImage) -> Counts {
    let mut c = [[0u64; 256]; 4];
    for p in img.pixels() {
        let [r, g, b] = p.0;
        let y = (299 * r as u32 + 587 * g as u32 + 114 * b as u32 + 500) / 1000;
        c[0][y as usize] += 1;
        c[1][r as usize] += 1;
        c[2][g as usize] += 1;
        c[3][b as usize] += 1;
    }
    c
}

/// Percentages of the total.
pub fn percents(counts: &[u64; 256]) -> [f64; 256] {
    let total = counts.iter().sum::<u64>().max(1) as f64;
    counts.map(|n| 100.0 * n as f64 / total)
}

/// log10 of a percentage; None for none at all.
pub fn log_percent(p: f64) -> Option<f64> {
    (p > 0.0).then(|| p.log10())
}

#[derive(Default)]
pub struct Histogram {
    pub log: bool,
    pub colors: bool,
    counts: Option<Box<Counts>>,
    /// The image counted (by address), and when.
    counted: Option<(usize, Instant)>,
}

impl Histogram {
    pub fn new(log: bool, colors: bool) -> Self {
        Self { log, colors, ..Default::default() }
    }

    /// Count the image if it's new (live frames: at most every LIVE_INTERVAL).
    pub fn update(&mut self, image: Option<&Arc<RgbImage>>, live: bool) {
        let Some(img) = image else {
            self.counts = None;
            self.counted = None;
            return;
        };
        let key = Arc::as_ptr(img) as usize;
        if let Some((k, at)) = self.counted {
            if k == key || (live && self.counts.is_some() && at.elapsed() < LIVE_INTERVAL) {
                return;
            }
        }
        self.counts = Some(Box::new(count(img)));
        self.counted = Some((key, Instant::now()));
    }

    pub fn counts(&self) -> Option<&Counts> {
        self.counts.as_deref()
    }

    /// Value, then percent and log10(percent) for each channel.
    pub fn sheet(&self) -> Option<Sheet> {
        let counts = self.counts()?;
        let mut headers = vec!["Value".to_string()];
        for name in CHANNELS {
            headers.push(format!("{name} (% of pixels)"));
            headers.push(format!("{name} log10(%)"));
        }
        let pct: Vec<[f64; 256]> = counts.iter().map(percents).collect();
        let rows = (0..256)
            .map(|v| {
                let mut row = vec![Cell::Num(v as f64)];
                for p in &pct {
                    row.push(Cell::Num(p[v]));
                    row.push(log_percent(p[v]).map(Cell::Num).unwrap_or(Cell::Empty));
                }
                row
            })
            .collect();
        Some(Sheet { name: "Histogram".into(), headers, rows })
    }
}

/// Mean, lowest and highest value present.
pub fn stats(counts: &[u64; 256]) -> Option<(f64, usize, usize)> {
    let total: u64 = counts.iter().sum();
    if total == 0 {
        return None;
    }
    let mean = counts.iter().enumerate().map(|(v, &n)| v as f64 * n as f64).sum::<f64>() / total as f64;
    let lo = counts.iter().position(|&n| n > 0)?;
    let hi = counts.iter().rposition(|&n| n > 0)?;
    Some((mean, lo, hi))
}

/// A round number at or above v (1, 2, 5 x 10^n).
fn nice_ceiling(v: f64) -> f64 {
    if v <= 0.0 {
        return 1.0;
    }
    let p = 10f64.powf(v.log10().floor());
    [1.0, 2.0, 5.0, 10.0].iter().map(|m| m * p).find(|&c| c >= v * 0.9999).unwrap_or(10.0 * p)
}

fn fmt_tick(t: f64) -> String {
    if t != 0.0 && t < 0.01 {
        return format!("{t:.0e}%");
    }
    let s = format!("{t:.3}");
    format!("{}%", s.trim_end_matches('0').trim_end_matches('.'))
}

fn fmt_percent(p: f64) -> String {
    if p == 0.0 {
        "0%".into()
    } else if p >= 1.0 {
        format!("{p:.2}%")
    } else if p >= 0.001 {
        format!("{p:.4}%")
    } else {
        format!("{p:.2e}%")
    }
}

impl App {
    pub(crate) fn histogram_panel(&mut self, ui: &mut egui::Ui) {
        let (image, live) = {
            let p = self.active_pane();
            (p.image(), p.live)
        };
        self.histogram.update(image.as_ref(), live);
        let h = &mut self.histogram;
        let mut save = false;
        ui.horizontal(|ui| {
            ui.checkbox(&mut h.log, "Logarithmic").on_hover_text(
                "Show log10 of the percentage: values few pixels have (e.g. clipped at 0 or 255)\nshow beside the common ones.");
            ui.checkbox(&mut h.colors, "Red, green, blue").on_hover_text("Also show each color channel.");
            if let Some((mean, lo, hi)) = h.counts().and_then(|c| stats(&c[0])) {
                let pct = percents(&h.counts().unwrap()[0]);
                ui.separator();
                ui.label(format!("Mean {mean:.1}   Lowest {lo}   Highest {hi}"));
                ui.separator();
                ui.label(format!("At 0: {}   At 255: {}", fmt_percent(pct[0]), fmt_percent(pct[255])))
                    .on_hover_text("Pixels at the ends are probably clipped (too dark or too bright to measure).");
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled(h.counts().is_some(), egui::Button::new("Save..."))
                    .on_hover_text("Save the table (each value's percentage and its log10) as Excel or CSV.").clicked()
                {
                    save = true;
                }
            });
        });
        if save {
            self.save_histogram();
        }
        let Some(counts) = self.histogram.counts() else {
            ui.label("No image in the active view.");
            return;
        };
        let (log, colors) = (self.histogram.log, self.histogram.colors);
        let shown: Vec<usize> = if colors { vec![0, 1, 2, 3] } else { vec![0] };
        let pct: Vec<[f64; 256]> = counts.iter().map(percents).collect();
        let size = egui::vec2(ui.available_width(), ui.available_height().max(120.0));
        let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
        let painter = ui.painter_at(rect);
        let plot = Rect::from_min_max(rect.min + egui::vec2(58.0, 6.0), rect.max - egui::vec2(10.0, 20.0));
        painter.rect_filled(plot, 0.0, Color32::WHITE);
        painter.rect_stroke(plot, 0.0, Stroke::new(1.0, Color32::GRAY), egui::StrokeKind::Inside);
        // y range and ticks
        let top = shown.iter().flat_map(|&c| pct[c].iter().cloned()).fold(0.0, f64::max);
        let (y0, y1) = if log {
            let low = shown.iter().flat_map(|&c| pct[c].iter().cloned()).filter(|&p| p > 0.0).fold(f64::INFINITY, f64::min);
            ((low.log10().floor()).max(LOG_FLOOR).min(-1.0), top.log10().ceil().max(0.0))
        } else {
            (0.0, nice_ceiling(top))
        };
        let to_y = |p: f64| -> f32 {
            let v = if log { log_percent(p).unwrap_or(f64::NEG_INFINITY).max(y0) } else { p };
            plot.bottom() - ((v - y0) / (y1 - y0).max(1e-12)) as f32 * plot.height()
        };
        let to_x = |v: f64| plot.left() + (v as f32 + 0.5) / 256.0 * plot.width();
        let font = FontId::proportional(11.0);
        let ticks: Vec<f64> = if log {
            (y0 as i64..=y1 as i64).map(|e| 10f64.powi(e as i32)).collect()
        } else {
            (0..=4).map(|k| y1 * k as f64 / 4.0).collect()
        };
        for t in ticks {
            let y = to_y(t);
            painter.line_segment([Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)], Stroke::new(1.0, Color32::from_gray(230)));
            painter.text(Pos2::new(plot.left() - 4.0, y), Align2::RIGHT_CENTER, fmt_tick(t), font.clone(), Color32::DARK_GRAY);
        }
        for v in [0, 64, 128, 192, 255] {
            let x = to_x(v as f64);
            painter.text(Pos2::new(x, plot.bottom() + 3.0), Align2::CENTER_TOP, v.to_string(), font.clone(), Color32::DARK_GRAY);
        }
        // Brightness as bars, colors as lines.
        for &c in &shown {
            if c == 0 {
                let fill = if colors { Color32::from_gray(200) } else { CHANNEL_COLORS[0] };
                for (v, &p) in pct[0].iter().enumerate() {
                    if p > 0.0 {
                        let (xa, xb) = (to_x(v as f64 - 0.5), to_x(v as f64 + 0.5));
                        painter.rect_filled(Rect::from_min_max(Pos2::new(xa, to_y(p)), Pos2::new(xb.max(xa + 1.0), plot.bottom())), 0.0, fill);
                    }
                }
            } else {
                let pts: Vec<Pos2> = pct[c].iter().enumerate().map(|(v, &p)| Pos2::new(to_x(v as f64), to_y(p))).collect();
                painter.add(egui::Shape::line(pts, Stroke::new(1.5, CHANNEL_COLORS[c])));
            }
        }
        // The value under the mouse.
        if let Some(pos) = response.hover_pos().filter(|p| plot.contains(*p)) {
            let v = (((pos.x - plot.left()) / plot.width() * 256.0).floor() as usize).min(255);
            let x = to_x(v as f64);
            painter.line_segment([Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())], Stroke::new(1.0, Color32::from_rgb(30, 80, 160)));
            let text = shown
                .iter()
                .map(|&c| {
                    let p = pct[c][v];
                    let l = log_percent(p).map(|l| format!(" (log {l:.2})")).unwrap_or_default();
                    format!("{}: {}{l}", CHANNELS[c], fmt_percent(p))
                })
                .collect::<Vec<_>>()
                .join("\n");
            let anchor = if pos.x > plot.center().x { Align2::RIGHT_TOP } else { Align2::LEFT_TOP };
            let at = Pos2::new(if pos.x > plot.center().x { x - 6.0 } else { x + 6.0 }, plot.top() + 4.0);
            let galley_text = format!("Value {v}\n{text}");
            let r = painter.text(at, anchor, &galley_text, FontId::proportional(12.0), Color32::BLACK);
            painter.rect_filled(r.expand(3.0), 2.0, Color32::from_white_alpha(220));
            painter.text(at, anchor, galley_text, FontId::proportional(12.0), Color32::BLACK);
        }
    }

    fn save_histogram(&mut self) {
        let Some(sheet) = self.histogram.sheet() else { return };
        let Some(path) = self.export_path("Save histogram", "histogram") else { return };
        match export::write_tables(&[sheet], &path, false) {
            Ok(()) => self.message(format!("Saved the histogram to {}", file_name(&path)), Some(6.0)),
            Err(e) => self.warn("Save histogram", format!("Saving failed: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_percents() {
        let mut img = RgbImage::from_pixel(10, 10, image::Rgb([0, 0, 0]));
        for x in 0..10 {
            img.put_pixel(x, 0, image::Rgb([255, 255, 255]));
        }
        img.put_pixel(0, 1, image::Rgb([255, 0, 0]));
        let c = count(&img);
        let p = percents(&c[0]);
        assert_eq!((p[255], p[76], p[0]), (10.0, 1.0, 89.0));
        assert_eq!(percents(&c[1])[255], 11.0);
        assert_eq!(log_percent(p[255]), Some(1.0));
        assert_eq!(log_percent(p[1]), None);
        assert_eq!(stats(&c[0]).map(|s| (s.1, s.2)), Some((0, 255)));
        let mut h = Histogram::default();
        h.update(Some(&Arc::new(img)), false);
        let s = h.sheet().unwrap();
        assert_eq!((s.rows.len(), s.headers.len()), (256, 9));
        assert_eq!(nice_ceiling(37.0), 50.0);
    }
}
