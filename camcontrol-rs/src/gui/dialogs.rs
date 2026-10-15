//! Dialogs: the processing tools that combine several images
//! (process_dialogs.py) and the Grid and crosshair window (overlay_settings.py).

use std::path::PathBuf;

use eframe::egui;
use image::RgbImage;
use serde_json::{json, Value};

use super::style::{color_hex, parse_color, CrosshairStyle, GridStyle, CROSSHAIR_ANCHORS, LINE_STYLES};
use crate::capture::IMAGE_EXTENSIONS;
use crate::processing::{fluorescence, focus_stack, hdr, stitch};

pub const KEYS: [&str; 4] = ["stack", "hdr", "stitch", "composite"];

pub fn title(key: &str) -> &'static str {
    match key {
        "stack" => "Focus stack",
        "hdr" => "HDR (exposure fusion)",
        "stitch" => "Stitch",
        _ => "Fluorescence composite",
    }
}

fn help(key: &str) -> &'static str {
    match key {
        "stack" => "Combines the sharp parts of images focused at different depths. Take one image at each focus step \
                    through the sample, without moving it.",
        "hdr" => "Blends images of the same view taken at different exposures, so bright and dark areas both show \
                  detail. Use 3 or more exposures, e.g. short, medium and long.",
        "stitch" => "Joins overlapping images into one larger image. Move the sample between shots so neighbouring \
                     images overlap by about a third. Large sets can take a while.",
        _ => "Colors each channel image (one per filter/stain) and adds them together. Colors are guessed from file \
              names (DAPI, FITC, GFP, TRITC, ...) where possible. Auto levels stretches each channel from its \
              faintest to its brightest signal.",
    }
}

pub fn short_name(key: &str) -> &'static str {
    match key {
        "stack" => "stack",
        "hdr" => "hdr",
        "stitch" => "stitch",
        _ => "composite",
    }
}

pub fn min_images(key: &str) -> usize {
    if key == "composite" { 1 } else { 2 }
}

const DEFAULT_ORDER: [&str; 6] = ["Blue", "Green", "Red", "Magenta", "Cyan", "Yellow"];

#[derive(Clone)]
struct Row {
    path: PathBuf,
    color: String,
    auto: bool,
    brightness: f64,
}

/// One tool's dialog; kept, so it remembers its files and options.
pub struct ProcessDialog {
    pub key: String,
    rows: Vec<Row>,
    selected: Option<usize>,
    align: bool,
    mode: String,
    crop: bool,
    pub start_folder: PathBuf,
    message: Option<String>,
}

pub enum DialogResult {
    None,
    Cancel,
    /// paths, settings (saved with the result)
    Run(Vec<PathBuf>, Value),
}

impl ProcessDialog {
    pub fn new(key: &str) -> Self {
        Self { key: key.into(), rows: Vec::new(), selected: None, align: true, mode: "scans".into(), crop: true,
               start_folder: PathBuf::new(), message: None }
    }

    fn add_row(&mut self, path: PathBuf) {
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        let color = fluorescence::guess_color(&name)
            .unwrap_or(DEFAULT_ORDER[self.rows.len() % DEFAULT_ORDER.len()])
            .to_string();
        self.rows.push(Row { path, color, auto: true, brightness: 1.0 });
    }

    pub fn set_paths(&mut self, paths: &[PathBuf]) {
        self.rows.clear();
        for p in paths {
            self.add_row(p.clone());
        }
    }

    pub fn settings(&self) -> Value {
        match self.key.as_str() {
            "stack" | "hdr" => json!({"align": self.align}),
            "stitch" => json!({"mode": self.mode, "crop": self.crop}),
            _ => json!({"channels": self.rows.iter().map(|r| json!({"color": r.color, "auto_levels": r.auto, "brightness": r.brightness})).collect::<Vec<_>>()}),
        }
    }

