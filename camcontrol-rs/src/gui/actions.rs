//! What the main window does (the behaviour half of main_window.py).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Instant;

use eframe::egui;
use image::RgbImage;
use serde_json::{json, Map, Value};

use super::dialogs::{self, ProcessDialog};
use super::gallery::{self, GalleryAction};
use super::player::{Player, PlayerAction};
use super::settings::{self, AVERAGE_CHOICES, FORMATS};
use super::stripes::PanelId;
use super::style::{CrosshairStyle, FiberLineStyle, GridStyle};
use super::view::ViewEvent;
use super::views::{self, LAYOUTS, MAX_VIEWS};
use super::{Action, App, FiberKind, FiberRequest, Pane, ProcessJob, ResultInfo, FIBER_LIVE_INTERVAL};
use crate::camera::worker::{CameraLink, CaptureRequest, Cmd, Ev, RecordOverlay, TimeLapse};
use crate::camera::{camera_list, DeviceKey};
use crate::capture::{self, clean_name, next_capture_path, HD2_SCALE};
use crate::export;
use crate::measure::{compute, kind, Measurement};
use crate::processing::fiber_axis::{self, FiberAxis};
use crate::processing::flatfield::FlatField;
use crate::video::is_video;

pub const SAVE_SCALES: [f64; 2] = [1.0, HD2_SCALE];
const IMAGE_FILTER: [&str; 6] = ["tif", "tiff", "png", "jpg", "jpeg", "bmp"];

pub(crate) fn file_name(p: &Path) -> String {
    p.file_name().unwrap_or_default().to_string_lossy().to_string()
}

pub(crate) fn timestamp() -> String {
    chrono::Local::now().format("%Y%m%d_%H%M%S").to_string()
}

/// A save dialog's path, with a known extension (else `default_ext`).
pub(crate) fn with_ext(path: PathBuf, allowed: &[&str], default_ext: &str) -> PathBuf {
    let ok = path.extension().is_some_and(|e| allowed.contains(&e.to_string_lossy().to_lowercase().as_str()));
    if ok { path } else { path.with_extension(default_ext) }
}

impl App {
    // --- views ---------------------------------------------------------------------------

    pub(crate) fn pane(&self, id: u64) -> &Pane {
        self.panes.iter().find(|p| p.id == id).unwrap_or(&self.panes[0])
    }

    pub(crate) fn pane_opt(&self, id: u64) -> Option<&Pane> {
        self.panes.iter().find(|p| p.id == id)
    }

    pub(crate) fn pane_mut(&mut self, id: u64) -> &mut Pane {
        let i = self.panes.iter().position(|p| p.id == id).unwrap_or(0);
        &mut self.panes[i]
    }

    pub(crate) fn active_pane(&self) -> &Pane {
        self.pane(self.active)
    }

    pub(crate) fn active_pane_mut(&mut self) -> &mut Pane {
        let a = self.active;
        self.pane_mut(a)
    }

    pub(crate) fn number(&self, id: u64) -> usize {
        self.panes.iter().position(|p| p.id == id).map(|i| i + 1).unwrap_or(1)
    }

    /// The views an action works on: all of them with Link views on, else the active one.
    pub(crate) fn targets(&self) -> Vec<u64> {
        if self.link && self.panes.len() > 1 { self.panes.iter().map(|p| p.id).collect() } else { vec![self.active] }
    }

    pub(crate) fn new_pane(&mut self) -> u64 {
        let id = self.next_pane;
        self.next_pane += 1;
        self.panes.push(Pane::new(id));
        id
    }

    pub(crate) fn set_active(&mut self, id: u64) {
        if self.pane_opt(id).is_none() || id == self.active {
            return;
        }
        let several = self.panes.len() > 1;
        self.active = id;
        self.choose_controlled();
        let path = self.active_pane().path.clone();
        self.gallery.set_open_path(path.as_deref());
        if several {
            let t = self.source_text(id).0;
            self.message(format!("Working on view {}: {t}", self.number(id)), Some(4.0));
        }
    }

    pub(crate) fn set_layout(&mut self, name: &str, add: bool) {
        if !LAYOUTS.iter().any(|l| l.0 == name) {
            return;
        }
        if name != "single" {
            self.multi_layout = name.into();
        }
        if name != "single" && self.panes.len() < 2 && add {
            self.layout = name.into();
            self.add_view_and_choose();
            return;
        }
        self.layout = name.into();
    }

    pub(crate) fn add_view(&mut self) -> Option<u64> {
        if self.panes.len() >= MAX_VIEWS {
            self.message("That's as many views as fit. Remove one first.", Some(6.0));
            return None;
        }
        let id = self.new_pane();
        if self.layout == "single" {
            self.layout = self.multi_layout.clone();
        }
        self.set_active(id);
        self.message(format!("View {} added: choose a camera or an image in its Source menu (or Controls > Device).", self.number(id)), Some(8.0));
        Some(id)
    }

    pub(crate) fn add_view_and_choose(&mut self) {
        if let Some(id) = self.add_view() {
            self.choose_source = Some(id);
        }
    }

    pub(crate) fn open_in_new_views_dialog(&mut self, _ctx: &egui::Context) {
        let d = rfd::FileDialog::new().set_title("Open images in new views").add_filter("Images", &IMAGE_FILTER).set_directory(&self.saved.folder);
        if let Some(paths) = d.pick_files() {
            self.open_paths_in_new_views(&paths);
        }
    }

    pub(crate) fn open_paths_in_new_views(&mut self, paths: &[PathBuf]) {
        let (mut failed, mut skipped, mut added) = (Vec::new(), 0, 0);
        for p in paths {
            if self.panes.len() >= MAX_VIEWS {
                skipped += 1;
                continue;
            }
            let Some(img) = capture::load_image(p) else {
                failed.push(file_name(p));
                continue;
            };
            let id = self.new_pane();
            self.show_image(id, Arc::new(img), Some(p.clone()), String::new(), None);
            added += 1;
        }
        if added > 0 && self.layout == "single" {
            self.layout = self.multi_layout.clone();
        }
        if !failed.is_empty() {
            self.warn("Open in new views", format!("Could not read {}", failed.join(", ")));
        }
        if skipped > 0 {
            self.message(format!("No room for {skipped} more view{}. Remove some first.", if skipped != 1 { "s" } else { "" }), Some(8.0));
        }
    }

    pub(crate) fn remove_view(&mut self, id: u64, force: bool) {
        if self.pane_opt(id).is_none() {
            return;
        }
        if self.panes.len() == 1 {
            self.message("There's always at least one view.", Some(5.0));
            return;
        }
        if !force {
            if let Some(busy) = self.pane(id).camera.as_ref().map(busy_list).filter(|b| !b.is_empty()) {
                let label = self.pane(id).camera.as_ref().unwrap().key.label();
                self.confirm("Remove view", format!("On {label}, {}. Stop it?", busy.join(" and ")), vec![Action::RemoveView(id)], vec![]);
                return;
            }
        }
        if id == self.active {
            let other = self.panes.iter().find(|p| p.id != id).unwrap().id;
            self.set_active(other);
        }
        if self.player.as_ref().is_some_and(|p| p.pane == id) {
            self.player = None;
        }
        self.release_camera(id);
        self.fiber_waiting.retain(|r| r.pane != id);
        self.panes.retain(|p| p.id != id);
        self.choose_controlled();
        if self.panes.len() == 1 {
            self.layout = "single".into();
        }
    }

    pub(crate) fn save_all_views(&mut self) {
        let shown: Vec<(Arc<RgbImage>, String)> = self
            .panes
            .iter()
            .filter_map(|p| p.image().map(|i| (i, format!("{} {}", self.number(p.id), self.source_text(p.id).0))))
            .collect();
        if shown.len() < 2 {
            self.warn("Save all views", "Two or more views need an image first.");
            return;
        }
        let default = next_capture_path(Path::new(&self.saved.folder), "views", "png", "views");
        let Some(path) = rfd::FileDialog::new()
            .set_title("Save all views as one image")
            .add_filter("PNG", &["png"])
            .add_filter("TIFF", &["tif"])
            .add_filter("JPEG", &["jpg"])
            .set_directory(&self.saved.folder)
            .set_file_name(file_name(&default))
            .save_file()
        else {
            return;
        };
        let path = with_ext(path, &["tif", "tiff", "png", "jpg", "jpeg"], "png");
        let images: Vec<&RgbImage> = shown.iter().map(|s| s.0.as_ref()).collect();
        let names: Vec<String> = shown.iter().map(|s| s.1.clone()).collect();
        match capture::save_image(&path, &views::grid_image(&images, &names)) {
            Ok(()) => self.message(format!("Saved {}", path.display()), Some(5.0)),
            Err(e) => self.warn("Save all views", format!("Save failed: {e}")),
        }
    }

