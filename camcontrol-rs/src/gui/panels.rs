//! The panels' contents: Controls (camera, output, photo/video/time-lapse,
//! view), Measurements, Counting and Fiber axis (main_window._build_controls,
//! measure_panel.py, count_panel.py, record_panel.py, fiber_panel.py).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText};

use super::actions::SAVE_SCALES;
use super::annotate::{class_color, fmt_angle};
use super::player::format_duration;
use super::settings::{AVERAGE_CHOICES, FORMATS, UNITS};
use super::{App, TUNE_DELAY};
use crate::camera::worker::{now_epoch, Cmd};
use crate::camera::{format_exposure, frame_sizes, ADJUSTABLE};
use crate::capture::{next_capture_path, scaled_size};
use crate::measure::{compute, fmt, is_circle, KINDS};
use crate::processing::count::MAX_CLASSES;
use crate::processing::fiber_axis::{method_label, Detection, LOOK_AT, METHODS};
use crate::video::VIDEO_FORMATS;

const BIG_BUTTON: f32 = 40.0;

fn method_tip(name: &str) -> &'static str {
    match name {
        "auto" => "Works out the fiber type and uses the best method for it: Panda rod outlines\nfor Panda fibers, Mirror symmetry for Bow-tie and Elliptical ones.",
        "rods" => "For Panda fibers: finds the round outlines of the two stress rods and takes\nthe line through their centres. Uses only edges, so shading and dark patches\ndon't matter, and needs no detection settings. Unclear if the outlines are faint\nor not round (e.g. Bow-tie fibers: use Mirror symmetry for those).",
        "symmetry" => "The line the end face is most mirror-symmetric about (and at 90° to it).\nNo threshold; lopsided stress parts don't pull it.",
        "centres" => "The line through the centres of the two stress parts (Panda, Bow-tie).\nEach part counts the same whatever its shape. Elliptical fibers use Second moments.",
        _ => "The principal axis of all stress-part pixels about the fiber centre.\nFar pixels count most, so the outer corners of bow-tie wedges decide it.",
    }
}

fn look_tip(name: &str) -> &'static str {
    match name {
        "both" => "Stress parts differ from the cladding in brightness, color or both.",
        "brightness" => "Only brightness counts. For gray stress parts, or when color fringes\n(e.g. moiré, dirt, colored light) get mistaken for stress parts.",
        _ => "Only color counts. For colored stress parts under uneven brightness.",
    }
}

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).weak().small()).wrap());
}

/// The main action of a box (Capture, Record, ...): full width, in the accent color.
fn big_button(ui: &mut egui::Ui, text: impl Into<String>, enabled: bool) -> egui::Response {
    primary(ui, text, enabled, egui::vec2(ui.available_width(), BIG_BUTTON))
}

pub(crate) fn primary(ui: &mut egui::Ui, text: impl Into<String>, enabled: bool, size: egui::Vec2) -> egui::Response {
    let (fill, color) = if enabled {
        (Color32::from_rgb(0x2f, 0x6f, 0xd0), Color32::WHITE)
    } else {
        (ui.visuals().widgets.inactive.weak_bg_fill, ui.visuals().weak_text_color())
    };
    ui.add_enabled(enabled, egui::Button::new(RichText::new(text).color(color).strong()).fill(fill).min_size(size))
}

/// An on/off button that looks like a button either way.
pub(crate) fn toggle(ui: &mut egui::Ui, on: bool, text: &str) -> egui::Response {
    ui.add(egui::Button::new(text).selected(on))
}

