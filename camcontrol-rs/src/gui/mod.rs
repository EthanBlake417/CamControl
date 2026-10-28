//! The main window (main_window.py): views in the middle (each showing a
//! camera, an image file, a video or a processing result), panels on edge
//! stripes (Controls, Captures, Measurements, Alignment, Fiber axis, Fiber log, Fiber count).
//!
//! Shortcuts:
//!     Space       take readings (Alignment) L    live / freeze
//!     G           grid                     C     crosshair
//!     F           fit image to window      1     100%
//!     Ctrl+= / -  zoom in / out (or the mouse wheel)
//!     Ctrl+O      open an image or video   Ctrl+S  save the active view's image
//!     Ctrl+E      export measurements      Ctrl+Shift+S  save settings
//!     Ctrl+K      one view / several views Ctrl+Shift+K  add a view
//!     Ctrl+1..6   show / hide Controls / Captures / Alignment / Fiber axis / Fiber log / Fiber count
//!     Ctrl+Q      quit
//!
//! Each view has its own source, live/frozen state, measurements and fiber
//! axes. The panels work on the active view (clicked last).
//! Controls work on the active view's camera, or, if it has none, on the
//! camera view used last. With Link views on, Capture, Live/Freeze, Fit,
//! 100% and Find fiber axes act on every view.

// Without the "extras" feature, the Measurements panel and processing are left out (their code stays).
#![cfg_attr(not(feature = "extras"), allow(dead_code, unused_imports, unused_variables))]

mod actions;
pub mod annotate;
mod dialogs;
mod fiber_log;
mod gallery;
mod histogram;
pub(crate) mod alignment;
mod panels;
mod player;
mod settings;
pub mod stripes;
pub mod style;
pub mod view;
pub mod views;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Instant;

use eframe::egui::{self, Color32, Key, KeyboardShortcut, Modifiers, RichText};
use image::RgbImage;
use serde_json::Value;

use crate::calibration::Calibration;
use crate::camera::worker::CameraLink;
use crate::camera::DeviceKey;
use crate::measure::Measurement;
use crate::processing::fiber_axis::{Detection, FiberAxis};
use crate::processing::flatfield::FlatField;

use dialogs::ProcessDialog;
use gallery::Gallery;
use player::Player;
use settings::Saved;
use stripes::{Edge, PanelId, PanelLayout, STRIPE};
use style::{CrosshairStyle, FiberLineStyle, GridStyle};
use view::{ImageView, Overlays, ViewEvent};

const SETTINGS_KEY: &str = "camcontrol";
const FIBER_LIVE_INTERVAL: f64 = 0.25; // how often Update live re-measures each live view
const TUNE_DELAY: f64 = 0.15; // re-measure this long after a detection slider stops moving

/// An unsaved processing result shown in a view.
#[derive(Clone)]
pub struct ResultInfo {
    pub short_name: String,
    pub meta: Value,
}

/// One view: what it shows, and the measurements, marks and fiber axes made on it.
pub struct Pane {
    pub id: u64,
    pub view: ImageView,
    pub measurements: Vec<Measurement>,
    pub next_id: usize,
    pub measure_selected: Option<usize>,
    pub fibers: Vec<FiberAxis>,
    pub fiber_selected: Option<usize>,
    /// Kept open while it shows a file, so L returns to it.
    pub camera: Option<CameraLink>,
    pub live: bool,
    pub path: Option<PathBuf>,
    pub title: String,
    pub result: Option<ResultInfo>,
    pub fiber_last: Option<Instant>,
    /// Live updates follow the fibers (instead of searching the whole frame)
    /// only once two searches in a row found the same ones.
    pub fiber_locked: bool,
    /// The details saved with the open file (exposure, gain, ...): one line, and all of them.
    pub details: Option<(String, String)>,
}

impl Pane {
    fn new(id: u64) -> Self {
        Self {
            id,
            view: ImageView::default(),
            measurements: Vec::new(),
            next_id: 1,
            measure_selected: None,
            fibers: Vec::new(),
            fiber_selected: None,
            camera: None,
            live: false,
            path: None,
            title: String::new(),
            result: None,
            fiber_last: None,
            fiber_locked: false,
            details: None,
        }
    }

    pub fn image(&self) -> Option<Arc<RgbImage>> {
        self.view.image().cloned()
    }
}

/// Something to do once the user has answered a question.
#[derive(Clone)]
pub enum Action {
    ClearAnnotations(u64),
    OpenImage(u64, PathBuf),
    OpenVideo(u64, PathBuf),
    ShowResult { pane: u64, image: Arc<RgbImage>, title: String, info: ResultInfo },
    DeleteFiles(Vec<PathBuf>),
    ClearMeasurements(u64),
    UseViewAsFlat,
    RemoveView(u64),
    SetPaneCamera(u64, Option<DeviceKey>),
    Reconnect,
    Quit,
}

pub enum Modal {
    Message { title: String, text: String },
    Confirm { title: String, text: String, yes: Vec<Action>, no: Vec<Action> },
}

pub struct ProcessJob {
    rx: Receiver<Result<RgbImage, String>>,
    title: String,
    info: ResultInfo,
    pane: u64,
    tool: String,
}

#[derive(Clone, PartialEq)]
pub(crate) enum FiberKind {
    Live,
    Image,
    Find,
}

struct FiberRequest {
    pane: u64,
    image: Arc<RgbImage>,
    kind: FiberKind,
    from_camera: bool,
    generation: u64,
    /// Live frames: the fibers measured last, to follow instead of searching again.
    previous: Option<Vec<(f64, f64, f64)>>,
    method: String,
    detection: Detection,
    centre_only: bool,
    region: Option<crate::processing::fiber_axis::Region>,
}