    pub(crate) fn view_specs(&self) -> Vec<Value> {
        self.panes
            .iter()
            .map(|p| {
                if let Some(c) = &p.camera {
                    json!({"camera": [c.key.name, c.key.n]})
                } else if let (Some(path), None) = (&p.path, &p.result) {
                    if is_video(path) { json!({}) } else { json!({"path": std::fs::canonicalize(path).unwrap_or(path.clone()).display().to_string().trim_start_matches(r"\\?\")}) }
                } else {
                    json!({})
                }
            })
            .collect()
    }

    /// Make the views show what specs say. Views beyond them are removed if nothing is lost by it.
    pub(crate) fn restore_views(&mut self, _ctx: &egui::Context, specs: &[Value]) {
        for (i, spec) in specs.iter().take(MAX_VIEWS).enumerate() {
            let id = if i < self.panes.len() { self.panes[i].id } else { self.new_pane() };
            if let Some(c) = spec["camera"].as_array().filter(|c| c.len() == 2) {
                if let (Some(name), Some(n)) = (c[0].as_str(), c[1].as_u64()) {
                    if !name.is_empty() {
                        let key = DeviceKey { name: name.into(), n: n as usize };
                        if self.pane(id).camera.as_ref().map(|c| &c.key) != Some(&key) {
                            self.set_pane_camera(id, Some(key), true);
                        }
                    }
                }
            } else if let Some(path) = spec["path"].as_str().map(PathBuf::from) {
                if path.is_file() && self.pane(id).path.as_ref() != Some(&path) {
                    if let Some(img) = capture::load_image(&path) {
                        self.show_image(id, Arc::new(img), Some(path), String::new(), None);
                    }
                }
            }
        }
        let extra: Vec<u64> = self.panes.iter().skip(specs.len().max(1)).map(|p| p.id).collect();
        for id in extra {
            let p = self.pane(id);
            let keep = p.camera.as_ref().is_some_and(|c| !busy_list(c).is_empty())
                || !p.measurements.is_empty() || !p.counter.marks.is_empty() || !p.fibers.is_empty();
            if !keep {
                self.remove_view(id, true);
            }
        }
    }

    // --- sources ---------------------------------------------------------------------------

    pub(crate) fn refresh_devices(&mut self) {
        self.devices = camera_list();
    }

    fn device_index(&self, key: &DeviceKey) -> Option<nokhwa::utils::CameraIndex> {
        self.devices.iter().find(|(k, _)| k == key).map(|(_, i)| i.clone())
    }

    fn camera_owner(&self, key: &DeviceKey) -> Option<u64> {
        self.panes.iter().find(|p| p.camera.as_ref().is_some_and(|c| &c.key == key)).map(|p| p.id)
    }

    /// A view's Source menu: its camera, No camera, Live, Open image, Remove.
    pub(crate) fn source_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, id: u64) {
        if ui.memory(|m| m.data.get_temp::<bool>(egui::Id::new("devices_fresh")).is_none()) {
            self.refresh_devices();
            ui.memory_mut(|m| m.data.insert_temp(egui::Id::new("devices_fresh"), true));
        }
        let current = self.pane(id).camera.as_ref().map(|c| c.key.clone());
        ui.label(egui::RichText::new("Camera").weak());
        let mut keys: Vec<DeviceKey> = self.devices.iter().map(|d| d.0.clone()).collect();
        if let Some(c) = &current {
            if !keys.contains(c) {
                keys.push(c.clone());
            }
        }
        for key in keys {
            let mut text = key.label();
            if let Some(owner) = self.camera_owner(&key).filter(|o| *o != id) {
                text += &format!("  (in view {}: move it here)", self.number(owner));
            }
            if !self.devices.iter().any(|d| d.0 == key) {
                text += "  (not connected)";
            }
            if ui.radio(current.as_ref() == Some(&key), text).clicked() {
                ui.close_menu();
                self.set_pane_camera(id, Some(key), false);
            }
        }
        if self.devices.is_empty() {
            ui.add_enabled(false, egui::Button::new("No cameras found"));
        }
        if ui.radio(current.is_none(), "No camera").clicked() {
            ui.close_menu();
            self.set_pane_camera(id, None, false);
        }
        ui.separator();
        let mut live = self.pane(id).live;
        if ui.add_enabled(current.is_some(), egui::Checkbox::new(&mut live, "Live")).changed() {
            self.set_live(id, live);
        }
        if ui.button("Open image...").clicked() {
            ui.close_menu();
            self.open_image_dialog(ctx, id);
        }
        ui.separator();
        if ui.add_enabled(self.panes.len() > 1, egui::Button::new("Remove this view")).clicked() {
            ui.close_menu();
            self.remove_view(id, false);
        }
    }

    /// Show camera key in the view, live (None: no camera). A camera used by
    /// another view moves to this one.
    pub(crate) fn set_pane_camera(&mut self, id: u64, key: Option<DeviceKey>, force: bool) {
        let current = self.pane(id).camera.as_ref().map(|c| c.key.clone());
        if key == current {
            if current.is_some() {
                self.set_live(id, true);
            }
            return;
        }
        let owner = key.as_ref().and_then(|k| self.camera_owner(k));
        if !force {
            for pid in [Some(id), owner].into_iter().flatten() {
                if let Some(c) = &self.pane(pid).camera {
                    let busy = busy_list(c);
                    if !busy.is_empty() {
                        let label = c.key.label();
                        self.confirm("Change camera", format!("On {label}, {}. Stop it?", busy.join(" and ")),
                                     vec![Action::SetPaneCamera(id, key)], vec![]);
                        return;
                    }
                }
            }
        }
        // Moved from another view: the open camera comes with it.
        let moved = owner.and_then(|o| {
            let p = self.pane_mut(o);
            p.live = false;
            p.camera.take()
        });
        self.release_camera(id);
        if let Some(key) = key {
            match moved {
                Some(link) => self.pane_mut(id).camera = Some(link),
                None => {
                    self.pane_mut(id).camera = Some(CameraLink::new(key));
                    self.start_link(id);
                }
            }
            self.set_live(id, true);
        }
        self.choose_controlled();
    }

    pub(crate) fn start_link(&mut self, id: u64) {
        if self.devices.is_empty() {
            self.refresh_devices();
        }
        let Some(key) = self.pane(id).camera.as_ref().map(|c| c.key.clone()) else { return };
        let index = self.device_index(&key);
        let size = self.saved.resolutions.get(&key.label()).and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
        });
        let flat = if self.saved.flat_on { self.flat.clone() } else { None };
        let wake = self.wake();
        if let Some(link) = self.pane_mut(id).camera.as_mut() {
            link.start(index, size, flat, wake);
        }
    }

    fn release_camera(&mut self, id: u64) {
        let p = self.pane_mut(id);
        p.live = false;
        if let Some(mut link) = p.camera.take() {
            link.stop();
        }
        self.capturing.remove(&id);
        if self.controlled == Some(id) {
            self.controlled = None;
        }
    }

    /// Controls work on the active view's camera; if it has none, on the camera
    /// they had; else on the first view with one.
    pub(crate) fn choose_controlled(&mut self) {
        let new = if self.active_pane().camera.is_some() {
            Some(self.active)
        } else if self.controlled.is_some_and(|c| self.pane_opt(c).is_some_and(|p| p.camera.is_some())) {
            self.controlled
        } else {
            self.panes.iter().find(|p| p.camera.is_some()).map(|p| p.id)
        };
        if new != self.controlled {
            self.controlled = new;
            self.record_status.clear();
        }
    }

    pub(crate) fn link(&self) -> Option<&CameraLink> {
        self.controlled.and_then(|c| self.pane_opt(c)).and_then(|p| p.camera.as_ref())
    }

    pub(crate) fn link_mut(&mut self) -> Option<&mut CameraLink> {
        let c = self.controlled?;
        self.panes.iter_mut().find(|p| p.id == c).and_then(|p| p.camera.as_mut())
    }

    pub(crate) fn reconnect_camera(&mut self, force: bool) -> bool {
        let Some(c) = self.controlled else {
            self.message("This view has no camera: choose one in Device.", Some(5.0));
            return false;
        };
        if !force {
            if let Some(link) = self.link() {
                let busy = busy_list(link);
                if !busy.is_empty() {
                    let label = link.key.label();
                    self.confirm("Reconnect camera", format!("On {label}, {}. Stop it?", busy.join(" and ")), vec![Action::Reconnect], vec![]);
                    return false;
                }
            }
        }
        self.message("Reconnecting to the camera...", None);
        self.capturing.remove(&c);
        self.refresh_devices();
        self.start_link(c);
        true
    }

    pub(crate) fn busy_cameras(&self) -> Vec<String> {
        self.panes
            .iter()
            .filter_map(|p| p.camera.as_ref())
            .filter_map(|c| {
                let b = busy_list(c);
                (!b.is_empty()).then(|| format!("on {}, {}", c.key.label(), b.join(" and ")))
            })
            .collect()
    }

    // --- camera events --------------------------------------------------------------------

    pub(crate) fn poll_cameras(&mut self, ctx: &egui::Context) {
        for i in 0..self.panes.len() {
            let id = self.panes[i].id;
            let Some(link) = self.panes[i].camera.as_mut() else { continue };
            let events = link.poll();
            let frame = link.take_frame();
            let fps = link.fps();
            for ev in events {
                self.camera_event(ctx, id, ev);
            }
            self.sync_record_overlay(i);
            if let Some(hold) = self.slider_hold.clone() {
                if self.controlled == Some(id) {
                    if let Some(link) = self.pane_mut(id).camera.as_mut() {
                        link.values.insert(hold.0.clone(), hold.1);
                    }
                }
            }
            let pane = &mut self.panes[i];
            if let (true, Some((f, tiles))) = (pane.live, frame) {
                pane.view.set_image_tiles(ctx, f.clone(), tiles);
                if id == self.active {
                    self.fps_label = fps.map(|f| format!("{f:.1} fps")).unwrap_or_default();
                }
                // Fiber axes of the live views, a few times a second, taking turns.
                if (self.fiber.live || self.collecting_on(id)) && self.fiber_job.is_none() {
                    let last = self.panes[i].fiber_last;
                    let due = last.is_none_or(|t| t.elapsed().as_secs_f64() >= FIBER_LIVE_INTERVAL);
                    let oldest = self.panes.iter().filter(|p| p.live).all(|p| match (p.fiber_last, last) {
                        (_, None) => true,
                        (None, Some(_)) => false,
                        (Some(a), Some(b)) => b <= a,
                    });
                    if due && oldest {
                        self.panes[i].fiber_last = Some(Instant::now());
                        self.request_fibers(id, f, FiberKind::Live);
                    }
                }
            }
        }
    }

    fn camera_event(&mut self, _ctx: &egui::Context, id: u64, ev: Ev) {
        let controlled = self.controlled == Some(id);
        match ev {
            Ev::Opened(_) => {
                if controlled {
                    self.message("Camera open", Some(3.0));
                }
                let label = self.pane(id).camera.as_ref().map(|c| c.key.label()).unwrap_or_default();
                if let Some(values) = self.camera_values.remove(&label) {
                    self.apply_camera_values(id, &values);
                }
                if self.pane(id).live {
                    // A camera's first frames replace whatever the view showed.
                }
            }
            Ev::OpenFailed(m) => {
                if controlled {
                    self.message(format!("{m} Turn the camera on, then click Reconnect."), None);
                }
            }
            Ev::Settings(_) => {}
            Ev::CaptureStarted(n, s) => self.message(format!("Capturing {n} frame(s), about {s:.1} s..."), None),
            Ev::CaptureDone(path) => {
                self.capturing.remove(&id);
                self.last_saved_label = format!("Saved {}", file_name(&path));
                self.message(format!("Saved {}", path.display()), Some(5.0));
                self.gallery.refresh(false, Some(&path));
            }
            Ev::Error(m) => {
                self.capturing.remove(&id);
                self.warn("CamControl", m);
            }
            Ev::RecordingChanged(on, path) => {
                if controlled {
                    self.record_status = match (&path, on) {
                        (Some(p), true) => format!("Recording to {}", file_name(p)),
                        (Some(p), false) => format!("Saved {}", file_name(p)),
                        _ => String::new(),
                    };
                }
                match (on, path) {
                    (true, Some(p)) => self.message(format!("Recording to {}...", file_name(&p)), None),
                    (false, Some(p)) => self.message(format!("Saved video {}", p.display()), Some(8.0)),
                    _ => {}
                }
            }
            Ev::RecordingProgress(s, f) => {
                if controlled && self.pane(id).camera.as_ref().is_some_and(|c| c.recording) {
                    self.record_status = format!("Recording {} ({f} frames)", super::player::format_duration(s));
                }
            }
            Ev::TimeLapseProgress(taken, total, due, path) => {
                if controlled {
                    self.tl_info = Some((taken, total, due));
                }
                self.last_saved_label = format!("Saved {} (time-lapse)", file_name(&path));
            }
            Ev::TimeLapseFinished(m) => {
                if controlled {
                    self.tl_info = None;
                    self.tl_status = m.clone();
                }
                self.message(m, Some(10.0));
            }
        }
    }

    pub(crate) fn apply_camera_values(&mut self, id: u64, values: &BTreeMap<String, i64>) {
        if let Some(link) = self.pane_mut(id).camera.as_mut() {
            for (name, v) in values {
                if link.ranges.get(name).copied().flatten().is_some() {
                    link.values.insert(name.clone(), *v);
                    link.send(Cmd::Control(name.clone(), *v as f64));
                }
            }
        }
    }

    // --- capture, live ----------------------------------------------------------------------

    fn capture_panes(&self) -> Vec<u64> {
        if self.link && self.panes.len() > 1 {
            self.panes.iter().filter(|p| p.camera.as_ref().is_some_and(|c| c.open)).map(|p| p.id).collect()
        } else {
            self.controlled.filter(|c| self.pane_opt(*c).and_then(|p| p.camera.as_ref()).is_some_and(|l| l.open)).into_iter().collect()
        }
    }

    pub(crate) fn can_capture(&self) -> bool {
        !self.capture_panes().is_empty() && self.capturing.is_empty()
    }

    pub(crate) fn capture_request(&self, name: &str) -> CaptureRequest {
        CaptureRequest {
            n_frames: AVERAGE_CHOICES[self.saved.average_idx.min(AVERAGE_CHOICES.len() - 1)],
            scale: SAVE_SCALES[self.saved.size_idx.min(1)],
            fmt: FORMATS[self.saved.format_idx.min(1)].1.into(),
            folder: PathBuf::from(&self.saved.folder),
            name: name.into(),
            markup: self.saved.recording.photo_markup,
        }
    }

    pub(crate) fn capture(&mut self) {
        let panes = self.capture_panes();
        if panes.is_empty() || !self.capturing.is_empty() {
            return;
        }
        let name = clean_name(&self.saved.name);
        for id in &panes {
            let this = if panes.len() > 1 {
                // Each camera its own series, so two can't take the same number.
                format!("{}_view{}", if name.is_empty() { "cap" } else { &name }, self.number(*id))
            } else {
                name.clone()
            };
            let req = self.capture_request(&this);
            if let Some(link) = &self.pane(*id).camera {
                link.send(Cmd::Capture(req));
            }
            self.capturing.insert(*id);
        }
    }

    pub(crate) fn toggle_live(&mut self, on: bool) {
        for id in self.targets() {
            if self.pane(id).camera.is_some() {
                self.set_live(id, on);
            }
        }
    }

    pub(crate) fn set_live(&mut self, id: u64, on: bool) {
        let ctx = self.ctx_clone();
        let p = self.pane_mut(id);
        let on = on && p.camera.is_some();
        p.live = on;
        if on {
            p.path = None;
            p.title.clear();
            p.result = None;
            p.fiber_last = None;
            p.fibers.clear(); // the last image's fibers: not to be followed into the camera's frames
            p.fiber_locked = false;
            if let Some(f) = p.camera.as_ref().and_then(|c| c.last_frame()) {
                p.view.set_image(&ctx, f);
            }
            if self.player.as_ref().is_some_and(|pl| pl.pane == id) {
                self.player = None;
            }
        }
        if id == self.active {
            self.gallery.set_open_path(None);
        }
    }

    fn ctx_clone(&self) -> egui::Context {
        self.ctx.clone()
    }

    /// What a view shows, for its title and the status bar: (text, tooltip).
    pub(crate) fn source_text(&self, id: u64) -> (String, String) {
        let p = self.pane(id);
        let cam = p.camera.as_ref().map(|c| c.key.label());
        if p.live {
            let c = p.camera.as_ref().unwrap();
            if c.open {
                return (format!("{} · Live", cam.unwrap()), c.name.clone());
            }
            return (format!("{} · {}", cam.unwrap(), c.status.0), c.status.2.clone());
        }
        if p.result.is_some() {
            return (format!("{} (unsaved)", p.title), "Processing result. File > Save image as (Ctrl+S) to keep it.".into());
        }
        if let Some(path) = &p.path {
            return (file_name(path), path.display().to_string());
        }
        if p.view.image().is_some() {
            return (format!("Frozen{}", cam.as_ref().map(|c| format!(" ({c})")).unwrap_or_default()),
                    if cam.is_some() { "L goes back to live.".into() } else { String::new() });
        }
        ("Empty: choose a camera or an image in Source".into(), String::new())
    }

    // --- opening and saving images -------------------------------------------------------

    pub(crate) fn open_image_dialog(&mut self, ctx: &egui::Context, id: u64) {
        let d = rfd::FileDialog::new()
            .set_title("Open image or video")
            .add_filter("Images and videos", &["tif", "tiff", "png", "jpg", "jpeg", "bmp", "mp4", "avi"])
            .add_filter("All files", &["*"])
            .set_directory(&self.saved.folder);
        if let Some(path) = d.pick_file() {
            self.open_image_path(ctx, id, path);
        }
    }

    /// Show an image file in a view (its camera keeps running for when it goes live again). Videos play.
    pub(crate) fn open_image_path(&mut self, ctx: &egui::Context, id: u64, path: PathBuf) {
        let then = if is_video(&path) { Action::OpenVideo(id, path) } else { Action::OpenImage(id, path) };
        self.ask_clear_annotations(ctx, id, "Open image", then);
    }

    /// Measurements and counts belong to the image they were made on.
    fn ask_clear_annotations(&mut self, ctx: &egui::Context, id: u64, title: &str, then: Action) {
        self.set_active(id);
        let p = self.pane(id);
        if p.measurements.is_empty() && p.counter.marks.is_empty() {
            self.perform(ctx, then);
            return;
        }
        self.confirm(title, "Clear this view's measurements and counts? They were made on a different image.",
                     vec![Action::ClearAnnotations(id), then.clone()], vec![then]);
    }

    /// Show an image in a view, frozen: a file (path) or a processing result.
    pub(crate) fn show_image(&mut self, id: u64, image: Arc<RgbImage>, path: Option<PathBuf>, title: String, result: Option<ResultInfo>) {
        self.set_live(id, false);
        let ctx = self.ctx_clone();
        let p = self.pane_mut(id);
        p.view.set_image(&ctx, image);
        p.title = if title.is_empty() { path.as_deref().map(file_name).unwrap_or_default() } else { title };
        // The details saved with the file, for the status bar.
        p.details = path.as_deref().and_then(capture::read_image_details).map(|m| {
            (capture::details_summary(&m), serde_json::to_string_pretty(&m).unwrap_or_default())
        }).filter(|d| !d.0.is_empty());
        p.path = path;
        p.result = result;
        if id == self.active {
            let path = self.pane(id).path.clone();
            self.gallery.set_open_path(path.as_deref());
        }
        self.measure_opened_image(id);
    }

    pub(crate) fn show_video_frame(&mut self, ctx: &egui::Context, id: u64, frame: Arc<RgbImage>) {
        if self.pane_opt(id).is_none() {
            return;
        }
        self.pane_mut(id).view.set_image(ctx, frame.clone());
        // Update live: while playing, as often as the measuring keeps up (following the fibers,
        // frames that come while it's busy are skipped); a paused or stepped-to frame, always.
        if self.fiber.live {
            let playing = self.player.as_ref().is_some_and(|p| p.pane == id && p.playing);
            self.request_fibers(id, frame, if playing { FiberKind::Live } else { FiberKind::Image });
        }
    }

    /// Whether this view is showing a video's frames.
    fn plays_video(&self, id: u64) -> bool {
        self.player.as_ref().is_some_and(|p| p.pane == id)
    }

    pub(crate) fn player_actions(&mut self, actions: Vec<PlayerAction>) {
        let ctx = self.ctx_clone();
        for a in actions {
            match a {
                PlayerAction::Frame(f) => {
                    if let Some(pane) = self.player.as_ref().map(|p| p.pane) {
                        self.show_video_frame(&ctx, pane, f);
                    }
                }
                PlayerAction::Close => {
                    // Back to the camera, if the view has one.
                    if let Some(p) = self.player.take() {
                        if self.pane_opt(p.pane).is_some_and(|q| q.camera.is_some()) {
                            self.set_live(p.pane, true);
                        }
                    }
                }
            }
        }
    }

    pub(crate) fn save_image(&mut self) {
        let a = self.active;
        let Some(image) = self.pane(a).image() else { return };
        let stem = self.pane(a).result.as_ref().map(|r| r.short_name.clone()).unwrap_or("image".into());
        let default = next_capture_path(Path::new(&self.saved.folder), &stem, "tif", "image");
        let Some(path) = rfd::FileDialog::new()
            .set_title("Save image")
            .add_filter("TIFF", &["tif"])
            .add_filter("PNG", &["png"])
            .add_filter("JPEG", &["jpg"])
            .set_directory(&self.saved.folder)
            .set_file_name(file_name(&default))
            .save_file()
        else {
            return;
        };
        let path = with_ext(path, &["tif", "tiff", "png", "jpg", "jpeg"], "tif");
        let mut meta = json!({"timestamp": export::now_iso(), "saved_size": [image.width(), image.height()]});
        if let Some(r) = &self.pane(a).result {
            if let (Some(m), Some(o)) = (meta.as_object_mut(), r.meta.as_object()) {
                for (k, v) in o {
                    m.insert(k.clone(), v.clone());
                }
            }
        } else {
            meta["source"] = json!(self.active_source());
            if let Some(p) = self.player.as_mut().filter(|p| p.pane == a) {
                p.pause();
                meta["video_frame"] = json!(p.position + 1);
            }
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = capture::save_image_with(&path, &image, Some(&meta)) {
            self.warn("Save image", format!("Save failed: {e}"));
            return;
        }
        self.message(format!("Saved {}", path.display()), Some(5.0));
        if self.pane(a).result.is_some() {
            let p = self.pane_mut(a);
            p.result = None;
            p.title = file_name(&path);
            p.path = Some(path.clone());
        }
        self.gallery.refresh(false, Some(&path));
    }

    /// Name of what the active view shows, for exports.
    pub(crate) fn active_source(&self) -> String {
        let p = self.active_pane();
        p.path.as_deref().map(file_name).unwrap_or_else(|| self.source_text(self.active).0)
    }

    pub(crate) fn perform(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::ClearAnnotations(id) => {
                let p = self.pane_mut(id);
                p.measurements.clear();
                p.next_id = 1;
                p.measure_selected = None;
                p.counter.marks.clear();
            }
            Action::OpenImage(id, path) => match capture::load_image(&path) {
                None => self.warn("CamControl", format!("Could not read {}", path.display())),
                Some(img) => {
                    if self.player.as_ref().is_some_and(|p| p.pane == id) {
                        self.player = None;
                    }
                    self.show_image(id, Arc::new(img), Some(path), String::new(), None);
                }
            },
            Action::OpenVideo(id, path) => {
                self.set_live(id, false);
                self.player = None; // one video plays at a time
                match Player::open(&path, id) {
                    Err(e) => self.warn("CamControl", format!("Could not play {}: {e}", path.display())),
                    Ok((mut player, first)) => {
                        self.pane_mut(id).view.set_image(ctx, first.clone());
                        let p = self.pane_mut(id);
                        p.title = file_name(&path);
                        p.path = Some(path.clone());
                        p.result = None;
                        p.fibers.clear();
                        player.play();
                        self.player = Some(player);
                        if self.fiber.live {
                            self.request_fibers(id, first, FiberKind::Live);
                        }
                        if id == self.active {
                            self.gallery.set_open_path(Some(&path));
                        }
                    }
                }
            }
            Action::ShowResult { pane, image, title, info } => {
                if self.player.as_ref().is_some_and(|p| p.pane == pane) {
                    self.player = None;
                }
                self.show_image(pane, image, None, title.clone(), Some(info));
                self.message(format!("{title} done. File > Save image as (Ctrl+S) to keep it."), Some(8.0));
            }
            Action::DeleteFiles(paths) => {
                let failed = gallery::delete_files(&paths);
                self.gallery.refresh(true, None);
                if !failed.is_empty() {
                    self.warn("Delete", format!("Couldn't delete (maybe open in another program):\n{}", failed.join("\n")));
                }
            }
            Action::ClearMeasurements(id) => {
                let p = self.pane_mut(id);
                p.measurements.clear();
                p.next_id = 1;
                p.measure_selected = None;
            }
            Action::ClearMarks(id) => self.pane_mut(id).counter.marks.clear(),
            Action::UseViewAsFlat => {
                let Some(img) = self.active_pane().image() else { return };
                let dir = settings::flat_dir();
                let _ = std::fs::create_dir_all(&dir);
                let path = dir.join(format!("flat_{}.tif", timestamp()));
                match capture::save_image(&path, &img) {
                    Ok(()) => {
                        if self.use_flat(&path, false) {
                            self.message(format!("Flat reference saved to {}", path.display()), Some(8.0));
                        }
                    }
                    Err(e) => self.warn("Flat-field", e),
                }
            }
            Action::RemoveView(id) => self.remove_view(id, true),
            Action::SetPaneCamera(id, key) => self.set_pane_camera(id, key, true),
            Action::Reconnect => {
                self.reconnect_camera(true);
            }
            Action::Quit => {
                for p in &mut self.panes {
                    if let Some(c) = &mut p.camera {
                        c.stop();
                    }
                }
                self.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    pub(crate) fn gallery_actions(&mut self, ctx: &egui::Context, actions: Vec<GalleryAction>) {
        for a in actions {
            match a {
                GalleryAction::Open(p) => {
                    let id = self.active;
                    self.open_image_path(ctx, id, p);
                }
                GalleryAction::OpenInViews(paths) => self.open_paths_in_new_views(&paths),
                GalleryAction::Process(key, paths) => self.run_process(&key, Some(paths)),
                GalleryAction::Delete(paths) => {
                    if paths.is_empty() {
                        continue;
                    }
                    let what = if paths.len() == 1 { file_name(&paths[0]) } else { format!("these {} files", paths.len()) };
                    self.confirm("Delete", format!("Move {what} to the Recycle Bin?\n\nTheir .json settings files go too."),
                                 vec![Action::DeleteFiles(paths)], vec![]);
                }
                GalleryAction::FolderChanged => {}
            }
        }
    }

    // --- view events (clicks on the image) ---------------------------------------------------

    pub(crate) fn view_event(&mut self, id: u64, e: ViewEvent) {
        match e {
            ViewEvent::Activated => self.set_active(id),
            ViewEvent::Drawn(k, points) if self.alignment.drawing.is_some() => self.alignment_drawn(&k, &points),
            ViewEvent::ReferenceMoved(a, b) => self.alignment.reference = Some((a, b)),
            ViewEvent::Drawn(k, points) if self.picking_rod => {
                self.picking_rod = false;
                self.tool = None;
                if k == "circle_centre" && points.len() >= 2 {
                    let r = (points[1].0 - points[0].0).hypot(points[1].1 - points[0].1).round();
                    self.fiber.detection.min_rod_px = r;
                    self.message(format!("Smallest rod: {r:.0} px radius"), Some(5.0));
                    if self.fiber.live {
                        self.find_fiber_axes(true);
                    }
                }
            }
            ViewEvent::Drawn(k, points) => {
                let cal = self.calibration.clone();
                let p = self.pane_mut(id);
                let m = Measurement { id: p.next_id, kind: k.clone(), points };
                match compute(&m, &cal) {
                    Err(err) => {
                        let text = format!("{} not added: {err}", kind(&k).label);
                        self.message(text, Some(5.0));
                    }
                    Ok(_) => {
                        p.next_id += 1;
                        p.measure_selected = Some(m.id);
                        p.measurements.push(m);
                    }
                }
            }
            ViewEvent::ToolExit => {
                self.tool = None;
                self.counting = false;
                self.picking_rod = false;
                self.alignment.drawing = None;
            }
            ViewEvent::CountAdd(x, y) => {
                if self.counting {
                    let cls = self.count_class;
                    self.pane_mut(id).counter.add(x, y, cls);
                }
            }
            ViewEvent::CountRemove(x, y, d) => {
                self.pane_mut(id).counter.remove_nearest(x, y, d);
            }
            ViewEvent::CountUndo => {
                self.pane_mut(id).counter.marks.pop();
            }
        }
    }

    pub(crate) fn clear_measurements(&mut self, id: u64, confirm: bool) {
        let n = self.pane(id).measurements.len();
        if n == 0 {
            return;
        }
        if confirm {
            self.confirm("Clear measurements", format!("Delete all {n} measurements?"), vec![Action::ClearMeasurements(id)], vec![]);
        } else {
            self.perform(&self.ctx_clone(), Action::ClearMeasurements(id));
        }
    }

    // --- processing ---------------------------------------------------------------------------

    /// Open a processing dialog (paths: images to start with; else those selected in Captures).
    pub(crate) fn run_process(&mut self, key: &str, paths: Option<Vec<PathBuf>>) {
        if self.job.is_some() {
            return; // one at a time
        }
        let d = self.dialogs.entry(key.into()).or_insert_with(|| ProcessDialog::new(key));
        let paths = paths.unwrap_or_else(|| {
            let sel: Vec<PathBuf> = self.gallery.selected_paths().into_iter().filter(|p| !is_video(p)).collect();
            if sel.len() >= dialogs::min_images(key) { sel } else { Vec::new() }
        });
        if !paths.is_empty() {
            d.set_paths(&paths);
        }
        self.open_dialog = Some(key.into());
    }

    pub(crate) fn start_process(&mut self, key: &str, paths: Vec<PathBuf>, settings: Value) {
        let title = dialogs::title(key);
        let n = paths.len();
        let meta = json!({
            "processing": title,
            "inputs": paths.iter().map(|p| file_name(p)).collect::<Vec<_>>(),
            "input_folder": paths[0].parent().map(|p| p.display().to_string()).unwrap_or_default(),
            "settings": settings,
        });
        let (tx, rx) = mpsc::channel();
        let (k, wake) = (key.to_string(), self.wake());
        std::thread::spawn(move || {
            let mut images = Vec::new();
            for p in &paths {
                match capture::load_image(p) {
                    Some(i) => images.push(i),
                    None => {
                        let _ = tx.send(Err(format!("Could not read {}", p.display())));
                        wake();
                        return;
                    }
                }
            }
            let r = std::panic::catch_unwind(|| dialogs::process(&k, images, &settings)).unwrap_or_else(|_| Err("The tool failed unexpectedly.".into()));
            let _ = tx.send(r);
            wake();
        });
        self.job = Some(ProcessJob {
            rx,
            title: format!("{title} of {n} image{}", if n > 1 { "s" } else { "" }),
            info: ResultInfo { short_name: dialogs::short_name(key).into(), meta },
            pane: self.active,
            tool: key.into(),
        });
    }

    pub(crate) fn poll_jobs(&mut self, ctx: &egui::Context) {
        if let Some(job) = &self.job {
            if let Ok(r) = job.rx.try_recv() {
                let job = self.job.take().unwrap();
                match r {
                    Ok(img) => {
                        let pane = if self.pane_opt(job.pane).is_some() { job.pane } else { self.active };
                        let then = Action::ShowResult { pane, image: Arc::new(img), title: job.title.clone(), info: job.info };
                        self.ask_clear_annotations(ctx, pane, &job.title, then);
                    }
                    Err(e) => self.warn(dialogs::title(&job.tool), e),
                }
            }
        }
        if let Some((_, rx)) = &self.fiber_job {
            match rx.try_recv() {
                Ok(axes) => {
                    let (req, _) = self.fiber_job.take().unwrap();
                    self.fibers_done(req, axes);
                    self.next_fiber_request();
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.fiber_job = None;
                    self.message("Fiber axes: measuring failed.", Some(6.0));
                    self.next_fiber_request();
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
    }

    // --- flat-field ---------------------------------------------------------------------------

    pub(crate) fn use_flat(&mut self, path: &Path, quiet: bool) -> bool {
        let Some(img) = capture::load_image(path) else {
            if !quiet {
                self.warn("Flat-field", format!("Could not read {}", path.display()));
            }
            return false;
        };
        self.flat = Some(Arc::new(FlatField::new(&img, &file_name(path))));
        self.saved.flat_path = path.display().to_string();
        if self.saved.flat_on {
            let f = self.flat.clone();
            for p in &self.panes {
                if let Some(c) = &p.camera {
                    c.send(Cmd::Flat(f.clone()));
                }
            }
        }
        true
    }

    /// Correct every camera's frames (one reference for all).
    pub(crate) fn set_flat_live(&mut self, on: bool) {
        let on = on && self.flat.is_some();
        self.saved.flat_on = on;
        let f = if on { self.flat.clone() } else { None };
        for p in &self.panes {
            if let Some(c) = &p.camera {
                c.send(Cmd::Flat(f.clone()));
            }
        }
    }

    pub(crate) fn set_flat_from_view(&mut self) {
        if self.active_pane().image().is_none() {
            return;
        }
        if self.active_pane().live && self.saved.flat_on {
            self.warn("Flat-field", "The live view is already corrected. Turn off \"Correct live view and captures\" first, \
                                     so the reference is taken from the uncorrected image.");
            return;
        }
        self.confirm("Flat-field", "Use the image in the view as the flat reference?\n\nIt should show an empty, evenly lit \
                      field (e.g. a blank slide) with the same lighting and zoom you'll use for samples. Averaging several \
                      frames when capturing it gives a cleaner reference.", vec![Action::UseViewAsFlat], vec![]);
    }

    pub(crate) fn load_flat(&mut self) {
        let start = if settings::flat_dir().is_dir() { settings::flat_dir() } else { PathBuf::from(&self.saved.folder) };
        if let Some(path) = rfd::FileDialog::new().set_title("Load flat reference").add_filter("Images", &IMAGE_FILTER).set_directory(start).pick_file() {
            if self.use_flat(&path, false) {
                self.message(format!("Flat reference: {}", file_name(&path)), Some(5.0));
            }
        }
    }

    pub(crate) fn apply_flat_to_view(&mut self) {
        let (Some(img), Some(flat)) = (self.active_pane().image(), self.flat.clone()) else { return };
        if self.active_pane().live {
            self.warn("Flat-field", "This corrects a frozen or opened image. For the live view, turn on \"Correct live view and captures\".");
            return;
        }
        let name = self.active_source();
        let meta = json!({"processing": "Flat-field correction", "inputs": [name], "flat_field": flat.name});
        let a = self.active;
        let then = Action::ShowResult {
            pane: a,
            image: Arc::new(flat.apply(&img)),
            title: format!("{name} (flat-field corrected)"),
            info: ResultInfo { short_name: "flat".into(), meta },
        };
        let ctx = self.ctx_clone();
        self.ask_clear_annotations(&ctx, a, "Flat-field", then);
    }

    // --- exports ---------------------------------------------------------------------------------

    pub(crate) fn export_path(&self, title: &str, stem: &str) -> Option<PathBuf> {
        let default = format!("{stem}_{}.xlsx", timestamp());
        let path = rfd::FileDialog::new()
            .set_title(title)
            .add_filter("Excel", &["xlsx"])
            .add_filter("CSV", &["csv"])
            .set_directory(&self.saved.folder)
            .set_file_name(default)
            .save_file()?;
        let path = with_ext(path, &["xlsx", "csv"], "xlsx");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        Some(path)
    }

    fn beside(path: &Path, suffix: &str) -> PathBuf {
        path.with_file_name(format!("{}_{suffix}.png", path.file_stem().unwrap_or_default().to_string_lossy()))
    }

    /// Save the table (Excel or CSV) plus the image and an annotated copy.
    pub(crate) fn export_measurements(&mut self) {
        let p = self.active_pane();
        let Some(image) = p.image().filter(|_| !p.measurements.is_empty()) else {
            self.warn("Export", "There are no measurements to export.");
            return;
        };
        let Some(path) = self.export_path("Export measurements", "measurements") else { return };
        let (ms, cal, source) = (self.active_pane().measurements.clone(), self.calibration.clone(), self.active_source());
        let (ip, ap) = (Self::beside(&path, "image"), Self::beside(&path, "annotated"));
        let r = export::export_measurements(&ms, &cal, &source, &path)
            .and_then(|_| image.save(&ip).map_err(|e| e.to_string()))
            .and_then(|_| super::annotate::render_annotated(&image, &ms, &cal).save(&ap).map_err(|e| e.to_string()));
        match r {
            Ok(()) => self.message(format!("Exported {} measurements to {}, {} and {}", ms.len(), file_name(&path), file_name(&ip), file_name(&ap)), Some(8.0)),
            Err(e) => self.warn("Export", format!("Export failed: {e}")),
        }
    }

    // --- fiber axes -------------------------------------------------------------------------------

    /// Measure a view's image in the background with the panel's settings as they are now.
    /// One runs at a time; a request made meanwhile waits (the newest per view), except live frames.
    pub(crate) fn request_fibers(&mut self, id: u64, image: Arc<RgbImage>, kind: FiberKind) {
        let req = FiberRequest {
            pane: id,
            image,
            kind: kind.clone(),
            from_camera: self.pane(id).live,
            generation: self.fiber_generation,
            previous: (kind == FiberKind::Live && self.pane(id).fiber_locked && !self.pane(id).fibers.is_empty())
                .then(|| self.pane(id).fibers.iter().map(|f| (f.cx, f.cy, f.radius)).collect()),
            method: self.fiber.method.clone(),
            detection: self.fiber.detection.clone(),
            centre_only: self.fiber.centre_only,
            region: self.alignment.region,
        };
        if self.fiber_job.is_some() {
            if kind != FiberKind::Live {
                self.fiber_waiting.retain(|r| r.pane != id);
                self.fiber_waiting.push(req);
            }
            return;
        }
        let (tx, rx) = mpsc::channel();
        let wake = self.wake();
        let (img, method, det, co) = (req.image.clone(), req.method.clone(), req.detection.clone(), req.centre_only);
        let previous = req.previous.clone();
        let region = req.region;
        std::thread::Builder::new()
            .name("fiber axes".into())
            .spawn(move || {
                let axes = fiber_axis::analyse_in(&img, None, previous.as_deref(), &method, false, &det, co, region.as_ref());
                let _ = tx.send(axes);
                wake();
            })
            .unwrap();
        self.fiber_job = Some((req, rx));
    }

    fn next_fiber_request(&mut self) {
        if self.fiber_waiting.is_empty() {
            return;
        }
        let r = self.fiber_waiting.remove(0);
        if self.pane_opt(r.pane).is_some() {
            self.request_fibers(r.pane, r.image, r.kind); // with the settings as they are now
        }
    }

    fn fibers_done(&mut self, req: FiberRequest, mut axes: Vec<FiberAxis>) {
        if req.generation != self.fiber_generation || self.pane_opt(req.pane).is_none() {
            return; // old settings, or the view is gone
        }
        let p = self.pane(req.pane);
        if req.from_camera {
            if !p.live || (req.kind == FiberKind::Live && !self.fiber.live && !self.collecting_on(req.pane)) {
                return;
            }
        } else if p.live {
            return;
        } else if req.kind == FiberKind::Live && self.plays_video(req.pane) {
            if !self.fiber.live {
                return; // a playing video's frame: shown even though the video has moved on
            }
        } else if !p.image().is_some_and(|i| Arc::ptr_eq(&i, &req.image)) {
            return; // not the image shown any more
        }
        // Left to right, then top to bottom, so numbers stay put between live updates.
        axes.sort_by(|a, b| {
            let ka = ((a.cy / a.radius.max(1.0)).round() as i64, a.cx);
            let kb = ((b.cy / b.radius.max(1.0)).round() as i64, b.cx);
            ka.0.cmp(&kb.0).then(ka.1.partial_cmp(&kb.1).unwrap())
        });
        let pane = self.pane_mut(req.pane);
        let sel = pane.fiber_selected.filter(|s| *s < axes.len());
        pane.fiber_selected = sel;
        if self.fiber.detection.threshold.is_none() && !axes.is_empty() {
            self.fiber.shown_threshold = axes[sel.unwrap_or(0)].threshold;
        }
        let n = axes.len();
        let unclear = axes.iter().filter(|f| !f.clear()).count();
        let circles = |a: &[FiberAxis]| a.iter().map(|f| (f.cx, f.cy, f.radius)).collect::<Vec<_>>();
        let locked = req.kind == FiberKind::Live && fiber_axis::same_fibers(&circles(&self.pane(req.pane).fibers), &circles(&axes));
        self.pane_mut(req.pane).fiber_locked = locked;
        self.pane_mut(req.pane).fibers = axes;
        self.record_fibers(req.pane);
        self.alignment_reading(req.pane);
        if req.kind == FiberKind::Find {
            if n > 0 {
                let note = if unclear > 0 { format!(" ({unclear} unclear)") } else { String::new() };
                self.message(format!("Found {n} fiber{}{note}.", if n != 1 { "s" } else { "" }), Some(6.0));
            } else {
                self.message("No fibers found. Check the focus and lighting, or draw a region of interest round the fiber (Alignment tab).", Some(10.0));
            }
        }
    }

    /// Method or detection settings changed: re-measure what's shown (where it was measured).
    pub(crate) fn fiber_settings_changed(&mut self) {
        self.fiber_generation += 1;
        let live_updates = self.fiber.live;
        let ids: Vec<u64> = self.panes.iter().map(|p| p.id).collect();
        for id in ids {
            let p = self.pane(id);
            let has = !p.fibers.is_empty();
            let Some(img) = p.image() else { continue };
            if p.live {
                if live_updates {
                    self.pane_mut(id).fiber_last = None;
                } else if has {
                    self.request_fibers(id, img, FiberKind::Image);
                }
            } else if has || live_updates {
                self.request_fibers(id, img, FiberKind::Image);
            }
        }
    }

    /// A new image was opened in a view: clear the old axes, and with Update live on, measure it.
    fn measure_opened_image(&mut self, id: u64) {
        let p = self.pane(id);
        let Some(img) = p.image().filter(|_| !p.live) else { return };
        self.pane_mut(id).fibers.clear();
        if self.fiber.live {
            self.request_fibers(id, img, FiberKind::Image);
        }
    }

    pub(crate) fn find_fiber_axes(&mut self, quiet: bool) {
        let mut any = false;
        for id in self.targets() {
            let Some(img) = self.pane(id).image() else { continue };
            let kind = if id == self.active && !quiet { FiberKind::Find } else { FiberKind::Image };
            self.request_fibers(id, img, kind);
            any = true;
        }
        if any && !quiet {
            self.message("Finding fiber axes...", None);
            // Shows the results, unless a panel showing them its own way is open (Fiber count, Alignment).
            let showing = [PanelId::Fiber, PanelId::FiberCount, PanelId::Alignment].iter().any(|&p| self.panels.is_open(p));
            if !showing {
                self.panels.set_open(PanelId::Fiber, true);
            }
        }
    }

    // --- recording ------------------------------------------------------------------------------

    pub(crate) fn request_recording(&mut self, start: bool) {
        let (folder, name, r) = (PathBuf::from(&self.saved.folder), clean_name(&self.saved.name), self.saved.recording.clone());
        let Some(link) = self.link() else { return };
        if start {
            link.send(Cmd::Record { folder, name, fmt: r.video_format.clone(), fps: r.video_fps as f64, max_seconds: r.video_limit as f64,
                                    markup: r.draw_analysis });
        } else {
            link.send(Cmd::RecordStop);
        }
    }

    /// Tells a camera what to draw on its video, photos or time-lapse images (the view's
    /// markup, where Include markup is on), when it changes.
    fn sync_record_overlay(&mut self, i: usize) {
        let pane = &self.panes[i];
        let Some(link) = pane.camera.as_ref() else { return };
        let r = &self.saved.recording;
        let wanted = pane.live && ((link.recording && r.draw_analysis) || (link.timelapse && r.tl_markup) || r.photo_markup);
        let want: Option<RecordOverlay> = if wanted { Some(self.markup_of(pane.id, Vec::new())).filter(|m| !m.is_empty()) } else { None };
        if link.overlay_sent != want {
            let link = self.panes[i].camera.as_mut().unwrap();
            link.send(Cmd::RecordOverlay(want.clone()));
            link.overlay_sent = want;
        }
    }

    pub(crate) fn request_timelapse(&mut self, start: bool) {
        if !start {
            if let Some(l) = self.link() {
                l.send(Cmd::TimeLapseStop);
            }
            return;
        }
        let r = self.saved.recording.clone();
        let tl = TimeLapse::new(r.interval_seconds(), r.tl_count as usize, r.tl_video, r.tl_video_fps as f64, &r.video_format);
        let mut req = self.capture_request(&clean_name(&self.saved.name));
        req.markup = r.tl_markup;
        let count = r.tl_count as usize;
        if let Some(link) = self.link_mut() {
            link.timelapse = true;
            link.send(Cmd::TimeLapse(tl, req));
        }
        self.tl_info = Some((0, count, crate::camera::worker::now_epoch()));
        self.tl_status = "Starting...".into();
    }

    // --- settings files --------------------------------------------------------------------------------

    pub(crate) fn recent_settings(&self) -> Vec<PathBuf> {
        self.saved.recent_settings.iter().map(PathBuf::from).filter(|p| p.is_file()).collect()
    }

    fn remember_settings_file(&mut self, path: &Path) {
        self.settings_path = Some(path.to_path_buf());
        self.saved.recent_settings = settings::add_recent(&self.saved.recent_settings, path);
    }

    fn settings_start_folder(&self) -> PathBuf {
        self.settings_path.as_ref().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_else(settings::default_settings_dir)
    }

    pub(crate) fn open_settings_dialog(&mut self, ctx: &egui::Context) {
        let d = rfd::FileDialog::new().set_title("Open settings").add_filter("CamControl settings", &["json"]).set_directory(self.settings_start_folder());
        if let Some(p) = d.pick_file() {
            self.open_settings_path(ctx, &p, false);
        }
    }

    pub(crate) fn open_settings_path(&mut self, ctx: &egui::Context, path: &Path, quiet: bool) {
        let data = match settings::load_settings_file(path) {
            Ok(d) => d,
            Err(e) => {
                if !quiet {
                    self.warn("Open settings", format!("Could not open {}: {e}", file_name(path)));
                }
                return;
            }
        };
        let problems = self.apply_settings(ctx, &data);
        self.remember_settings_file(path);
        let mut m = format!("Settings: {}", file_name(path));
        if !problems.is_empty() {
            m += &format!(". {}", problems.join(" "));
        }
        self.message(m, Some(8.0));
    }

    pub(crate) fn save_settings(&mut self, _ctx: &egui::Context) {
        match self.settings_path.clone() {
            Some(p) => self.write_settings(&p),
            None => self.save_settings_as(),
        }
    }

    pub(crate) fn save_settings_as(&mut self) {
        let start = self.settings_path.clone().unwrap_or_else(|| settings::default_settings_dir().join("settings.json"));
        let mut d = rfd::FileDialog::new().set_title("Save settings as").add_filter("CamControl settings", &["json"]);
        if let Some(dir) = start.parent() {
            let _ = std::fs::create_dir_all(dir);
            d = d.set_directory(dir);
        }
        d = d.set_file_name(file_name(&start));
        if let Some(p) = d.save_file() {
            let p = if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("json")) { p } else { p.with_extension("json") };
            self.write_settings(&p);
        }
    }

    fn write_settings(&mut self, path: &Path) {
        match settings::save_settings_file(path, &self.current_settings()) {
            Ok(()) => {
                self.remember_settings_file(path);
                self.message(format!("Saved settings to {}", path.display()), Some(6.0));
            }
            Err(e) => self.warn("Save settings", format!("Could not save: {e}")),
        }
    }

    pub(crate) fn current_settings(&self) -> Value {
        let s = &self.saved;
        let mut data = json!({
            "capture": {"average": AVERAGE_CHOICES[s.average_idx.min(4)], "save_scale": SAVE_SCALES[s.size_idx.min(1)], "format": FORMATS[s.format_idx.min(1)].1},
            "output": {"folder": s.folder, "name": s.name},
            "flat_field": {"path": if s.flat_path.is_empty() { Value::Null } else { json!(s.flat_path) }, "on": s.flat_on},
            "recording": serde_json::to_value(&s.recording).unwrap(),
            "view": {"grid": s.grid, "grid_style": serde_json::to_value(&s.grid_style).unwrap(),
                     "crosshair": s.crosshair, "crosshair_style": serde_json::to_value(&s.crosshair_style).unwrap()},
            "views": {"layout": self.layout, "link": self.link, "views": self.view_specs()},
            "fiber": self.fiber_settings_value(),
        });
        // Each camera's control values (what it last reported, or what's waiting for it).
        let mut cameras: Map<String, Value> = self.camera_values.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
        for p in &self.panes {
            if let Some(c) = p.camera.as_ref().filter(|c| !c.values.is_empty()) {
                cameras.insert(c.key.label(), json!(c.values));
            }
        }
        if let Some(c) = self.link().filter(|c| cameras.contains_key(&c.key.label())) {
            data["camera"] = cameras[&c.key.label()].clone();
        }
        if !cameras.is_empty() {
            data["cameras"] = Value::Object(cameras);
        }
        data
    }

    /// The Fiber axis panel's settings and the Fiber axes tab's, for settings files.
    fn fiber_settings_value(&self) -> Value {
        let f = &self.fiber;
        json!({
            "method": f.method, "live": f.live, "show_parts": f.show_parts, "centre_only": f.centre_only, "advanced": f.advanced,
            "detection": serde_json::to_value(&f.detection).unwrap(),
            "outside_lines": self.saved.fiber_long_lines,
            "lines": serde_json::to_value(&self.saved.fiber_lines).unwrap(),
            "region": serde_json::to_value(self.alignment.region).unwrap(),
            "reference": self.alignment.reference.map(|(a, b)| [a.0, a.1, b.0, b.1]),
            "readings": self.alignment.n,
            "top_number": self.saved.count_first,
            "ascending": self.saved.count_ascending,
        })
    }

    /// A settings file's "fiber" section (what's there and valid); re-measures with it.
    fn apply_fiber_settings(&mut self, v: &Value) {
        let f = &mut self.fiber;
        if let Some(m) = v["method"].as_str().filter(|m| fiber_axis::METHODS.iter().any(|x| x.0 == *m)) {
            f.method = m.into();
        }
        for (key, slot) in [("live", &mut f.live), ("show_parts", &mut f.show_parts), ("centre_only", &mut f.centre_only),
                            ("advanced", &mut f.advanced)] {
            if let Some(b) = v[key].as_bool() {
                *slot = b;
            }
        }
        if v["detection"].is_object() {
            if let Ok(d) = serde_json::from_value::<fiber_axis::Detection>(v["detection"].clone()) {
                f.detection = d;
            }
        }
        if let Some(b) = v["outside_lines"].as_bool() {
            self.saved.fiber_long_lines = b;
        }
        if v["lines"].is_object() {
            self.saved.fiber_lines = FiberLineStyle::from_value(&v["lines"]);
        }
        if let Some(r) = v.get("region") {
            self.alignment.region = serde_json::from_value(r.clone()).unwrap_or(None);
        }
        if let Some(r) = v.get("reference") {
            self.alignment.reference = serde_json::from_value::<Option<[f64; 4]>>(r.clone()).ok().flatten().map(|r| ((r[0], r[1]), (r[2], r[3])));
        }
        if let Some(n) = v["readings"].as_u64() {
            self.alignment.n = (n as usize).clamp(1, 1000);
        }
        if let Some(n) = v["top_number"].as_i64() {
            self.saved.count_first = n;
        }
        if let Some(b) = v["ascending"].as_bool() {
            self.saved.count_ascending = b;
        }
        self.fiber_settings_changed();
    }

    /// Apply a settings file's contents. Returns notes about anything that couldn't be applied.
    pub(crate) fn apply_settings(&mut self, ctx: &egui::Context, data: &Value) -> Vec<String> {
        let mut problems = Vec::new();
        if let Some(v) = data.get("views").filter(|v| v.is_object()) {
            if let Some(list) = v["views"].as_array().filter(|l| !l.is_empty()) {
                let list = list.clone();
                self.restore_views(ctx, &list);
            }
            if let Some(b) = v["link"].as_bool() {
                self.link = b;
            }
            if let Some(l) = v["layout"].as_str() {
                self.set_layout(l, false);
            }
        }
        // Camera values: per camera name, or (older files) one set for the controlled camera.
        let mut cameras: BTreeMap<String, BTreeMap<String, i64>> = BTreeMap::new();
        if let Some(c) = data.get("cameras").and_then(|c| c.as_object()) {
            for (label, values) in c {
                if let Some(o) = values.as_object() {
                    cameras.insert(label.clone(), o.iter().filter_map(|(k, v)| v.as_f64().map(|n| (k.clone(), n as i64))).collect());
                }
            }
        } else if let (Some(o), Some(l)) = (data.get("camera").and_then(|c| c.as_object()), self.link()) {
            cameras.insert(l.key.label(), o.iter().filter_map(|(k, v)| v.as_f64().map(|n| (k.clone(), n as i64))).collect());
        }
        let mut waiting = false;
        for (label, values) in cameras {
            let target = self.panes.iter().find(|p| p.camera.as_ref().is_some_and(|c| c.key.label() == label)).map(|p| (p.id, p.camera.as_ref().unwrap().open));
            match target {
                Some((id, true)) => self.apply_camera_values(id, &values),
                other => {
                    waiting |= other.is_some();
                    self.camera_values.insert(label, values);
                }
            }
        }
        if waiting {
            problems.push("Camera settings will be applied when the camera opens.".into());
        }
        let cap = &data["capture"];
        if let Some(a) = cap["average"].as_u64() {
            if let Some(i) = AVERAGE_CHOICES.iter().position(|&n| n as u64 == a) {
                self.saved.average_idx = i;
            }
        }
        let scale = cap["save_scale"].as_f64().or_else(|| {
            // Files saved before 2026-10-06 held a size.
            cap["save_size"].as_array().map(|s| if s.len() == 2 && s[0] == 3264 && s[1] == 1836 { HD2_SCALE } else { 1.0 })
        });
        if let Some(s) = scale {
            if let Some(i) = SAVE_SCALES.iter().position(|&v| (v - s).abs() < 1e-6) {
                self.saved.size_idx = i;
            }
        }
        if let Some(f) = cap["format"].as_str() {
            if let Some(i) = FORMATS.iter().position(|x| x.1 == f) {
                self.saved.format_idx = i;
            }
        }
        let flat = &data["flat_field"];
        #[cfg(feature = "extras")]
        match flat["path"].as_str().map(PathBuf::from) {
            Some(p) if p.is_file() => {
                if self.saved.flat_path != p.display().to_string() || self.flat.is_none() {
                    self.use_flat(&p, false);
                }
                self.set_flat_live(flat["on"].as_bool().unwrap_or(false));
            }
            other => {
                self.set_flat_live(false);
                if let Some(p) = other {
                    problems.push(format!("Flat reference not found ({}); correction is off.", file_name(&p)));
                }
            }
        }
        if data.get("recording").is_some() {
            self.saved.recording.merge(&data["recording"]);
        }
        if let Some(f) = data["output"]["folder"].as_str().filter(|f| !f.is_empty()) {
            self.saved.folder = f.into();
            self.gallery.set_output_folder(Path::new(f));
        }
        if let Some(n) = data["output"]["name"].as_str() {
            self.saved.name = n.into();
        }
        let view = &data["view"];
        if let Some(b) = view["grid"].as_bool() {
            self.saved.grid = b;
        }
        if let Some(b) = view["crosshair"].as_bool() {
            self.saved.crosshair = b;
        }
        if view["grid_style"].is_object() {
            self.saved.grid_style = GridStyle::from_value(&view["grid_style"]);
        }
        if view["crosshair_style"].is_object() {
            self.saved.crosshair_style = CrosshairStyle::from_value(&view["crosshair_style"]);
        }
        if data["fiber"].is_object() {
            self.apply_fiber_settings(&data["fiber"]);
        }
        problems
    }

    #[cfg(test)]
    pub(crate) fn modal_open(&self) -> bool {
        matches!(self.modal, Some(super::Modal::Confirm { .. } | super::Modal::Message { .. }))
    }
}

/// What's going on with a camera that closing it would stop.
pub(crate) fn busy_list(link: &CameraLink) -> Vec<&'static str> {
    let mut v = Vec::new();
    if link.recording {
        v.push("a video is recording");
    }
    if link.timelapse {
        v.push("a time-lapse is running");
    }
    v
}