impl App {
    pub(crate) fn controls_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        // --- Camera ---
        let mut title = "Camera".to_string();
        if let (Some(c), true) = (self.controlled, self.panes.len() > 1) {
            title += &format!(" (view {})", self.number(c));
        }
        ui.group(|ui| {
            ui.strong(title);
            ui.horizontal(|ui| {
                let (text, color, tip) = match self.link() {
                    None => ("No camera in this view".to_string(), Some([128, 128, 128]), "Choose one in Device, or in a view's Source menu.".to_string()),
                    Some(l) => l.status.clone(),
                };
                let mut rt = RichText::new(text);
                if let Some(c) = color {
                    rt = rt.color(Color32::from_rgb(c[0], c[1], c[2]));
                }
                ui.label(rt).on_hover_text(tip);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Reconnect").on_hover_text("Connect to the camera again, e.g. after turning it on or plugging it in.").clicked() {
                        self.reconnect_camera(false);
                    }
                });
            });
            egui::Grid::new("device").num_columns(2).show(ui, |ui| {
                ui.label("Device");
                let target = self.controlled.unwrap_or(self.active);
                let current = self.pane(target).camera.as_ref().map(|c| c.key.clone());
                let text = current.as_ref().map(|k| k.label()).unwrap_or("No camera".into());
                let combo = egui::ComboBox::from_id_salt("device_combo").selected_text(text).width(220.0).show_ui(ui, |ui| {
                    if ui.memory(|m| m.data.get_temp::<bool>(egui::Id::new("devices_fresh")).is_none()) {
                        self.refresh_devices();
                        ui.memory_mut(|m| m.data.insert_temp(egui::Id::new("devices_fresh"), true));
                    }
                    let mut keys: Vec<_> = self.devices.iter().map(|d| d.0.clone()).collect();
                    if let Some(c) = &current {
                        if !keys.contains(c) {
                            keys.push(c.clone());
                        }
                    }
                    for key in keys {
                        let mut t = key.label();
                        if let Some(o) = self.panes.iter().find(|p| p.id != target && p.camera.as_ref().is_some_and(|c| c.key == key)) {
                            t += &format!(" (view {})", self.number(o.id));
                        }
                        if !self.devices.iter().any(|d| d.0 == key) {
                            t += " (not connected)";
                        }
                        if ui.selectable_label(current.as_ref() == Some(&key), t).clicked() {
                            self.set_pane_camera(target, Some(key), false);
                        }
                    }
                    if ui.selectable_label(current.is_none(), "No camera").clicked() {
                        self.set_pane_camera(target, None, false);
                    }
                });
                if !combo.response.context_menu_opened() && combo.inner.is_none() {
                    ui.memory_mut(|m| m.data.remove::<bool>(egui::Id::new("devices_fresh")));
                }
                combo.response.on_hover_text("The camera this view shows. Any USB camera the system lists will work;\none in use in another view is moved to this one.");
                ui.end_row();

                ui.label("Resolution");
                let (sizes, size, open) = match self.link() {
                    Some(l) => {
                        let mut s = frame_sizes(&l.modes);
                        if let Some(fs) = l.frame_size {
                            if !s.iter().any(|x| (x.0, x.1) == fs) {
                                s.push((fs.0, fs.1, None));
                            }
                        }
                        (s, l.frame_size, l.open)
                    }
                    None => (Vec::new(), None, false),
                };
                let label = |w: u32, h: u32, fps: Option<f64>| format!("{w} x {h}{}", fps.map(|f| format!("  ({f:.0} fps)")).unwrap_or_default());
                let cur = size.map(|(w, h)| label(w, h, sizes.iter().find(|s| (s.0, s.1) == (w, h)).and_then(|s| s.2))).unwrap_or_default();
                let mut chosen = None;
                ui.add_enabled_ui(open && sizes.len() > 1, |ui| {
                    egui::ComboBox::from_id_salt("resolution").selected_text(cur).width(220.0).show_ui(ui, |ui| {
                        for (w, h, fps) in &sizes {
                            if ui.selectable_label(size == Some((*w, *h)), label(*w, *h, *fps)).clicked() {
                                chosen = Some((*w, *h));
                            }
                        }
                    }).response.on_hover_text("Frame size from the camera. The largest has the most detail;\na smaller one may give more frames per second.");
                });
                if let (Some((w, h)), Some(key)) = (chosen, self.link().map(|l| l.key.label())) {
                    if Some((w, h)) != size {
                        self.saved.resolutions.insert(key, format!("{w}x{h}"));
                        self.reconnect_camera(false);
                    }
                }
                ui.end_row();
            });
            // Sliders.
            let open = self.link().is_some_and(|l| l.open);
            let mut changes = Vec::new();
            let mut holding = None;
            egui::Grid::new("sliders").num_columns(2).show(ui, |ui| {
                for name in ADJUSTABLE {
                    ui.label(capitalize(name));
                    let info = self.link().map(|l| (l.ranges.get(name).copied().flatten(), l.values.get(name).copied(), l.exposure_log2));
                    match info {
                        Some((Some(r), v, log2)) if open => {
                            let mut value = v.unwrap_or(r.default);
                            let text = if name == "exposure" { format!("{value}  (~{})", format_exposure(value as f64, log2)) } else { value.to_string() };
                            ui.horizontal(|ui| {
                                ui.spacing_mut().slider_width = 150.0;
                                let resp = ui.add(egui::Slider::new(&mut value, r.min..=r.max).show_value(false));
                                ui.label(text);
                                if resp.changed() {
                                    changes.push((name, value));
                                }
                                if resp.is_pointer_button_down_on() {
                                    holding = Some((name.to_string(), value));
                                }
                            });
                        }
                        _ => {
                            ui.label(RichText::new(if open { "not available" } else { "-" }).weak());
                        }
                    }
                    ui.end_row();
                }
            });
            self.slider_hold = holding;
            for (name, v) in changes {
                if let Some(link) = self.link_mut() {
                    link.values.insert(name.into(), v);
                    link.send(Cmd::Control(name.into(), v as f64));
                }
            }
        });

        // --- Output ---
        ui.group(|ui| {
            ui.strong("Output");
            egui::Grid::new("output").num_columns(2).show(ui, |ui| {
                ui.label("Folder");
                ui.horizontal(|ui| {
                    if ui.button("...").clicked() {
                        if let Some(f) = rfd::FileDialog::new().set_title("Output folder").set_directory(&self.saved.folder).pick_folder() {
                            self.saved.folder = f.display().to_string();
                            self.gallery.set_output_folder(&f);
                        }
                    }
                    ui.add(egui::Label::new(&self.saved.folder).truncate()).on_hover_text(&self.saved.folder);
                });
                ui.end_row();
                ui.label("Name");
                ui.add(egui::TextEdit::singleline(&mut self.saved.name).hint_text("blank = date and time").desired_width(200.0))
                    .on_hover_text("Files are saved as name-001, name-002, ... continuing after the\nhighest number already in the folder, so nothing is overwritten.\nCapturing from several cameras at once (Link views) adds _view1, _view2, ...");
                ui.end_row();
                ui.label("");
                let ext = FORMATS[self.saved.format_idx.min(1)].1;
                let next = if self.saved.name.trim().is_empty() {
                    format!("Next: cap_<date>_<time>.{ext}")
                } else {
                    format!("Next: {}", next_capture_path(&PathBuf::from(&self.saved.folder), &self.saved.name, ext, "cap").file_name().unwrap_or_default().to_string_lossy())
                };
                ui.label(RichText::new(next).weak());
                ui.end_row();
            });
            let mut on = self.saved.flat_on;
            if ui.add_enabled(self.flat.is_some(), egui::Checkbox::new(&mut on, "Correct live view and captures"))
                .on_hover_text("Correct uneven lighting in the live views, photos, videos and time-lapses.\nSet a flat reference first: Process > Flat-field correction.")
                .changed()
            {
                self.set_flat_live(on);
            }
        });

        // --- Photo / Video / Time-lapse ---
        ui.group(|ui| {
            ui.horizontal(|ui| {
                for (i, t) in ["Photo", "Video", "Time-lapse"].iter().enumerate() {
                    if toggle(ui, self.saved.capture_tab == i, t).clicked() {
                        self.saved.capture_tab = i;
                    }
                }
            });
            ui.separator();
            let open = self.link().is_some_and(|l| l.open);
            match self.saved.capture_tab {
                1 => self.video_tab(ui, open),
                2 => self.timelapse_tab(ui, open, ctx),
                _ => self.photo_tab(ui),
            }
        });

        // --- View ---
        ui.group(|ui| {
            ui.strong("View");
            let wide = |c: &mut egui::Ui, on: bool, text: &str, enabled: bool| {
                c.add_enabled(enabled, egui::Button::new(text).selected(on).min_size(egui::vec2(c.available_width(), 0.0)))
            };
            ui.columns(3, |c| {
                let live = self.active_pane().live;
                let has_cam = self.active_pane().camera.is_some();
                if wide(&mut c[0], live, "Live  (L)", has_cam).clicked() {
                    self.toggle_live(!live);
                }
                if wide(&mut c[1], self.saved.grid, "Grid  (G)", true).clicked() {
                    self.saved.grid = !self.saved.grid;
                }
                if wide(&mut c[2], self.saved.crosshair, "Crosshair  (C)", true).clicked() {
                    self.saved.crosshair = !self.saved.crosshair;
                }
            });
            ui.columns(2, |c| {
                if wide(&mut c[0], false, "Fit to window  (F)", true).clicked() {
                    for id in self.targets() {
                        self.pane_mut(id).view.fit();
                    }
                }
                if wide(&mut c[1], false, "100%  (1)", true).clicked() {
                    for id in self.targets() {
                        self.pane_mut(id).view.actual_size();
                    }
                }
            });
            ui.columns(2, |c| {
                if wide(&mut c[0], self.layout != "single", "Several views", true).clicked() {
                    let t = if self.layout == "single" { self.multi_layout.clone() } else { "single".into() };
                    self.set_layout(&t, true);
                }
                if wide(&mut c[1], false, "Add view", true).clicked() {
                    self.add_view_and_choose();
                }
            });
        });
    }

    fn photo_tab(&mut self, ui: &mut egui::Ui) {
        let frame_size = self.link().and_then(|l| l.frame_size);
        egui::Grid::new("photo").num_columns(2).show(ui, |ui| {
            ui.label("Average");
            egui::ComboBox::from_id_salt("average").selected_text(avg_label(AVERAGE_CHOICES[self.saved.average_idx.min(4)])).show_ui(ui, |ui| {
                for (i, n) in AVERAGE_CHOICES.iter().enumerate() {
                    ui.selectable_value(&mut self.saved.average_idx, i, avg_label(*n));
                }
            }).response.on_hover_text("Averaging several frames reduces noise.");
            ui.end_row();
            ui.label("Save size");
            let label = |i: usize| match (frame_size, i) {
                (Some((w, h)), 0) => { let (w, h) = scaled_size(w, h, 1.0); format!("{w} x {h} (native)") }
                (Some((w, h)), _) => { let (w, h) = scaled_size(w, h, SAVE_SCALES[1]); format!("{w} x {h} (HD2 size, 1.7x upscaled)") }
                (None, 0) => "Native (camera size)".into(),
                _ => "HD2 size (1.7x upscaled)".into(),
            };
            egui::ComboBox::from_id_salt("save_size").selected_text(label(self.saved.size_idx.min(1))).show_ui(ui, |ui| {
                for i in 0..2 {
                    ui.selectable_value(&mut self.saved.size_idx, i, label(i));
                }
            }).response.on_hover_text("Native saves frames at the size the camera sends. The HD2 size is\nscaled up 1.7x to match HD2 files (1920x1080 becomes 3264x1836):\nmore pixels, no extra detail, and a different µm/px.");
            ui.end_row();
            ui.label("Format");
            egui::ComboBox::from_id_salt("format").selected_text(FORMATS[self.saved.format_idx.min(1)].0).show_ui(ui, |ui| {
                for (i, (label, _)) in FORMATS.iter().enumerate() {
                    ui.selectable_value(&mut self.saved.format_idx, i, *label);
                }
            });
            ui.end_row();
        });
        if big_button(ui, "Capture  (Space)", self.can_capture()).clicked() {
            self.capture();
        }
        if !self.last_saved_label.is_empty() {
            ui.add(egui::Label::new(&self.last_saved_label).wrap());
        }
    }

    fn video_tab(&mut self, ui: &mut egui::Ui, open: bool) {
        let recording = self.link().is_some_and(|l| l.recording);
        ui.add_enabled_ui(open, |ui| {
            let r = &mut self.saved.recording;
            egui::Grid::new("video").num_columns(2).show(ui, |ui| {
                ui.add_enabled_ui(!recording, |ui| {
                    ui.label("Format");
                });
                ui.add_enabled_ui(!recording, |ui| {
                    egui::ComboBox::from_id_salt("vformat").selected_text(r.video_format.clone()).show_ui(ui, |ui| {
                        for (f, _) in VIDEO_FORMATS {
                            ui.selectable_value(&mut r.video_format, f.to_string(), f);
                        }
                    });
                });
                ui.end_row();
                ui.label("Frame rate");
                ui.add_enabled(!recording, egui::DragValue::new(&mut r.video_fps).range(1..=60).suffix(" fps"))
                    .on_hover_text("Playback frame rate. The video plays at real speed whatever the\ncamera manages (about 25 fps, less with long exposures).");
                ui.end_row();
                ui.label("Stop after");
                ui.add_enabled(!recording, egui::DragValue::new(&mut r.video_limit).range(0..=24 * 3600)
                    .custom_formatter(|v, _| if v == 0.0 { "no limit".into() } else { format!("{v:.0} s") }));
                ui.end_row();
            });
            let text = if recording { "Stop recording" } else { "Record" };
            if big_button(ui, text, open).clicked() {
                self.request_recording(!recording);
            }
            if !self.record_status.is_empty() {
                ui.add(egui::Label::new(&self.record_status).wrap());
            }
        });
    }

    fn timelapse_tab(&mut self, ui: &mut egui::Ui, open: bool, ctx: &egui::Context) {
        let running = self.link().is_some_and(|l| l.timelapse);
        ui.add_enabled_ui(open, |ui| {
            let r = &mut self.saved.recording;
            egui::Grid::new("timelapse").num_columns(2).show(ui, |ui| {
                ui.label("Every");
                ui.add_enabled_ui(!running, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut r.tl_interval).range(0.5..=100000.0).speed(0.5).fixed_decimals(1));
                        egui::ComboBox::from_id_salt("unit").selected_text(UNITS[r.tl_unit.min(2)].0).show_ui(ui, |ui| {
                            for (i, (u, _)) in UNITS.iter().enumerate() {
                                ui.selectable_value(&mut r.tl_unit, i, *u);
                            }
                        });
                    });
                });
                ui.end_row();
                ui.label("Take");
                ui.horizontal(|ui| {
                    ui.add_enabled(!running, egui::DragValue::new(&mut r.tl_count).range(0..=100000)
                        .custom_formatter(|v, _| if v == 0.0 { "until stopped".into() } else { format!("{v:.0} images") }));
                    if r.tl_count > 0 {
                        let total = format_duration((r.tl_count as f64 - 1.0) * r.interval_seconds());
                        let mut tip = "How long the time-lapse takes.".to_string();
                        if r.tl_video {
                            tip += &format!(" The video will last {:.1} s.", r.tl_count as f64 / r.tl_video_fps as f64);
                        }
                        ui.label(RichText::new(format!("= {total}")).weak()).on_hover_text(tip);
                    }
                });
                ui.end_row();
            });
            ui.add_enabled_ui(!running, |ui| {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut r.tl_video, "Make a video at").on_hover_text("Join the images into a video at the end (format from the Video tab).");
                    ui.add(egui::DragValue::new(&mut r.tl_video_fps).range(1..=60).suffix(" fps"));
                });
            });
            let text = if running { "Stop time-lapse" } else { "Start time-lapse" };
            if big_button(ui, text, open).on_hover_text("Each image uses the Photo tab's averaging, size and format.").clicked() {
                self.request_timelapse(!running);
            }
            if running {
                if let Some((taken, total, due)) = self.tl_info {
                    let of = if total > 0 { format!(" of {total}") } else { String::new() };
                    self.tl_status = if total > 0 && taken >= total {
                        format!("{taken}{of} images taken. Finishing...")
                    } else {
                        format!("{taken}{of} images taken. Next in {}", format_duration(due - now_epoch()))
                    };
                }
                ctx.request_repaint_after(Duration::from_millis(500));
            }
            ui.add(egui::Label::new(&self.tl_status).wrap());
        });
    }

    pub(crate) fn measure_panel(&mut self, ui: &mut egui::Ui) {
        let a = self.active;
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(120.0);
                let tool_button = |ui: &mut egui::Ui, on: bool, text: &str| {
                    ui.add(egui::Button::new(text).selected(on).min_size(egui::vec2(120.0, 0.0)))
                };
                if tool_button(ui, self.tool.is_none(), "Pan").clicked() {
                    self.tool = None;
                }
                for k in KINDS {
                    if tool_button(ui, self.tool.as_deref() == Some(k.name), k.label).clicked() {
                        self.tool = Some(k.name.into());
                        self.counting = false; // one clicking mode at a time
                    }
                }
            });
            let u = self.calibration.unit.clone();
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    if ui.button("Delete").on_hover_text("Delete the selected measurement (or press Delete).").clicked() {
                        self.delete_selected_measurement();
                    }
                    if ui.button("Clear all").clicked() {
                        self.clear_measurements(a, true);
                    }
                    if ui.button("Export...").on_hover_text("Save the table as Excel or CSV, plus the image and an annotated copy.").clicked() {
                        self.export_measurements();
                    }
                });
                let cal = self.calibration.clone();
                let mut select = None;
                egui::ScrollArea::vertical().max_height((ui.available_height() - 40.0).max(60.0)).auto_shrink([false, true]).show(ui, |ui| {
                    egui::Grid::new("measurements").striped(true).num_columns(6).show(ui, |ui| {
                        for h in ["#".to_string(), "Type".into(), format!("Length / perim. ({u})"), format!("Area ({u}²)"), "Angle (°)".into(), "Details".into()] {
                            ui.strong(h);
                        }
                        ui.end_row();
                        let p = self.pane(a);
                        for m in &p.measurements {
                            let Ok(r) = compute(m, &cal) else { continue };
                            let sel = p.measure_selected == Some(m.id);
                            let length = r.length.or(r.perimeter);
                            let details = if is_circle(&m.kind) {
                                let (cx, cy) = r.center_px.unwrap();
                                format!("⌀ {}, r {} {u}; centre ({cx:.1}, {cy:.1}) px", fmt(r.diameter.unwrap()), fmt(r.radius.unwrap()))
                            } else if m.kind == "rectangle" {
                                format!("{} × {} {u}", fmt(r.width.unwrap()), fmt(r.height.unwrap()))
                            } else if m.kind == "polyline" || m.kind == "polygon" {
                                format!("{} points", m.points.len())
                            } else {
                                String::new()
                            };
                            if ui.selectable_label(sel, m.id.to_string()).clicked() {
                                select = Some(m.id);
                            }
                            for t in [crate::measure::kind(&m.kind).label.to_string(), length.map(fmt).unwrap_or_default(),
                                      r.area.map(fmt).unwrap_or_default(), r.angle_deg.map(fmt).unwrap_or_default(), details] {
                                if ui.selectable_label(sel, t).clicked() {
                                    select = Some(m.id);
                                }
                            }
                            ui.end_row();
                        }
                    });
                });
                if let Some(s) = select {
                    self.pane_mut(a).measure_selected = Some(s);
                }
                hint(ui, "Click points on the image. Polyline/polygon: double-click, right-click or Enter to finish. \
                          Backspace undoes a point, Esc cancels. Middle-drag pans.");
            });
        });
        if ui.ui_contains_pointer() && !ui.ctx().wants_keyboard_input() && ui.input(|i| i.key_pressed(egui::Key::Delete)) {
            self.delete_selected_measurement();
        }
    }

    fn delete_selected_measurement(&mut self) {
        let p = self.active_pane_mut();
        if let Some(sel) = p.measure_selected.take() {
            p.measurements.retain(|m| m.id != sel);
        }
    }

    pub(crate) fn count_panel(&mut self, ui: &mut egui::Ui) {
        let a = self.active;
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(110.0);
                if ui.add(egui::Button::new("Count").selected(self.counting).min_size(egui::vec2(110.0, 28.0))).on_hover_text("Turn on to add marks by clicking the image.").clicked() {
                    self.counting = !self.counting;
                    if self.counting {
                        self.tool = None;
                    }
                }
                if ui.add(egui::Button::new("Undo").min_size(egui::vec2(110.0, 0.0))).clicked() {
                    self.pane_mut(a).counter.marks.pop();
                }
                if ui.add(egui::Button::new("Clear all").min_size(egui::vec2(110.0, 0.0))).clicked() {
                    self.clear_marks(a);
                }
                if ui.add(egui::Button::new("Export...").min_size(egui::vec2(110.0, 0.0))).on_hover_text("Save the counts as Excel or CSV, plus an image with the marks drawn on.").clicked() {
                    self.export_counts();
                }
            });
            ui.vertical(|ui| {
                let counts = self.pane(a).counter.counts();
                let total: usize = counts.iter().sum();
                egui::Grid::new("classes").num_columns(4).show(ui, |ui| {
                    for i in 0..MAX_CLASSES {
                        ui.radio_value(&mut self.count_class, i, "");
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                        let c = class_color(i);
                        ui.painter().rect(rect, 0.0, Color32::from_rgb(c[0], c[1], c[2]), egui::Stroke::new(1.0, Color32::BLACK), egui::StrokeKind::Inside);
                        let mut name = self.class_names[i].clone();
                        if ui.add_sized([170.0, 20.0], egui::TextEdit::singleline(&mut name)).changed() {
                            self.class_names[i] = if name.trim().is_empty() { format!("Class {}", i + 1) } else { name.trim().to_string() };
                        }
                        let n = counts[i];
                        ui.label(if total > 0 { format!("{n}  ({:.0}%)", 100.0 * n as f64 / total as f64) } else { "0".into() });
                        ui.end_row();
                    }
                    ui.label("");
                    ui.label("");
                    ui.label("");
                    ui.strong(format!("Total: {total}"));
                    ui.end_row();
                });
                hint(ui, "Left-click adds a mark of the selected class. Right-click removes the nearest mark. \
                          Backspace undoes the last one, Esc stops counting. Middle-drag pans.");
            });
        });
    }

    pub(crate) fn fiber_panel(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        let mut tune = false;
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(210.0);
                if primary(ui, "Find axes", true, egui::vec2(200.0, 30.0)).on_hover_text("Measure the fibers in the image in the view.").clicked() {
                    self.find_fiber_axes(false);
                }
                if ui.checkbox(&mut self.fiber.live, "Update live").on_hover_text(
                    "Keep measuring the live view (a few times a second),\ne.g. while rotating a fiber to line up its axis,\nand measure each saved image as it's opened.").changed() && self.fiber.live
                {
                    self.find_fiber_axes(true);
                }
                ui.checkbox(&mut self.fiber.show_parts, "Show stress parts").on_hover_text(
                    "Shade the stress parts the axis was measured from, with a cross at each one's centre,\nand ring the point the axis turns about. Also used for the exported drawing.");
                changed |= ui.checkbox(&mut self.fiber.centre_only, "Only the centre fiber").on_hover_text(
                    "Of the fibers found automatically, measure only the one nearest the middle of the image\n(e.g. the lit fiber in a V-groove, with its neighbours ignored). Faster for Update live.").changed();
                ui.checkbox(&mut self.fiber.use_circles, "Use circle measurements").on_hover_text(
                    "If there are Circle measurements, measure the fibers inside them\ninstead of finding fibers automatically.");
                ui.horizontal(|ui| {
                    if ui.button("Clear").clicked() {
                        self.active_pane_mut().fibers.clear();
                    }
                    if ui.button("Export...").on_hover_text("Save the table as Excel or CSV, plus the image with the axes drawn on.").clicked() {
                        self.export_fiber_axes();
                    }
                });
                ui.checkbox(&mut self.fiber.advanced, "Show advanced settings").on_hover_text(
                    "The method and the stress part detection settings.\nNot needed normally: Automatic picks the method for each fiber.");
                if self.fiber.advanced {
                    ui.horizontal(|ui| {
                        ui.label("Method:");
                        egui::ComboBox::from_id_salt("method").selected_text(method_label(&self.fiber.method)).show_ui(ui, |ui| {
                            for (name, label) in METHODS {
                                changed |= ui.selectable_value(&mut self.fiber.method, name.to_string(), label).on_hover_text(method_tip(name)).changed();
                            }
                        }).response.on_hover_text("How the axis angle is worked out. Hover over a choice for details.");
                    });
                    changed |= ui.add_enabled(self.fiber.method != "rods", egui::Checkbox::new(&mut self.fiber.parts_centre, "Turn about stress parts"))
                        .on_hover_text("Measure the axis about the centre of the stress parts instead of the fiber centre\n(Mirror symmetry and Second moments; for stress parts off the fiber's middle).").changed();
                }
            });
            if self.fiber.advanced {
                ui.add_enabled_ui(self.fiber.method != "rods", |ui| {
                    ui.group(|ui| {
                        ui.set_width(300.0);
                        ui.strong("Stress part detection").on_hover_text("Tune these when the shaded stress parts don't match what you see\n(turn on Show stress parts and Update live to watch them change).");
                        let d = &mut self.fiber.detection;
                        egui::Grid::new("detection").num_columns(2).show(ui, |ui| {
                            ui.label("Look at:");
                            let label = LOOK_AT.iter().find(|l| l.0 == d.look_at).map(|l| l.1).unwrap_or("");
                            egui::ComboBox::from_id_salt("look").selected_text(label).show_ui(ui, |ui| {
                                for (name, label, _) in LOOK_AT {
                                    changed |= ui.selectable_value(&mut d.look_at, name.to_string(), label).on_hover_text(look_tip(name)).changed();
                                }
                            });
                            ui.end_row();
                            ui.label("Threshold:");
                            ui.horizontal(|ui| {
                                let mut auto = d.threshold.is_none();
                                if ui.checkbox(&mut auto, "Auto").on_hover_text("Pick the threshold automatically (Otsu's method).\nUntick to set it yourself, starting from the automatic value.").changed() {
                                    d.threshold = if auto { None } else { Some((self.fiber.shown_threshold * 2.0).round() / 2.0) };
                                    changed = true;
                                }
                                let mut t = d.threshold.unwrap_or(self.fiber.shown_threshold);
                                let r = ui.add_enabled(!auto, egui::Slider::new(&mut t, 0.5..=40.0).step_by(0.5).fixed_decimals(1))
                                    .on_hover_text("How different from the cladding a pixel must be to count as stress part,\nin multiples of the cladding's own noise. Lower finds fainter parts, but also noise.");
                                if r.changed() {
                                    d.threshold = Some(t);
                                    tune = true;
                                }
                            });
                            ui.end_row();
                            ui.label("Smoothing:");
                            tune |= ui.add(egui::Slider::new(&mut d.smoothing, 0.0..=5.0).step_by(0.25).suffix("%"))
                                .on_hover_text("Blur before comparing, as % of the fiber radius.\nMore smooths out noise and fine texture, but rounds off small parts.").changed();
                            ui.end_row();
                            ui.label("Ignore specks:");
                            tune |= ui.add(egui::Slider::new(&mut d.min_size, 1.0..=40.0).step_by(1.0).suffix("%"))
                                .on_hover_text("Leave out regions smaller than a square this wide (% of the fiber radius):\ndust, scratches, noise.").changed();
                            ui.end_row();
                        });
                        if ui.button("Defaults").on_hover_text("Back to the standard detection settings.").clicked() {
                            *d = Detection::default();
                            changed = true;
                        }
                    });
                });
            }
            ui.vertical(|ui| {
                let a = self.active;
                let mut select = None;
                egui::ScrollArea::both().max_height((ui.available_height() - 50.0).max(60.0)).auto_shrink([false, true]).show(ui, |ui| {
                    egui::Grid::new("fibers").striped(true).num_columns(9).show(ui, |ui| {
                        for h in ["#", "Type", "Slow axis (°)", "Fast axis (°)", "Clarity", "Method", "Centre (px)", "Axis through (px)", "Diameter (px)"] {
                            let r = ui.strong(h);
                            if h == "Clarity" {
                                r.on_hover_text("How much longer the stress parts spread along the slow axis than across it.\nBelow 1.3 the axis isn't clear (e.g. a plain round fiber, or poor contrast).\nPanda rod outlines: how much of the rods' outlines was found;\nbelow 50% (or with faint outlines) the axis isn't clear.");
                            }
                        }
                        ui.end_row();
                        let p = self.pane(a);
                        for (i, f) in p.fibers.iter().enumerate() {
                            let sel = p.fiber_selected == Some(i);
                            let clarity = match f.coverage {
                                Some(c) => format!("{:.0}% outline", c * 100.0),
                                None => format!("{:.2}", f.elongation),
                            } + if f.clear() { "" } else { " (unclear)" };
                            let (ax, ay) = f.axis_point();
                            for t in [(i + 1).to_string(), f.fiber_type.clone(), fmt_angle(f.slow_deg), fmt_angle(f.fast_deg()), clarity,
                                      method_label(&f.method).to_string(), format!("{:.1}, {:.1}", f.cx, f.cy), format!("{ax:.1}, {ay:.1}"),
                                      format!("{:.1}", 2.0 * f.radius)] {
                                if ui.selectable_label(sel, t).clicked() {
                                    select = Some(i);
                                }
                            }
                            ui.end_row();
                        }
                    });
                });
                if let Some(s) = select {
                    self.pane_mut(a).fiber_selected = Some(s);
                    if let Some(t) = self.pane(a).fibers.get(s).map(|f| f.threshold) {
                        if self.fiber.detection.threshold.is_none() {
                            self.fiber.shown_threshold = t;
                        }
                    }
                }
                hint(ui, "Clarity: how sure the result is; \"(unclear)\" means check it by eye. Angles: 0° is horizontal, + is turned \
                          counter-clockwise, - clockwise (-90° to +90°). Solid magenta line: slow axis (through the stress rods / wedges / \
                          ellipse). Dashed yellow: fast axis. Fibers are found automatically; for one that isn't, draw a Circle \
                          (3 pt or centre) round it in Measurements, then Find axes.");
            });
        });
        if changed {
            self.fiber_settings_changed();
        }
        if tune {
            self.tune_due = Some(Instant::now() + Duration::from_secs_f64(TUNE_DELAY));
        }
    }
}

fn avg_label(n: usize) -> String {
    format!("{n} frame{}", if n > 1 { "s" } else { "" })
}

fn capitalize(s: &str) -> String {
    super::capitalize(s)
}