pub struct FiberSettings {
    pub live: bool,
    pub method: String,
    pub show_parts: bool,
    pub centre_only: bool,
    pub advanced: bool,
    pub detection: Detection,
    /// The automatic threshold last measured (shown when Auto is on).
    pub shown_threshold: f64,
}

impl App {
    /// The View menu's items: in the menu bar, and on right-clicking a view.
    pub(crate) fn view_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let ctx = ctx.clone();
        let fmt = |s: &KeyboardShortcut| ctx.format_shortcut(s);
        let mut live = self.active_pane().live;
        if ui.add_enabled(self.active_pane().camera.is_some(), egui::Checkbox::new(&mut live, "Live  (L)")).changed() {
            self.toggle_live(live);
        }
        ui.checkbox(&mut self.saved.grid, "Grid  (G)");
        ui.checkbox(&mut self.saved.crosshair, "Crosshair  (C)");
        if ui.button("Display settings...").clicked() {
            self.overlay_open = true;
            ui.close_menu();
        }
        ui.separator();
        if ui.add(egui::Button::new("Fit to window").shortcut_text("F")).clicked() {
            for id in self.targets() {
                self.pane_mut(id).view.fit();
            }
            ui.close_menu();
        }
        if ui.add(egui::Button::new("100%").shortcut_text("1")).clicked() {
            for id in self.targets() {
                self.pane_mut(id).view.actual_size();
            }
            ui.close_menu();
        }
        if ui.add(egui::Button::new("Zoom in").shortcut_text(fmt(&ZOOM_IN))).clicked() {
            self.active_pane_mut().view.zoom_in();
        }
        if ui.add(egui::Button::new("Zoom out").shortcut_text(fmt(&ZOOM_OUT))).clicked() {
            self.active_pane_mut().view.zoom_out();
        }
        ui.separator();
        ui.menu_button("Layout", |ui| {
            for (name, label) in views::LAYOUTS {
                if ui.radio(self.layout == name, label).clicked() {
                    self.set_layout(name, true);
                    ui.close_menu();
                }
            }
        });
        let mut multi = self.layout != "single";
        if ui.checkbox(&mut multi, "Several views  (Ctrl+K)").changed() {
            let target = if multi { self.multi_layout.clone() } else { "single".into() };
            self.set_layout(&target, true);
        }
        if ui.add(egui::Button::new("Add view").shortcut_text(fmt(&ADD_VIEW))).clicked() {
            ui.close_menu();
            self.add_view_and_choose();
        }
        if ui.button("Open images in new views...").clicked() {
            ui.close_menu();
            self.open_in_new_views_dialog(&ctx);
        }
        if ui.add_enabled(self.panes.len() > 1, egui::Button::new("Remove this view")).clicked() {
            ui.close_menu();
            let a = self.active;
            self.remove_view(a, false);
        }
        ui.checkbox(&mut self.link, "Link views");
        ui.menu_button("Source of this view", |ui| {
            let a = self.active;
            self.source_menu(ui, &ctx, a);
        });
        ui.separator();
        ui.menu_button("Panels", |ui| {
            for id in PanelId::ALL {
                let mut open = self.panels.is_open(id);
                if ui.checkbox(&mut open, format!("{}  (Ctrl+{})", id.title(), id.shortcut())).changed() {
                    self.panels.set_open(id, open);
                }
            }
        });
    }

    /// How fiber axes are drawn: the Fiber axis panel's and the overlay settings' choices.
    pub(crate) fn fiber_drawing(&self) -> annotate::FiberDrawing {
        annotate::FiberDrawing {
            show_parts: self.fiber.show_parts,
            long_lines: self.saved.fiber_long_lines,
            show_slow: self.saved.fiber_lines.show_axes != "fast",
            show_fast: self.saved.fiber_lines.show_axes != "slow",
            slow: self.saved.fiber_lines.slow(),
            fast: self.saved.fiber_lines.fast(),
            slow_bands: self.saved.fiber_lines.slow_by_angle.bands(self.saved.fiber_lines.slow_opacity),
            fast_bands: self.saved.fiber_lines.fast_by_angle.bands(self.saved.fiber_lines.fast_opacity),
            // Aligning: numbered from the Alignment panel's Fiber #, else the Fiber Count panel's.
            first_number: if self.panels.is_open(PanelId::Alignment) { self.alignment.next_number } else { self.saved.count_first },
            ascending: if self.panels.is_open(PanelId::Alignment) { self.alignment.ascending } else { self.saved.count_ascending },
            reference: self.alignment.reference_deg(),
            readout_px: self.saved.fiber_lines.readout_size as f32,
            readout_mono: self.saved.fiber_lines.readout_font == "mono",
            readout_bold: self.saved.fiber_lines.readout_bold,
            parts_fill: self.saved.fiber_lines.parts_fill(),
            parts_outline: self.saved.fiber_lines.parts_outline(),
            numbers_only: self.counting_clean(),
            only: None,
        }
    }

    /// The Fiber count panel is open with its axes, angles and region hidden.
    pub(crate) fn counting_clean(&self) -> bool {
        self.panels.is_open(PanelId::FiberCount) && !self.saved.count_show_axes
    }
}