    pub fn ui(&mut self, ctx: &egui::Context) -> DialogResult {
        let mut result = DialogResult::None;
        let mut open = true;
        egui::Window::new(title(&self.key)).collapsible(false).resizable(true).default_size([560.0, 460.0]).open(&mut open).show(ctx, |ui| {
            ui.add(egui::Label::new(help(&self.key)).wrap());
            ui.separator();
            let composite = self.key == "composite";
            egui::ScrollArea::vertical().max_height(260.0).auto_shrink([false, true]).show(ui, |ui| {
                egui::Grid::new("files").striped(true).num_columns(if composite { 4 } else { 1 }).show(ui, |ui| {
                    ui.strong("Image");
                    if composite {
                        ui.strong("Color");
                        ui.strong("Auto levels");
                        ui.strong("Brightness");
                    }
                    ui.end_row();
                    for (i, r) in self.rows.iter_mut().enumerate() {
                        let name = r.path.file_name().unwrap_or_default().to_string_lossy().to_string();
                        if ui.selectable_label(self.selected == Some(i), name).on_hover_text(r.path.display().to_string()).clicked() {
                            self.selected = Some(i);
                        }
                        if composite {
                            egui::ComboBox::from_id_salt(("color", i)).selected_text(r.color.clone()).show_ui(ui, |ui| {
                                for (c, _) in fluorescence::COLORS {
                                    ui.selectable_value(&mut r.color, c.to_string(), c);
                                }
                            });
                            ui.checkbox(&mut r.auto, "");
                            ui.add(egui::DragValue::new(&mut r.brightness).range(0.1..=10.0).speed(0.05).fixed_decimals(1))
                                .on_hover_text("Multiplies the channel after levels. Above 1 brightens faint stains.");
                        }
                        ui.end_row();
                    }
                });
            });
            ui.horizontal(|ui| {
                if ui.button("Add images...").clicked() {
                    let mut d = rfd::FileDialog::new().set_title("Add images").add_filter("Images", &IMAGE_EXTENSIONS);
                    if self.start_folder.is_dir() {
                        d = d.set_directory(&self.start_folder);
                    }
                    for p in d.pick_files().unwrap_or_default() {
                        self.add_row(p);
                    }
                }
                if ui.button("Remove").clicked() {
                    if let Some(i) = self.selected.take() {
                        if i < self.rows.len() {
                            self.rows.remove(i);
                        }
                    }
                }
                if ui.button("Clear").clicked() {
                    self.rows.clear();
                }
            });
            ui.separator();
            match self.key.as_str() {
                "stack" => {
                    ui.checkbox(&mut self.align, "Align images first (refocusing shifts the image slightly)");
                }
                "hdr" => {
                    ui.checkbox(&mut self.align, "Align images first");
                }
                "stitch" => {
                    egui::ComboBox::from_label("Mode")
                        .selected_text(if self.mode == "scans" { "Flat sample moved under the camera (scans)" } else { "Camera turned to look around (panorama)" })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.mode, "scans".into(), "Flat sample moved under the camera (scans)");
                            ui.selectable_value(&mut self.mode, "panorama".into(), "Camera turned to look around (panorama)");
                        });
                    ui.checkbox(&mut self.crop, "Crop off the black border");
                }
                _ => {}
            }
            if let Some(m) = &self.message {
                ui.colored_label(egui::Color32::from_rgb(200, 80, 0), m);
            }
            ui.horizontal(|ui| {
                if ui.button("Run").clicked() {
                    let need = min_images(&self.key);
                    if self.rows.len() < need {
                        self.message = Some(format!("Add at least {need} image{}.", if need > 1 { "s" } else { "" }));
                    } else {
                        self.message = None;
                        result = DialogResult::Run(self.rows.iter().map(|r| r.path.clone()).collect(), self.settings());
                    }
                }
                if ui.button("Cancel").clicked() {
                    result = DialogResult::Cancel;
                }
            });
        });
        if !open {
            result = DialogResult::Cancel;
        }
        result
    }
}

/// Run a tool on loaded images (on a background thread).
pub fn process(key: &str, images: Vec<RgbImage>, settings: &Value) -> Result<RgbImage, String> {
    match key {
        "stack" => focus_stack::focus_stack(&images, settings["align"].as_bool().unwrap_or(true)).map(|r| r.0),
        "hdr" => hdr::exposure_fusion(&images, settings["align"].as_bool().unwrap_or(true)),
        "stitch" => stitch::stitch(&images, settings["mode"].as_str().unwrap_or("scans"), settings["crop"].as_bool().unwrap_or(true)),
        _ => {
            let channels: Vec<fluorescence::ChannelSettings> = serde_json::from_value(settings["channels"].clone()).map_err(|e| e.to_string())?;
            fluorescence::composite(&images, &channels)
        }
    }
}

// --- Grid and crosshair ------------------------------------------------------------------

