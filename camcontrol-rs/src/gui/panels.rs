//! The panels' contents: Controls (camera, output, photo/video/time-lapse,
//! view), Measurements, Counting and Fiber axis (main_window._build_controls,
//! measure_panel.py, count_panel.py, record_panel.py, fiber_panel.py).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText};

use super::actions::SAVE_SCALES;
use super::annotate::fmt_angle;
use super::player::format_duration;
use super::settings::{AVERAGE_CHOICES, FORMATS, UNITS};
use super::{App, TUNE_DELAY};
use crate::camera::worker::{now_epoch, Cmd};
use crate::camera::{format_exposure, frame_sizes, ADJUSTABLE, EXPOSURE_US};
use crate::capture::{next_capture_path, scaled_size};
use crate::measure::{compute, fmt, is_circle, KINDS};
use crate::processing::fiber_axis::{brightest_centre, method_label, Detection, LOOK_AT, METHODS};
use crate::video::VIDEO_FORMATS;

const BIG_BUTTON: f32 = 40.0;

fn method_tip(name: &str) -> &'static str {
    match name {
        "auto" => "Works out the fiber type and uses the best method for it: Panda rod outlines\nfor Panda fibers, Mirror symmetry for Bow-tie and Elliptical ones; the Apple core\nwhen the rods are washed out (or to confirm them).",
        "core" => "For Panda fibers: the bright \"apple core\" round the fiber's core, pinched on the\ntwo sides where the rods press on it. Its mirror axes are the fiber's axes; the one\nthrough the pinches is the slow axis. Works when the rods are washed out (grey or\nfront-lit pictures). Unclear if the core isn't symmetric or isn't pinched.",
        "rods" => "For Panda fibers: finds the round outlines of the two stress rods and takes\nthe line through their centers. Uses only edges, so shading and dark patches\ndon't matter, and needs no detection settings. Unclear if the outlines are faint\nor not round (e.g. Bow-tie fibers: use Mirror symmetry for those).",
        "symmetry" => "The line the end face is most mirror-symmetric about (and at 90° to it).\nNo threshold; lopsided stress parts don't pull it.",
        "centres" => "The line through the centers of the two stress parts (Panda, Bow-tie).\nEach part counts the same whatever its shape. Elliptical fibers use Second moments.",
        _ => "The principal axis of all stress-part pixels about the fiber center.\nFar pixels count most, so the outer corners of bow-tie wedges decide it.",
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
                    let us = self.link().and_then(|l| Some((l.ranges.get(EXPOSURE_US).copied().flatten()?, l.values.get(EXPOSURE_US).copied())));
                    match info {
                        Some((Some(_), v, log2)) if open && name == "exposure" && us.is_some() => {
                            // Set as a time in ms; the slider is logarithmic so short and long times both fit.
                            let (r, cur) = us.unwrap();
                            let (lo, hi) = (r.min as f64 / 1000.0, r.max as f64 / 1000.0);
                            let mut ms = cur.unwrap_or(r.default) as f64 / 1000.0;
                            let how = if log2 {
                                "The camera itself only doubles or halves its exposure; times in between are\n\
                                 made by brightening the next shorter step in software (up to 2x), and times\n\
                                 past its longest by adding frames."
                            } else {
                                "Set in steps of 0.1 ms."
                            };
                            let step = v.map(|v| format!("\nCamera exposure value now: {v}")).unwrap_or_default();
                            ui.horizontal(|ui| {
                                ui.spacing_mut().slider_width = 150.0;
                                let slider = ui.add(egui::Slider::new(&mut ms, lo..=hi).logarithmic(true).show_value(false));
                                let speed = ms * 0.005;
                                let field = ui.add(egui::DragValue::new(&mut ms).range(lo..=hi).speed(speed).max_decimals(2)
                                    .suffix(" ms").update_while_editing(false))
                                    .on_hover_text(format!("Exposure time. Type a number of milliseconds, or drag.\n{how}{step}"));
                                if slider.changed() || field.changed() {
                                    changes.push((EXPOSURE_US, (ms * 1000.0).round() as i64));
                                }
                                if slider.is_pointer_button_down_on() || field.is_pointer_button_down_on() {
                                    holding = Some((EXPOSURE_US.to_string(), (ms * 1000.0).round() as i64));
                                }
                            });
                        }
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
        ui.group(|ui| {
            ui.set_width(ui.available_width());
            ui.strong("Histogram");
            self.histogram_box(ui, 170.0);
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
        ui.checkbox(&mut self.saved.recording.photo_markup, "Include markup").on_hover_text(MARKUP_TIP);
        if big_button(ui, "Capture", self.can_capture()).clicked() {
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
            ui.checkbox(&mut r.draw_analysis, "Include markup")
                .on_hover_text(format!("{MARKUP_TIP}\nTurn on Update live in Fiber axis so the fiber axes follow the camera."));
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
                ui.checkbox(&mut r.tl_markup, "Include markup").on_hover_text(MARKUP_TIP);
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
                                format!("⌀ {}, r {} {u}; center ({cx:.1}, {cy:.1}) px", fmt(r.diameter.unwrap()), fmt(r.radius.unwrap()))
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

    pub(crate) fn fiber_panel(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        let mut tune = false;
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(210.0);
                ui.horizontal(|ui| {
                    if primary(ui, "Find axes", true, egui::vec2(130.0, 30.0)).on_hover_text("Measure the fibers in the image in the view.").clicked() {
                        self.find_fiber_axes(false);
                    }
                    if ui.add(egui::Button::new("Clear").min_size(egui::vec2(62.0, 30.0))).on_hover_text("Remove the axes from the view.").clicked() {
                        self.active_pane_mut().fibers.clear();
                    }
                });
                if ui.checkbox(&mut self.fiber.live, "Update live").on_hover_text(
                    "Keep measuring the live view (a few times a second),\ne.g. while rotating a fiber to line up its axis;\nmeasure each saved image as it's opened, and the frames of a video\nas it plays (as often as it keeps up) or as you step through it.").changed() && self.fiber.live
                {
                    self.find_fiber_axes(true);
                }
                ui.checkbox(&mut self.fiber.show_parts, "Show stress parts").on_hover_text(
                    "Shade the stress parts the axis was measured from, with a cross at each one's center,\nand ring the point the axis turns about. Also used for the exported drawing.");
                changed |= ui.checkbox(&mut self.fiber.centre_only, "Only the center fiber").on_hover_text(
                    "Of the fibers found automatically, measure only the one nearest the middle of the image\n(e.g. the lit fiber in a V-groove, with its neighbours ignored). Faster for Update live.").changed();
                ui.horizontal(|ui| {
                    ui.label("Method:");
                    egui::ComboBox::from_id_salt("method").selected_text(method_label(&self.fiber.method)).show_ui(ui, |ui| {
                        for (name, label) in METHODS {
                            changed |= ui.selectable_value(&mut self.fiber.method, name.to_string(), label).on_hover_text(method_tip(name)).changed();
                        }
                    }).response.on_hover_text("How the axis angle is worked out. Hover over a choice for details.\nAutomatic picks the method for each fiber.");
                });
                ui.checkbox(&mut self.fiber.advanced, "Show advanced settings").on_hover_text(
                    "Smallest rod, turning about the stress parts, circle measurements and the stress part detection settings.\nNot needed normally.");
                if self.fiber.advanced {
                    ui.horizontal(|ui| {
                        ui.label("Smallest rod:");
                        let d = &mut self.fiber.detection;
                        let r = ui.add(egui::DragValue::new(&mut d.min_rod_px).range(0.0..=5000.0).speed(1.0)
                                .custom_formatter(|v, _| if v <= 0.0 { "any".into() } else { format!("{v:.0} px") }))
                            .on_hover_text("Panda rod outlines: round things with a smaller radius than this (image pixels) are never\n\
                                            taken for a stress rod, e.g. dust, bubbles or the core. \"any\" (0) tries every size;\n\
                                            raise it when a speck is outlined instead of a rod. Each fiber gets it as a share of\n\
                                            its own radius (18% to 40%).");
                        tune |= r.changed();
                        let picking = self.picking_rod;
                        if ui.add(egui::Button::new("Draw").selected(picking)).on_hover_text(
                            "Draw it on the view: click the center of the smallest rod (or anything that size),\nthen its edge. Esc cancels.").clicked() {
                            self.picking_rod = !picking;
                            self.tool = (!picking).then(|| "circle_centre".to_string());
                            self.counting = false;
                            if !picking {
                                self.message("Smallest rod: click the center of a rod, then its edge (Esc cancels).", Some(8.0));
                            }
                        }
                    });
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
                    egui::Grid::new("fibers").striped(true).num_columns(13).show(ui, |ui| {
                        for h in ["#", "Type", "Slow axis (°)", "Fast axis (°)", "Angle (°)", "Clarity", "Contrast", "Quality", "Method", "Center (px)",
                                  "Axis through (px)", "Diameter (px)", "Center brightness"] {
                            let r = ui.strong(h);
                            if h == "Angle (°)" {
                                r.on_hover_text("With a reference line (Alignment panel): how far the fiber's axis nearest\n\
                                                 perpendicular to the line is from perpendicular (slow or fast, whichever).");
                            } else if h == "Contrast" || h == "Quality" {
                                r.on_hover_text(super::fiber_log::QUALITY_HELP);
                            } else if h == "Center brightness" {
                                r.on_hover_text("Brightness (0-255) of the brightest spot near the fiber's center, e.g. light in the core.\n\
                                                 With several fibers, the brightest is ringed in green and named in the readout\n\
                                                 by its position counted from the right.");
                            } else if h == "Clarity" {
                                r.on_hover_text("How much longer the stress parts spread along the slow axis than across it.\nBelow 1.3 the axis isn't clear (e.g. a plain round fiber, or poor contrast).\nPanda rod outlines: how much of the rods' outlines was found;\nbelow 50% (or with faint outlines) the axis isn't clear.");
                            }
                        }
                        ui.end_row();
                        let p = self.pane(a);
                        let brightest = brightest_centre(&p.fibers).map(|b| b.0);
                        let drawing = self.fiber_drawing();
                        for (i, f) in p.fibers.iter().enumerate() {
                            let sel = p.fiber_selected == Some(i);
                            let clarity = match f.coverage {
                                Some(c) => format!("{:.0}% outline", c * 100.0),
                                None => format!("{:.2}", f.elongation),
                            } + if f.clear() { "" } else { " (unclear)" };
                            let (ax, ay) = f.axis_point();
                            // Quality cells (with the numbers on hover) right after Clarity, so they show without scrolling.
                            let q = f.image_quality.as_ref();
                            let quality = [egui::RichText::new(super::fiber_log::fmt_contrast(q)), super::fiber_log::grade_text(q)];
                            let plain = |t: String| (egui::RichText::new(t), false);
                            let cells = [plain((i + 1).to_string()), plain(f.fiber_type.clone()), plain(fmt_angle(drawing.shown(f.slow_deg))),
                                         plain(fmt_angle(drawing.shown(f.fast_deg()))),
                                         plain(drawing.off_perpendicular(f).zip(drawing.perpendicular_axis(f))
                                             .map(|(d, a)| format!("{a} {d:+.2}")).unwrap_or_default()), plain(clarity)].into_iter()
                                .chain(quality.map(|t| (t, true)))
                                .chain([method_label(&f.method).to_string(), format!("{:.1}, {:.1}", f.cx, f.cy), format!("{ax:.1}, {ay:.1}"),
                                        format!("{:.1}", 2.0 * f.radius),
                                        f.centre_brightness.map(|b| format!("{b:.0}{}", if brightest == Some(i) { "  (brightest)" } else { "" }))
                                            .unwrap_or_default()].map(plain));
                            for (t, hover) in cells {
                                let mut r = ui.selectable_label(sel, t);
                                if hover {
                                    r = r.on_hover_text(super::fiber_log::quality_hover(q));
                                }
                                if r.clicked() {
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
                          ellipse). Dashed yellow: fast axis. Fibers are found automatically; to keep to some of the image, draw a \
                          region of interest in the Alignment tab.");
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

const MARKUP_TIP: &str = "Draw everything shown on the view onto what's saved: fiber axes and their readout,\n\
                          fiber numbers, region of interest, reference line and measurements.";

fn avg_label(n: usize) -> String {
    format!("{n} frame{}", if n > 1 { "s" } else { "" })
}

fn capitalize(s: &str) -> String {
    super::capitalize(s)
}