pub struct App {
    pub(crate) panes: Vec<Pane>,
    pub(crate) active: u64,
    pub(crate) controlled: Option<u64>,
    next_pane: u64,
    pub(crate) devices: Vec<(DeviceKey, nokhwa::utils::CameraIndex)>,
    pub(crate) layout: String,
    pub(crate) multi_layout: String,
    pub(crate) link: bool,
    pub(crate) saved: Saved,
    pub(crate) last_saved_label: String,
    pub(crate) record_status: String,
    pub(crate) tl_status: String,
    pub(crate) tl_info: Option<(usize, usize, f64)>,
    pub(crate) overlay_open: bool,
    pub(crate) overlay_tab: String,
    pub(crate) flat: Option<Arc<FlatField>>,
    pub(crate) tool: Option<String>,
    pub(crate) calibration: Calibration,
    /// The next circle drawn on a view sets Smallest rod (Fiber axis panel) instead of becoming a measurement.
    pub(crate) picking_rod: bool,
    pub(crate) fiber: FiberSettings,
    pub(crate) fiber_log: fiber_log::FiberLog,
    pub(crate) histogram: histogram::Histogram,
    pub(crate) alignment: alignment::Alignment,
    fiber_job: Option<(FiberRequest, Receiver<Vec<FiberAxis>>)>,
    fiber_waiting: Vec<FiberRequest>,
    fiber_generation: u64,
    pub(crate) tune_due: Option<Instant>,
    pub(crate) gallery: Gallery,
    pub(crate) panels: PanelLayout,
    pub(crate) player: Option<Player>,
    pub(crate) dialogs: HashMap<String, ProcessDialog>,
    pub(crate) open_dialog: Option<String>,
    job: Option<ProcessJob>,
    pub(crate) modal: Option<Modal>,
    pub(crate) about: bool,
    pub(crate) choose_source: Option<u64>,
    status: (String, Option<Instant>),
    pub(crate) settings_path: Option<PathBuf>,
    pub(crate) camera_values: BTreeMap<String, BTreeMap<String, i64>>,
    pub(crate) capturing: HashSet<u64>,
    pub(crate) slider_hold: Option<(String, i64)>,
    allow_close: bool,
    wake: Arc<dyn Fn() + Send + Sync>,
    logo: Option<egui::TextureHandle>,
    pub(crate) fps_label: String,
    pub(crate) ctx: egui::Context,
}