fn line_combo(ui: &mut egui::Ui, id: &str, line: &mut String) -> bool {
    let label = LINE_STYLES.iter().find(|l| l.0 == line.as_str()).map(|l| l.1).unwrap_or("Solid");
    let mut changed = false;
    egui::ComboBox::from_id_salt(id).selected_text(label).show_ui(ui, |ui| {
        for (name, label) in LINE_STYLES {
            changed |= ui.selectable_value(line, name.to_string(), label).changed();
        }
    });
    changed
}

fn color_button(ui: &mut egui::Ui, color: &mut String) -> bool {
    let mut c = parse_color(color).unwrap_or([255, 255, 255]);
    let changed = ui.color_edit_button_srgb(&mut c).on_hover_text("Choose the line color").changed();
    if changed {
        *color = color_hex(c);
    }
    changed
}

/// The non-modal Grid and crosshair window; the overlays follow every change.
/// Returns true while open.
pub fn overlay_window(ctx: &egui::Context, tab: &mut String, grid: &mut GridStyle, crosshair: &mut CrosshairStyle) -> bool {
    let mut open = true;
    let mut close = false;
    egui::Window::new("Grid and crosshair").collapsible(false).resizable(false).open(&mut open).show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.selectable_value(tab, "grid".into(), "Grid");
            ui.selectable_value(tab, "crosshair".into(), "Crosshair");
        });
        ui.separator();
        egui::Grid::new("overlay").num_columns(2).show(ui, |ui| {
            if tab == "crosshair" {
                ui.label("Centred on:");
                let label = CROSSHAIR_ANCHORS.iter().find(|a| a.0 == crosshair.anchor).map(|a| a.1).unwrap_or("");
                egui::ComboBox::from_id_salt("anchor").selected_text(label).show_ui(ui, |ui| {
                    for (name, label) in CROSSHAIR_ANCHORS {
                        ui.selectable_value(&mut crosshair.anchor, name.to_string(), label);
                    }
                }).response.on_hover_text("Image centre: the middle of the picture (the optical axis), e.g. to centre a fiber.\n\
                    Window centre: the middle of what you're looking at, wherever you've zoomed or panned to.");
                ui.end_row();
                ui.label("Ring:");
                ui.add(egui::DragValue::new(&mut crosshair.circle).range(0..=500).suffix(" px"))
                    .on_hover_text("Radius of the ring round the centre, on screen (0: no ring)");
                ui.end_row();
                ui.label("Line width:");
                ui.add(egui::DragValue::new(&mut crosshair.width).range(0.5..=10.0).speed(0.1).suffix(" px"));
                ui.end_row();
                ui.label("Line style:");
                line_combo(ui, "cline", &mut crosshair.line);
                ui.end_row();
                ui.label("Color:");
                color_button(ui, &mut crosshair.color);
                ui.end_row();
                ui.label("Opacity:");
                ui.add(egui::Slider::new(&mut crosshair.opacity, 5..=100).suffix("%"));
                ui.end_row();
            } else {
                ui.label("Columns:");
                ui.add(egui::DragValue::new(&mut grid.columns).range(1..=100)).on_hover_text("Number of columns (vertical lines + 1)");
                ui.end_row();
                ui.label("Rows:");
                ui.horizontal(|ui| {
                    ui.add_enabled(!grid.square, egui::DragValue::new(&mut grid.rows).range(1..=100));
                    ui.checkbox(&mut grid.square, "Square cells").on_hover_text("Work out the rows from the image's shape, so every cell is square");
                });
                ui.end_row();
                ui.label("Line width:");
                ui.add(egui::DragValue::new(&mut grid.width).range(0.5..=10.0).speed(0.1).suffix(" px"));
                ui.end_row();
                ui.label("Line style:");
                line_combo(ui, "gline", &mut grid.line);
                ui.end_row();
                ui.label("Color:");
                color_button(ui, &mut grid.color);
                ui.end_row();
                ui.label("Opacity:");
                ui.add(egui::Slider::new(&mut grid.opacity, 5..=100).suffix("%"));
                ui.end_row();
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Defaults").on_hover_text("Back to the standard look").clicked() {
                if tab == "crosshair" { *crosshair = CrosshairStyle::default() } else { *grid = GridStyle::default() }
            }
            if ui.button("Close").clicked() {
                close = true;
            }
        });
    });
    open && !close
}