pub(crate) const OPEN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
pub(crate) const SAVE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
pub(crate) const SAVE_SETTINGS: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::S);
pub(crate) const EXPORT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::E);
pub(crate) const QUIT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Q);
pub(crate) const MULTI: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::K);
pub(crate) const ADD_VIEW: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::K);
pub(crate) const ZOOM_IN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Equals);
pub(crate) const ZOOM_OUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Minus);

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let ctx = cc.egui_ctx.clone();
        ctx.set_visuals(egui::Visuals::light());
        ctx.style_mut(|st| {
            st.spacing.item_spacing = egui::vec2(6.0, 5.0);
            st.spacing.button_padding = egui::vec2(7.0, 3.0);
            st.spacing.interact_size.y = 22.0;
            for w in [&mut st.visuals.widgets.inactive, &mut st.visuals.widgets.hovered, &mut st.visuals.widgets.active] {
                w.corner_radius = egui::CornerRadius::same(4);
            }
            st.visuals.selection.bg_fill = Color32::from_rgb(0xc4, 0xd8, 0xf5);
            st.visuals.selection.stroke = egui::Stroke::new(1.0, Color32::from_rgb(0x1e, 0x4f, 0xa0));
        });
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || ctx.request_repaint());
        let mut saved: Saved = cc
            .storage
            .and_then(|s| s.get_string(SETTINGS_KEY))
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        saved.panels.repair();
        saved.grid_style = GridStyle::from_value(&serde_json::to_value(&saved.grid_style).unwrap());
        saved.crosshair_style = CrosshairStyle::from_value(&serde_json::to_value(&saved.crosshair_style).unwrap());
        saved.fiber_lines = FiberLineStyle::from_value(&serde_json::to_value(&saved.fiber_lines).unwrap());
        let logo = image::load_from_memory(include_bytes!("../../../assets/get_logo.png")).ok().map(|i| {
            let i = i.to_rgba8();
            cc.egui_ctx.load_texture("logo", egui::ColorImage::from_rgba_unmultiplied([i.width() as usize, i.height() as usize], i.as_raw()), Default::default())
        });
        let fiber = FiberSettings {
            live: saved.fiber_live,
            method: saved.fiber_method.clone(),
            show_parts: saved.fiber_show_parts,
            centre_only: saved.fiber_centre_only,
            advanced: saved.fiber_advanced,
            detection: saved.fiber_detection.clone(),
            shown_threshold: 6.0,
        };
        let multi_layout = if views::LAYOUTS.iter().any(|l| l.0 == saved.multi_layout && l.0 != "single") { saved.multi_layout.clone() } else { "grid".into() };
        let overlay_tab = if ["grid", "crosshair", "fiber", "parts", "marks"].contains(&saved.overlay_tab.as_str()) { saved.overlay_tab.clone() } else { "grid".into() };
        let mut gallery = Gallery::new(wake.clone());
        gallery.count = saved.gallery_count.clamp(1, 500);
        std::thread::spawn(crate::processing::fiber_axis::warm_up); // the first Find axes needn't wait
        let mut saved = saved;
        if saved.layout_version < settings::LAYOUT_VERSION {
            saved.bottom_height = saved.bottom_height.max(300.0);
            saved.layout_version = settings::LAYOUT_VERSION;
        }
        let mut app = Self {
            fiber_log: fiber_log::FiberLog { record: saved.fiber_log_record, ..Default::default() },
            histogram: histogram::Histogram::new(saved.histogram_log, saved.histogram_colors),
            alignment: alignment::Alignment::new(saved.align_n, saved.align_region, saved.align_reference, saved.align_next, saved.align_ascending),
            panes: Vec::new(),
            active: 0,
            controlled: None,
            next_pane: 1,
            devices: Vec::new(),
            layout: "single".into(),
            multi_layout,
            link: saved.views_link,
            panels: saved.panels.clone(),
            saved,
            last_saved_label: String::new(),
            record_status: String::new(),
            tl_status: "Images use the Photo tab's averaging, size and format.".into(),
            tl_info: None,
            overlay_open: false,
            overlay_tab,
            flat: None,
            tool: None,
            calibration: Calibration::pixels(),
            picking_rod: false,
            fiber,
            fiber_job: None,
            fiber_waiting: Vec::new(),
            fiber_generation: 0,
            tune_due: None,
            gallery,
            player: None,
            dialogs: HashMap::new(),
            open_dialog: None,
            job: None,
            modal: None,
            about: false,
            choose_source: None,
            status: (String::new(), None),
            settings_path: None,
            camera_values: BTreeMap::new(),
            capturing: HashSet::new(),
            slider_hold: None,
            allow_close: false,
            wake,
            logo,
            fps_label: String::new(),
            ctx: cc.egui_ctx.clone(),
        };
        app.load(&cc.egui_ctx);
        app
    }

    fn load(&mut self, ctx: &egui::Context) {
        // For tests: start with this output folder (not saved over the user's choice unless changed).
        if let Some(f) = std::env::var_os("CAMCONTROL_TEST_FOLDER") {
            self.saved.folder = PathBuf::from(f).display().to_string();
        }
        let folder = PathBuf::from(&self.saved.folder);
        self.gallery.output_folder = Some(folder.clone());
        if !self.saved.gallery_follow && !self.saved.gallery_folder.is_empty() {
            self.gallery.follow_output = false;
            self.gallery.set_folder(&PathBuf::from(&self.saved.gallery_folder));
        } else {
            self.gallery.set_output_folder(&folder);
        }
        // Flat-field correction only with the "extras" feature (the saved choice is kept for it).
        let flat = if cfg!(feature = "extras") { self.saved.flat_path.clone() } else { String::new() };
        if !cfg!(feature = "extras") {
        } else if !flat.is_empty() && PathBuf::from(&flat).is_file() {
            self.use_flat(&PathBuf::from(&flat), true);
            if self.saved.flat_on {
                self.set_flat_live(true);
            }
        } else {
            self.saved.flat_on = false;
        }
        // Command line: camcontrol [--index N] [image or video files...]
        let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
        let index = args.iter().position(|a| a == "--index").and_then(|i| args.get(i + 1)?.to_str()?.parse::<usize>().ok());
        let files: Vec<PathBuf> = args
            .iter()
            .enumerate()
            .filter(|(i, a)| *a != "--index" && !(*i > 0 && args[i - 1] == "--index"))
            .map(|(_, a)| PathBuf::from(a))
            .filter(|p| p.is_file())
            .collect();

        // The views and what each showed; the first run shows the first camera.
        self.refresh_devices();
        let mut specs = self.saved.views.clone();
        if specs.is_empty() {
            specs = vec![self.devices.first().map(|(k, _)| serde_json::json!({"camera": [k.name, k.n]})).unwrap_or(Value::Null)];
        }
        if let Some((k, _)) = index.and_then(|i| self.devices.get(i)) {
            specs[0] = serde_json::json!({"camera": [k.name, k.n]}); // --index: that camera in the first view
        }
        let first = self.new_pane();
        self.active = first;
        self.restore_views(ctx, &specs);
        let layout = self.saved.views_layout.clone();
        self.set_layout(&layout, false);
        // The settings file used last, if it's still there.
        if let Some(p) = self.recent_settings().first().cloned() {
            self.open_settings_path(ctx, &p, true);
        }
        // Files given on the command line (also "Open with" from the file manager):
        // the first in the active view, more each in a new view.
        if let Some(path) = files.first().cloned() {
            let a = self.active;
            self.perform(ctx, if crate::video::is_video(&path) { Action::OpenVideo(a, path) } else { Action::OpenImage(a, path) });
            if files.len() > 1 {
                self.open_paths_in_new_views(&files[1..]);
            }
            if std::env::var_os("CAMCONTROL_FIND_AXES").is_some() {
                self.panels.set_open(PanelId::Fiber, true);
                self.find_fiber_axes(false);
            }
        }
    }

    pub(crate) fn wake(&self) -> Arc<dyn Fn() + Send + Sync> {
        self.wake.clone()
    }

    pub(crate) fn message(&mut self, text: impl Into<String>, seconds: Option<f64>) {
        self.status = (text.into(), seconds.map(|s| Instant::now() + std::time::Duration::from_secs_f64(s)));
    }

    pub(crate) fn warn(&mut self, title: &str, text: impl Into<String>) {
        let text = text.into();
        self.message(text.clone(), Some(8.0));
        self.modal = Some(Modal::Message { title: title.into(), text });
    }

    pub(crate) fn confirm(&mut self, title: &str, text: impl Into<String>, yes: Vec<Action>, no: Vec<Action>) {
        self.modal = Some(Modal::Confirm { title: title.into(), text: text.into(), yes, no });
    }

    // --- the frame ---------------------------------------------------------------------

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let consume = |s: &KeyboardShortcut| ctx.input_mut(|i| i.consume_shortcut(s));
        if consume(&SAVE_SETTINGS) {
            self.save_settings(ctx);
        }
        if consume(&ADD_VIEW) {
            self.add_view_and_choose();
        }
        if consume(&OPEN) {
            let a = self.active;
            self.open_image_dialog(ctx, a);
        }
        if consume(&SAVE) {
            self.save_image();
        }
        #[cfg(feature = "extras")]
        if consume(&EXPORT) {
            self.export_measurements();
        }
        if consume(&QUIT) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if consume(&MULTI) {
            let target = if self.layout == "single" { self.multi_layout.clone() } else { "single".into() };
            self.set_layout(&target, true);
        }
        if consume(&ZOOM_IN) {
            self.active_pane_mut().view.zoom_in();
        }
        if consume(&ZOOM_OUT) {
            self.active_pane_mut().view.zoom_out();
        }
        for id in PanelId::ALL {
            let key = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7][id.shortcut() - 1];
            if consume(&KeyboardShortcut::new(Modifiers::COMMAND, key)) {
                self.panels.toggle(id);
            }
        }
        if ctx.wants_keyboard_input() || self.modal.is_some() {
            return; // typing in a text field
        }
        let pressed = |k: Key| ctx.input(|i| i.key_pressed(k) && i.modifiers.is_none());
        if pressed(Key::Space) {
            self.take_readings();
        }
        if pressed(Key::L) {
            let on = !self.active_pane().live;
            self.toggle_live(on);
        }
        if pressed(Key::G) {
            self.saved.grid = !self.saved.grid;
        }
        if pressed(Key::C) {
            self.saved.crosshair = !self.saved.crosshair;
        }
        if pressed(Key::F) {
            for id in self.targets() {
                self.pane_mut(id).view.fit();
            }
        }
        if pressed(Key::Num1) {
            for id in self.targets() {
                self.pane_mut(id).view.actual_size();
            }
        }
    }

    fn menu(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::menu::bar(ui, |ui| {
            let fmt = |s: &KeyboardShortcut| ctx.format_shortcut(s);
            ui.menu_button("File", |ui| {
                if ui.add(egui::Button::new("Open image...").shortcut_text(fmt(&OPEN))).clicked() {
                    ui.close_menu();
                    let a = self.active;
                    self.open_image_dialog(&ctx, a);
                }
                if ui.button("Open images in new views...").clicked() {
                    ui.close_menu();
                    self.open_in_new_views_dialog(&ctx);
                }
                if ui.add(egui::Button::new("Save image as...").shortcut_text(fmt(&SAVE))).clicked() {
                    ui.close_menu();
                    self.save_image();
                }
                if ui.add_enabled(self.can_capture(), egui::Button::new("Capture").shortcut_text("Space")).clicked() {
                    ui.close_menu();
                    self.capture();
                }
                if ui.button("Open captures folder").clicked() {
                    ui.close_menu();
                    let f = PathBuf::from(&self.saved.folder);
                    let _ = std::fs::create_dir_all(&f);
                    gallery::open_folder(&f);
                }
                ui.separator();
                if ui.button("Open settings...").clicked() {
                    ui.close_menu();
                    self.open_settings_dialog(&ctx);
                }
                ui.menu_button("Open recent settings", |ui| {
                    let recent = self.recent_settings();
                    if recent.is_empty() {
                        ui.add_enabled(false, egui::Button::new("(none yet)"));
                    }
                    for p in &recent {
                        if ui.button(p.file_name().unwrap_or_default().to_string_lossy()).on_hover_text(p.display().to_string()).clicked() {
                            ui.close_menu();
                            self.open_settings_path(&ctx, p, false);
                        }
                    }
                    if !recent.is_empty() {
                        ui.separator();
                        if ui.button("Clear list").clicked() {
                            self.saved.recent_settings.clear();
                            ui.close_menu();
                        }
                    }
                });
                if ui.add(egui::Button::new("Save settings").shortcut_text(fmt(&SAVE_SETTINGS))).clicked() {
                    ui.close_menu();
                    self.save_settings(&ctx);
                }
                if ui.button("Save settings as...").clicked() {
                    ui.close_menu();
                    self.save_settings_as();
                }
                ui.separator();
                if ui.button("Reconnect camera").clicked() {
                    ui.close_menu();
                    self.reconnect_camera(false);
                }
                ui.separator();
                if ui.add(egui::Button::new("Quit").shortcut_text(fmt(&QUIT))).clicked() {
                    ui.close_menu();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("View", |ui| {
                self.view_menu(ui, &ctx);
            });
            #[cfg(feature = "extras")]
            ui.menu_button("Process", |ui| {
                ui.menu_button("Flat-field correction", |ui| {
                    let info = self.flat.as_ref().map(|f| format!("Reference: {}", f.name)).unwrap_or("No flat reference (use or load one below)".into());
                    ui.add_enabled(false, egui::Button::new(info));
                    if ui.button("Use current image as flat reference").clicked() {
                        ui.close_menu();
                        self.set_flat_from_view();
                    }
                    if ui.button("Load flat reference...").clicked() {
                        ui.close_menu();
                        self.load_flat();
                    }
                    ui.separator();
                    let mut on = self.saved.flat_on;
                    if ui.add_enabled(self.flat.is_some(), egui::Checkbox::new(&mut on, "Correct live view and captures")).changed() {
                        self.set_flat_live(on);
                    }
                    if ui.add_enabled(self.flat.is_some(), egui::Button::new("Correct the image in the view")).clicked() {
                        ui.close_menu();
                        self.apply_flat_to_view();
                    }
                });
                ui.separator();
                for key in dialogs::KEYS {
                    if ui.button(format!("{}...", dialogs::title(key))).clicked() {
                        ui.close_menu();
                        self.run_process(key, None);
                    }
                }
            });
            ui.menu_button("Help", |ui| {
                if ui.button("About CamControl").clicked() {
                    self.about = true;
                    ui.close_menu();
                }
            });
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        if let (_, Some(until)) = &self.status {
            if Instant::now() > *until {
                self.status = (String::new(), None);
            } else {
                ui.ctx().request_repaint_after(*until - Instant::now());
            }
        }
        ui.horizontal(|ui| {
            ui.add(egui::Label::new(&self.status.0).truncate());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (text, tip) = self.source_text(self.active);
                ui.label(RichText::new(text).strong()).on_hover_text(tip);
                let pane = self.active_pane();
                if pane.live && !self.fps_label.is_empty() {
                    ui.separator();
                    ui.label(&self.fps_label);
                }
                if let Some(img) = pane.view.image() {
                    ui.separator();
                    ui.label(format!("{} x {}", img.width(), img.height()));
                    ui.separator();
                    ui.label(format!("{:.0}%", pane.view.zoom * 100.0));
                }
                if let Some((line, all)) = pane.details.as_ref().filter(|_| !pane.live) {
                    ui.separator();
                    ui.add(egui::Label::new(RichText::new(line).weak()).truncate()).on_hover_text(all);
                }
                if let Some((x, y, [r, g, b])) = self.panes.iter().find_map(|p| p.view.hover) {
                    ui.separator();
                    ui.label(format!("x {x}, y {y}   RGB {r} {g} {b}"));
                }
            });
        });
    }

    fn panel_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, id: PanelId) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(id.title()).strong().size(15.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("–").on_hover_text("Minimize (its button on the edge brings it back)").clicked() {
                    self.panels.set_open(id, false);
                }
            });
        });
        ui.separator();
        match id {
            PanelId::Controls => {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.controls_panel(ui, ctx));
            }
            PanelId::Captures => {
                let actions = self.gallery.ui(ui);
                self.gallery_actions(ctx, actions);
            }
            #[cfg(feature = "extras")]
            PanelId::Measurements => self.measure_panel(ui),
            #[cfg(not(feature = "extras"))]
            PanelId::Measurements => {}
            PanelId::Alignment => {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.alignment_panel(ui));
            }
            PanelId::Fiber => {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.fiber_panel(ui));
            }
            PanelId::FiberLog => self.fiber_log_panel(ui),
            PanelId::FiberCount => {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.fiber_count_panel(ui));
            }
        }
    }

    fn views_area(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let several = self.panes.len() > 1;
        if several {
            egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(egui::Margin::symmetric(6, 3)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label("Layout:");
                for (name, label) in views::LAYOUTS {
                    if panels::toggle(ui, self.layout == name, label).clicked() {
                        self.set_layout(name, true);
                    }
                }
                if self.layout == "single" {
                    ui.separator();
                    for i in 0..self.panes.len() {
                        let id = self.panes[i].id;
                        let text = format!("{}  {}", i + 1, self.source_text(id).0);
                        if panels::toggle(ui, id == self.active, &text).clicked() {
                            self.set_active(id);
                        }
                    }
                }
                ui.separator();
                ui.checkbox(&mut self.link, "Link views").on_hover_text(
                    "Zoom and pan the views together, and make Capture, Live/Freeze, Fit, 100%\n\
                     and Find fiber axes act on all of them. Off: they act on the active view\n\
                     (the one clicked last, with the highlighted title).");
                if ui.add_enabled(self.panes.len() < views::MAX_VIEWS, egui::Button::new("+ Add view"))
                    .on_hover_text("Add a view; choose its camera or image with its Source menu.").clicked()
                {
                    self.add_view_and_choose();
                }
                if ui.button("Save all as one image...").on_hover_text("Save the images of all the views in one file, with their names.").clicked() {
                    self.save_all_views();
                }
            });
            });
        }
        // The player bar, at the bottom, while a video is open.
        let mut player_actions = Vec::new();
        if let Some(p) = &mut self.player {
            egui::TopBottomPanel::bottom("player").show_inside(ui, |ui| player_actions = p.ui(ui));
        }
        self.player_actions(player_actions);

        let shown: Vec<u64> = if self.layout == "single" || !several {
            vec![self.active]
        } else {
            self.panes.iter().map(|p| p.id).collect()
        };
        let area = ui.available_rect_before_wrap();
        let cols = views::columns_for(&self.layout, shown.len(), area.width(), area.height());
        let rows = shown.len().div_ceil(cols);
        let gap = 4.0;
        let cw = (area.width() - gap * (cols as f32 - 1.0)) / cols as f32;
        let ch = (area.height() - gap * (rows as f32 - 1.0)) / rows as f32;
        let mut events: Vec<(u64, ViewEvent)> = Vec::new();
        let keys_ok = !ctx.wants_keyboard_input() && self.modal.is_none();
        let tool = self.tool.clone();
        let extras = self.view_extras();
        for p in &mut self.panes {
            if p.view.tool != tool {
                p.view.set_tool(tool.clone());
            }
        }
        let mut moved: Option<u64> = None;
        for (k, id) in shown.iter().enumerate() {
            let (r, c) = (k / cols, k % cols);
            let rect = egui::Rect::from_min_size(area.min + egui::vec2(c as f32 * (cw + gap), r as f32 * (ch + gap)), egui::vec2(cw, ch));
            let mut cell = ui.new_child(egui::UiBuilder::new().max_rect(rect).id_salt(("cell", *id)));
            let header = several && self.layout != "single";
            if header {
                self.cell_header(&mut cell, ctx, *id);
            }
            let grid = self.saved.grid.then_some(&self.saved.grid_style);
            let crosshair = self.saved.crosshair.then_some(&self.saved.crosshair_style);
            let active = self.active;
            let (cal, fiber_drawing) = (&self.calibration, self.fiber_drawing());
            // Its header's × may just have removed this view.
            let Some(idx) = self.panes.iter().position(|p| p.id == *id) else { continue };
            let pane = &mut self.panes[idx];
            let overlays = Overlays {
                grid,
                crosshair,
                measurements: &pane.measurements,
                calibration: cal,
                selected: if pane.id == active { pane.measure_selected } else { None },
                fibers: &pane.fibers,
                fiber_selected: pane.fiber_selected,
                fiber_drawing,
                extras: &extras,
            };
            let ev = pane.view.show(&mut cell, &overlays, keys_ok && pane.id == active);
            if pane.view.moved {
                moved = Some(pane.id);
            }
            events.extend(ev.into_iter().map(|e| (*id, e)));
        }
        ui.allocate_rect(area, egui::Sense::hover());
        // Right-click on a view: the View menu.
        let ids: Vec<u64> = self.panes.iter().map(|p| p.id).collect();
        for id in ids {
            if let Some(r) = self.pane(id).view.response.clone() {
                r.context_menu(|ui| self.view_menu(ui, ctx));
            }
        }
        // Link views: the others follow the one that moved.
        if let Some(src) = moved.filter(|_| self.link && self.panes.len() > 1) {
            if let Some(state) = self.pane(src).view.state() {
                for p in self.panes.iter_mut().filter(|p| p.id != src) {
                    p.view.follow(&state);
                }
            }
        }
        for (id, e) in events {
            self.view_event(id, e);
        }
    }

    fn cell_header(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, id: u64) {
        let n = self.number(id);
        let (text, tip) = self.source_text(id);
        let active = id == self.active;
        let fill = if active { ui.visuals().selection.bg_fill } else { ui.visuals().panel_fill };
        let frame = egui::Frame::new().fill(fill).inner_margin(egui::Margin::symmetric(6, 2));
        frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let title = ui.add(egui::Label::new(RichText::new(format!("{n}  {text}")).strong()).truncate().sense(egui::Sense::click()));
                if title.on_hover_text(if tip.is_empty() { "Click a view to work on it (measure, count, fiber axes, Controls).".into() } else { tip }).clicked() {
                    self.set_active(id);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("×").on_hover_text("Remove this view.").clicked() {
                        self.remove_view(id, false);
                    }
                    ui.menu_button("Source...", |ui| self.source_menu(ui, ctx, id))
                        .response
                        .on_hover_text("What this view shows: a camera or a file. Also Live/Freeze and Remove.");
                });
            });
        });
    }

    fn windows(&mut self, ctx: &egui::Context) {
        // A processing tool's dialog.
        if let Some(key) = self.open_dialog.clone() {
            let folder = PathBuf::from(&self.saved.folder);
            let d = self.dialogs.entry(key.clone()).or_insert_with(|| ProcessDialog::new(&key));
            d.start_folder = folder;
            match d.ui(ctx) {
                dialogs::DialogResult::None => {}
                dialogs::DialogResult::Cancel => self.open_dialog = None,
                dialogs::DialogResult::Run(paths, settings) => {
                    self.open_dialog = None;
                    self.start_process(&key, paths, settings);
                }
            }
        }
        if self.overlay_open {
            let s = &mut self.saved;
            self.overlay_open = dialogs::overlay_window(ctx, &mut self.overlay_tab, &mut s.grid_style, &mut s.crosshair_style, &mut s.fiber_long_lines, &mut s.fiber_lines);
        }
        if let Some(id) = self.choose_source {
            let mut open = true;
            egui::Window::new(format!("What view {} shows", self.number(id))).collapsible(false).resizable(false).open(&mut open)
                .show(ctx, |ui| {
                    self.source_menu(ui, ctx, id);
                });
            if !open || self.pane_opt(id).is_none_or(|p| p.camera.is_some() || p.view.image().is_some()) {
                self.choose_source = None;
            }
        }
        if self.about {
            let mut open = true;
            egui::Window::new("About CamControl").collapsible(false).resizable(false).open(&mut open).show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if let Some(logo) = &self.logo {
                        let s = logo.size_vec2();
                        ui.image((logo.id(), s * (160.0 / s.x)));
                    }
                    ui.vertical(|ui| {
                        ui.heading("CamControl");
                        ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                    });
                });
                ui.label("Camera control, measurement and PM fiber alignment for USB microscope cameras.");
                for line in [
                    "Live view, photos, video and time-lapse",
                    "Slow and fast axes of PM fibers, alignment readings and fiber counting",
                    "Measurements, image processing and Excel/CSV export",
                ] {
                    ui.label(format!("•  {line}"));
                }
                ui.add_space(4.0);
                ui.label("Made by Ethan Blake at Grid Evolution Technologies.");
            });
            self.about = open;
        }
        if let Some(job) = &self.job {
            egui::Window::new("CamControl").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!("{}: working on the images...", dialogs::title(&job.tool)));
                    });
                });
        }
        let mut done: Option<Vec<Action>> = None;
        match &self.modal {
            Some(Modal::Message { title, text }) => {
                egui::Window::new(title.as_str()).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                    .show(ctx, |ui| {
                        ui.add(egui::Label::new(text.as_str()).wrap());
                        if ui.button("OK").clicked() || ui.input(|i| i.key_pressed(Key::Enter) || i.key_pressed(Key::Escape)) {
                            done = Some(Vec::new());
                        }
                    });
            }
            Some(Modal::Confirm { title, text, yes, no }) => {
                egui::Window::new(title.as_str()).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                    .show(ctx, |ui| {
                        ui.add(egui::Label::new(text.as_str()).wrap());
                        ui.horizontal(|ui| {
                            if ui.button("Yes").clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                                done = Some(yes.clone());
                            }
                            if ui.button("No").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                                done = Some(no.clone());
                            }
                        });
                    });
            }
            None => {}
        }
        if let Some(actions) = done {
            self.modal = None;
            for a in actions {
                self.perform(ctx, a);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn save_state_for_test(&mut self) {
        self.save_state();
    }

    fn save_state(&mut self) {
        self.saved.panels = self.panels.clone();
        self.saved.views = self.view_specs();
        self.saved.views_layout = self.layout.clone();
        self.saved.views_link = self.link;
        self.saved.gallery_count = self.gallery.count;
        self.saved.gallery_follow = self.gallery.follow_output;
        self.saved.gallery_folder = self.gallery.folder.as_ref().map(|f| f.display().to_string()).unwrap_or_default();
        self.saved.fiber_method = self.fiber.method.clone();
        self.saved.fiber_advanced = self.fiber.advanced;
        self.saved.fiber_centre_only = self.fiber.centre_only;
        self.saved.fiber_show_parts = self.fiber.show_parts;
        self.saved.histogram_log = self.histogram.log;
        self.saved.histogram_colors = self.histogram.colors;
        self.saved.fiber_detection = self.fiber.detection.clone();
        self.saved.fiber_live = self.fiber.live;
        self.saved.fiber_log_record = self.fiber_log.record;
        self.saved.align_n = self.alignment.n;
        self.saved.align_region = self.alignment.region;
        self.saved.align_reference = self.alignment.reference.map(|(a, b)| [a.0, a.1, b.0, b.1]);
        self.saved.align_next = self.alignment.next_number;
        self.saved.align_ascending = self.alignment.ascending;
        self.saved.multi_layout = self.multi_layout.clone();
        self.saved.overlay_tab = self.overlay_tab.clone();
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_cameras(ctx);
        self.poll_jobs(ctx);
        if let Some(p) = &mut self.player {
            if let Some(f) = p.tick(ctx) {
                let pane = p.pane;
                self.show_video_frame(ctx, pane, f);
            }
        }
        if self.tune_due.is_some_and(|t| Instant::now() >= t) {
            self.tune_due = None;
            self.fiber_settings_changed();
        } else if let Some(t) = self.tune_due {
            ctx.request_repaint_after(t - Instant::now());
        }
        self.shortcuts(ctx);

        egui::TopBottomPanel::top("menu").show(ctx, |ui| self.menu(ui));
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| self.status_bar(ui));
        for (edge, id) in [(Edge::Left, "left_stripe"), (Edge::Right, "right_stripe")] {
            if self.panels.stripe_visible(edge) {
                let side = if edge == Edge::Left { egui::panel::Side::Left } else { egui::panel::Side::Right };
                egui::SidePanel::new(side, id).exact_width(STRIPE).resizable(false).frame(stripe_frame(ctx))
                    .show(ctx, |ui| self.panels.stripe(ui, edge));
            }
        }
        for (edge, id, width) in [(Edge::Left, "left_panel", self.saved.left_width), (Edge::Right, "right_panel", self.saved.right_width)] {
            if let Some(panel) = self.panels.open_on(edge) {
                let side = if edge == Edge::Left { egui::panel::Side::Left } else { egui::panel::Side::Right };
                let r = egui::SidePanel::new(side, id).resizable(true).default_width(width).width_range(220.0..=900.0)
                    .show(ctx, |ui| self.panel_ui(ui, ctx, panel));
                let w = r.response.rect.width();
                if edge == Edge::Left { self.saved.left_width = w } else { self.saved.right_width = w }
            }
        }
        if self.panels.stripe_visible(Edge::Bottom) {
            egui::TopBottomPanel::bottom("bottom_stripe").exact_height(STRIPE).resizable(false).frame(stripe_frame(ctx))
                .show(ctx, |ui| self.panels.stripe(ui, Edge::Bottom));
        }
        if let Some(panel) = self.panels.open_on(Edge::Bottom) {
            let r = egui::TopBottomPanel::bottom("bottom_panel").resizable(true).default_height(self.saved.bottom_height).height_range(120.0..=700.0)
                .show(ctx, |ui| self.panel_ui(ui, ctx, panel));
            self.saved.bottom_height = r.response.rect.height();
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(Color32::from_gray(45))).show(ctx, |ui| self.views_area(ui, ctx));
        self.panels.end_frame(ctx);
        self.windows(ctx);

        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close {
            let busy = self.busy_cameras();
            if !busy.is_empty() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.confirm("Quit", format!("{}. Stop and quit?", capitalize(&busy.join("; "))), vec![Action::Quit], vec![]);
            }
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.save_state();
        if let Ok(text) = serde_json::to_string(&self.saved) {
            storage.set_string(SETTINGS_KEY, text);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Cameras close (and recordings finish) as their links drop.
        for p in &mut self.panes {
            if let Some(c) = &mut p.camera {
                c.stop();
            }
        }
    }

    fn auto_save_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(30)
    }
}

/// Stripes have no margins: their buttons use the full width.
fn stripe_frame(ctx: &egui::Context) -> egui::Frame {
    egui::Frame::side_top_panel(&ctx.style()).inner_margin(0).fill(Color32::from_gray(232))
}

pub(crate) fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}
