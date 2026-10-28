//! Captures panel (gallery.py): thumbnails of the newest images and videos in
//! the output folder, or another folder chosen in the panel.
//! - Updates by itself when files in the folder change (checked every second).
//! - Double-click to open (videos play); then the arrow keys (also Home/End,
//!   Page Up/Down) step to the next or previous one and open it; with Shift
//!   or Ctrl held they only select. The image open in the main view is
//!   outlined; selected ones are tinted.
//! - Ctrl/Shift-click selects several; right-click for tools.
//! - Delete moves the selected files and their .json settings to the Recycle Bin.
//! - Thumbnails are read on background threads and cached until the file changes.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::{self, Color32, ColorImage, Key, Pos2, Rect, Sense, Stroke, TextureHandle, TextureOptions, Vec2};
use image::RgbImage;

use crate::capture::{self, is_image};
use crate::imgproc::{self as ip, Interp};
use crate::video::{self, is_video};

pub const THUMB: Vec2 = Vec2::new(160.0, 90.0);
const CELL_PAD: f32 = 6.0;
/// Tools offered for several selected images: (key, menu text).
pub const PROCESS_TOOLS: [(&str, &str); 4] = [
    ("stack", "Focus stack"),
    ("hdr", "HDR (exposure fusion)"),
    ("stitch", "Stitch"),
    ("composite", "Fluorescence composite"),
];

pub enum GalleryAction {
    Open(PathBuf),
    OpenInViews(Vec<PathBuf>),
    Process(String, Vec<PathBuf>),
    /// Ask before moving these to the Recycle Bin.
    Delete(Vec<PathBuf>),
    FolderChanged,
}

struct Loader {
    jobs: Sender<(PathBuf, SystemTime, u64)>,
    results: Receiver<(PathBuf, SystemTime, Option<RgbImage>)>,
    generation: Arc<Mutex<u64>>,
}

fn thumbnail(path: &Path) -> Option<RgbImage> {
    let img = if is_video(path) { video::first_frame(path)? } else { capture::load_image(path)? };
    let (w, h) = (img.width() as f32, img.height() as f32);
    let s = (THUMB.x / w).min(THUMB.y / h);
    Some(ip::resize_rgb(&img, ((w * s).round() as u32).max(1), ((h * s).round() as u32).max(1), Interp::Area))
}

impl Loader {
    fn new(wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let (jtx, jrx) = mpsc::channel::<(PathBuf, SystemTime, u64)>();
        let (rtx, rrx) = mpsc::channel();
        let jrx = Arc::new(Mutex::new(jrx));
        let generation = Arc::new(Mutex::new(0u64));
        for k in 0..2 {
            let (jrx, rtx, generation, wake) = (jrx.clone(), rtx.clone(), generation.clone(), wake.clone());
            std::thread::Builder::new()
                .name(format!("thumbnails {k}"))
                .spawn(move || loop {
                    let job = jrx.lock().unwrap().recv();
                    let Ok((path, mtime, gen_id)) = job else { return };
                    if gen_id != *generation.lock().unwrap() {
                        continue; // cancelled: another folder is shown
                    }
                    let small = thumbnail(&path);
                    if rtx.send((path, mtime, small)).is_err() {
                        return;
                    }
                    wake();
                })
                .unwrap();
        }
        Self { jobs: jtx, results: rrx, generation }
    }

    fn cancel(&self) {
        *self.generation.lock().unwrap() += 1;
    }
}

pub struct Gallery {
    pub folder: Option<PathBuf>,
    pub output_folder: Option<PathBuf>,
    pub follow_output: bool,
    pub count: usize,
    shown: Vec<(PathBuf, SystemTime)>,
    cache: HashMap<(PathBuf, SystemTime), Option<TextureHandle>>,
    requested: HashSet<(PathBuf, SystemTime)>,
    selected: HashSet<PathBuf>,
    current: Option<usize>,
    anchor: Option<usize>,
    open_path: Option<PathBuf>,
    loader: Loader,
    last_scan: Option<Instant>,
    scroll_to: Option<usize>,
}

fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()).to_string_lossy().to_lowercase();
    if cfg!(windows) { norm(a) == norm(b) } else { a == b || std::fs::canonicalize(a).ok() == std::fs::canonicalize(b).ok() }
}

impl Gallery {
    pub fn new(wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self {
            folder: None,
            output_folder: None,
            follow_output: true,
            count: 12,
            shown: Vec::new(),
            cache: HashMap::new(),
            requested: HashSet::new(),
            selected: HashSet::new(),
            current: None,
            anchor: None,
            open_path: None,
            loader: Loader::new(wake),
            last_scan: None,
            scroll_to: None,
        }
    }

    /// Selected files, oldest first (the order they were taken).
    pub fn selected_paths(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self.shown.iter().filter(|(p, _)| self.selected.contains(p)).map(|(p, _)| p.clone()).collect();
        v.reverse();
        v
    }

    pub fn set_open_path(&mut self, path: Option<&Path>) {
        self.open_path = path.map(|p| p.to_path_buf());
    }

    pub fn set_output_folder(&mut self, folder: &Path) {
        self.output_folder = Some(folder.to_path_buf());
        if self.follow_output {
            self.set_folder(folder);
        }
    }

    pub fn set_folder(&mut self, folder: &Path) {
        if self.folder.as_deref() != Some(folder) {
            self.folder = Some(folder.to_path_buf());
            self.selected.clear();
            self.current = None;
            self.refresh(true, None);
        }
    }

    fn recent_files(&self) -> Vec<(PathBuf, SystemTime)> {
        let Some(folder) = &self.folder else { return Vec::new() };
        let mut found: Vec<(PathBuf, SystemTime)> = std::fs::read_dir(folder)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .map(|e| e.path())
            .filter(|p| is_image(p) || is_video(p))
            .filter_map(|p| {
                let m = std::fs::metadata(&p).ok()?.modified().ok()?;
                Some((p, m))
            })
            .collect();
        found.sort_by(|a, b| b.1.cmp(&a.1)); // newest first
        found.truncate(self.count);
        found
    }

    /// Re-read the folder; true if what's shown changed. select: make this the current one.
    pub fn refresh(&mut self, force: bool, select: Option<&Path>) -> bool {
        self.last_scan = Some(Instant::now());
        let files = self.recent_files();
        let changed = files != self.shown;
        if changed || force {
            if force {
                self.loader.cancel();
                self.requested.clear();
            }
            self.shown = files;
            self.selected.retain(|p| self.shown.iter().any(|(q, _)| q == p));
        }
        if let Some(sel) = select {
            if let Some(i) = self.shown.iter().position(|(p, _)| same_path(p, sel)) {
                self.current = Some(i);
                self.anchor = Some(i);
                self.selected = [self.shown[i].0.clone()].into();
                self.scroll_to = Some(i);
            }
        }
        changed
    }

    fn take_thumbnails(&mut self, ctx: &egui::Context) {
        while let Ok((path, mtime, small)) = self.loader.results.try_recv() {
            let tex = small.map(|s| {
                let mut s = s;
                if is_video(&path) {
                    draw_play_mark(&mut s);
                }
                let ci = ColorImage::from_rgb([s.width() as usize, s.height() as usize], s.as_raw());
                ctx.load_texture(path.to_string_lossy(), ci, TextureOptions::LINEAR)
            });
            if tex.is_some() {
                self.cache.insert((path, mtime), tex);
            } else {
                self.requested.remove(&(path, mtime)); // unreadable, maybe still being written: try later
            }
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) -> Vec<GalleryAction> {
        let mut actions = Vec::new();
        self.take_thumbnails(ui.ctx());
        if self.last_scan.is_none_or(|t| t.elapsed() > Duration::from_secs(1)) && self.refresh(false, None) {
            actions.push(GalleryAction::FolderChanged);
        }
        ui.ctx().request_repaint_after(Duration::from_secs(1));
        ui.horizontal(|ui| {
            ui.label("Show last");
            let mut n = self.count as u32;
            if ui.add(egui::DragValue::new(&mut n).range(1..=500)).changed() {
                self.count = n as usize;
                self.refresh(true, None);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Refresh").clicked() {
                    self.refresh(true, None);
                }
            });
        });
        ui.horizontal(|ui| {
            ui.label("Folder");
            let text = self.folder.as_ref().map(|f| f.display().to_string()).unwrap_or_default();
            if ui.button("...").on_hover_text("Choose the folder to show").clicked() {
                let mut d = rfd::FileDialog::new().set_title("Folder to show");
                if let Some(f) = &self.folder {
                    d = d.set_directory(f);
                }
                if let Some(f) = d.pick_folder() {
                    self.follow_output = self.output_folder.as_deref().is_some_and(|o| same_path(o, &f));
                    self.set_folder(&f);
                }
            }
            ui.add(egui::Label::new(egui::RichText::new(text.clone()).weak()).truncate()).on_hover_text(text);
        });
        if ui.checkbox(&mut self.follow_output, "Same as output folder").on_hover_text("Show the folder captures are saved to").changed()
            && self.follow_output
        {
            if let Some(o) = self.output_folder.clone() {
                self.set_folder(&o);
            }
        }

        // Thumbnails: as many columns as fit.
        let cell = Vec2::new(THUMB.x + 2.0 * CELL_PAD, THUMB.y + 18.0 + 3.0 * CELL_PAD);
        let hint_h = 34.0;
        let avail = ui.available_size() - Vec2::new(0.0, hint_h);
        let cols = ((avail.x / (cell.x + 4.0)).floor() as usize).max(1);
        let mut clicked: Option<(usize, egui::Modifiers)> = None;
        let mut double: Option<usize> = None;
        let mut context: Option<usize> = None;
        let open = self.open_path.clone();
        let accent = ui.visuals().selection.bg_fill;
        let list_id = ui.id().with("thumbs");
        let scroll = egui::ScrollArea::vertical().max_height(avail.y.max(60.0)).auto_shrink([false, false]);
        let mut list_rect = Rect::NOTHING;
        scroll.show(ui, |ui| {
            let shown = self.shown.clone();
            for (row, chunk) in shown.chunks(cols).enumerate() {
                ui.horizontal(|ui| {
                    for (k, (path, mtime)) in chunk.iter().enumerate() {
                        let i = row * cols + k;
                        let (rect, resp) = ui.allocate_exact_size(cell, Sense::click());
                        list_rect = list_rect.union(rect);
                        let selected = self.selected.contains(path);
                        let is_open = open.as_deref().is_some_and(|o| same_path(o, path));
                        let p = ui.painter();
                        if selected {
                            p.rect_filled(rect.shrink(1.0), 5.0, accent.gamma_multiply(0.25));
                        }
                        let area = Rect::from_min_size(Pos2::new(rect.center().x - THUMB.x / 2.0, rect.top() + CELL_PAD), THUMB);
                        let key = (path.clone(), *mtime);
                        match self.cache.get(&key) {
                            Some(Some(tex)) => {
                                let s = tex.size_vec2();
                                let r = Rect::from_center_size(area.center(), s);
                                p.image(tex.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                            }
                            _ => {
                                p.rect_filled(area, 0.0, Color32::from_gray(60));
                                if !self.requested.contains(&key) {
                                    self.requested.insert(key.clone());
                                    let gen_id = *self.loader.generation.lock().unwrap();
                                    let _ = self.loader.jobs.send((path.clone(), *mtime, gen_id));
                                }
                            }
                        }
                        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                        let font = egui::FontId::proportional(12.0);
                        let text_rect = Rect::from_min_size(Pos2::new(rect.left() + CELL_PAD, area.bottom() + CELL_PAD), Vec2::new(rect.width() - 2.0 * CELL_PAD, 16.0));
                        let shown_name = elide_middle(ui, &name, &font, text_rect.width());
                        let rich = if is_open { egui::RichText::new(shown_name).strong() } else { egui::RichText::new(shown_name) };
                        let galley = egui::WidgetText::from(rich.font(font)).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Body);
                        p.galley(Pos2::new(text_rect.center().x - galley.size().x / 2.0, text_rect.top()), galley, ui.visuals().text_color());
                        if is_open {
                            p.rect_stroke(rect.shrink(1.5), 5.0, Stroke::new(2.0, accent), egui::StrokeKind::Inside);
                        }
                        let size_mb = std::fs::metadata(path).map(|m| format!("\n{:.1} MB", m.len() as f64 / 1e6)).unwrap_or_default();
                        let when: chrono::DateTime<chrono::Local> = (*mtime).into();
                        let resp = resp.on_hover_text(format!("{name}\n{}{size_mb}", when.format("%Y-%m-%d %H:%M:%S")));
                        if resp.double_clicked() {
                            double = Some(i);
                        } else if resp.clicked() {
                            clicked = Some((i, ui.input(|inp| inp.modifiers)));
                        }
                        if resp.secondary_clicked() {
                            context = Some(i);
                        }
                        if self.scroll_to == Some(i) {
                            resp.scroll_to_me(None);
                            self.scroll_to = None;
                        }
                        let n_sel = self.selected.len();
                        resp.context_menu(|ui| {
                            let images: Vec<PathBuf> = self.selected_paths().into_iter().filter(|p| !is_video(p)).collect();
                            let n = images.len();
                            let s = |n: usize| if n == 1 { "" } else { "s" };
                            if ui.button(if is_video(path) { "Play" } else { "Open" }).clicked() {
                                actions.push(GalleryAction::Open(path.clone()));
                                ui.close_menu();
                            }
                            if ui.add_enabled(n > 0, egui::Button::new(format!("Open in new view{} ({n} image{})", s(n), s(n))))
                                .on_hover_text("Show them next to the main image, e.g. to compare them").clicked()
                            {
                                actions.push(GalleryAction::OpenInViews(images.clone()));
                                ui.close_menu();
                            }
                            if ui.button(if cfg!(windows) { "Show in Explorer" } else { "Show in file manager" }).clicked() {
                                show_in_file_manager(path);
                                ui.close_menu();
                            }
                            #[cfg(feature = "extras")]
                            {
                                ui.separator();
                                for (key, text) in PROCESS_TOOLS {
                                    let need = if key == "composite" { 1 } else { 2 };
                                    if ui.add_enabled(n >= need, egui::Button::new(format!("{text} ({n} image{})...", s(n)))).clicked() {
                                        actions.push(GalleryAction::Process(key.into(), images.clone()));
                                        ui.close_menu();
                                    }
                                }
                            }
                            ui.separator();
                            let label = if n_sel > 1 { format!("Delete ({n_sel} selected)") } else { "Delete".into() };
                            if ui.button(label).clicked() {
                                actions.push(GalleryAction::Delete(self.selected_paths()));
                                ui.close_menu();
                            }
                        });
                    }
                });
            }
        });
        let list_hovered = ui.rect_contains_pointer(list_rect);
        if let Some(i) = context {
            if !self.selected.contains(&self.shown[i].0) {
                self.select_one(i);
            }
        }
        if let Some((i, m)) = clicked {
            self.click(i, m);
            ui.memory_mut(|mem| mem.request_focus(list_id));
        }
        if let Some(i) = double {
            self.select_one(i);
            actions.push(GalleryAction::Open(self.shown[i].0.clone()));
            ui.memory_mut(|mem| mem.request_focus(list_id));
        }
        // Keys while the list has focus (clicked last, no text field in use).
        let focused = ui.memory(|m| m.has_focus(list_id)) || (list_hovered && !ui.ctx().wants_keyboard_input());
        if focused && !self.shown.is_empty() && !ui.ctx().wants_keyboard_input() {
            let (keys, mods) = ui.input(|i| {
                ([Key::ArrowRight, Key::ArrowDown, Key::ArrowLeft, Key::ArrowUp, Key::Home, Key::End, Key::PageDown, Key::PageUp, Key::Delete]
                     .map(|k| i.key_pressed(k)), i.modifiers)
            });
            let n = self.shown.len() as isize;
            let cur = self.current.map(|c| c as isize).unwrap_or(-1);
            let step = [1, cols as isize, -1, -(cols as isize), -n, n, (cols * 3) as isize, -((cols * 3) as isize)];
            for (k, &d) in step.iter().enumerate() {
                if keys[k] {
                    let target = if k == 4 { 0 } else if k == 5 { n - 1 } else { (cur + d).clamp(0, n - 1) };
                    let t = target as usize;
                    if mods.shift || mods.ctrl || mods.command {
                        self.click(t, mods);
                    } else if Some(t) != self.current {
                        self.select_one(t);
                        actions.push(GalleryAction::Open(self.shown[t].0.clone()));
                    }
                    self.scroll_to = Some(t);
                }
            }
            if keys[8] && !self.selected.is_empty() {
                actions.push(GalleryAction::Delete(self.selected_paths()));
            }
        }
        actions
    }

    fn select_one(&mut self, i: usize) {
        self.selected = [self.shown[i].0.clone()].into();
        self.current = Some(i);
        self.anchor = Some(i);
    }

    fn click(&mut self, i: usize, m: egui::Modifiers) {
        let path = self.shown[i].0.clone();
        if m.shift {
            let a = self.anchor.unwrap_or(i);
            let (lo, hi) = (a.min(i), a.max(i));
            if !(m.ctrl || m.command) {
                self.selected.clear();
            }
            for k in lo..=hi {
                self.selected.insert(self.shown[k].0.clone());
            }
        } else if m.ctrl || m.command {
            if !self.selected.remove(&path) {
                self.selected.insert(path);
            }
            self.anchor = Some(i);
        } else {
            self.selected = [path].into();
            self.anchor = Some(i);
        }
        self.current = Some(i);
    }
}

fn elide_middle(ui: &egui::Ui, text: &str, font: &egui::FontId, width: f32) -> String {
    let fits = |s: &str| ui.painter().layout_no_wrap(s.into(), font.clone(), Color32::WHITE).size().x <= width;
    if fits(text) {
        return text.into();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut keep = chars.len();
    while keep > 2 {
        keep -= 1;
        let (a, b) = (keep.div_ceil(2), keep / 2);
        let s: String = chars[..a].iter().chain(['…'].iter()).chain(chars[chars.len() - b..].iter()).collect();
        if fits(&s) {
            return s;
        }
    }
    "…".into()
}

/// A white play triangle in a dark circle, so videos stand out from images.
fn draw_play_mark(img: &mut RgbImage) {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let (cx, cy) = (w / 2.0, h / 2.0);
    let r = w.min(h) * 0.22;
    let k = r * 0.5;
    let tri = [(cx - k * 0.7, cy - k), (cx - k * 0.7, cy + k), (cx + k, cy)];
    let inside_tri = |x: f32, y: f32| {
        let s = |a: (f32, f32), b: (f32, f32)| (b.0 - a.0) * (y - a.1) - (b.1 - a.1) * (x - a.0);
        let (d1, d2, d3) = (s(tri[0], tri[1]), s(tri[1], tri[2]), s(tri[2], tri[0]));
        (d1 >= 0.0 && d2 >= 0.0 && d3 >= 0.0) || (d1 <= 0.0 && d2 <= 0.0 && d3 <= 0.0)
    };
    for (x, y, p) in img.enumerate_pixels_mut() {
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        if inside_tri(fx, fy) {
            p.0 = [255, 255, 255];
        } else if (fx - cx).hypot(fy - cy) <= r {
            p.0 = p.0.map(|v| (v as f32 * 0.41) as u8);
        }
    }
}

pub fn show_in_file_manager(path: &Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg("/select,").arg(path).spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("xdg-open").arg(path.parent().unwrap_or(path)).spawn();
    }
}

pub fn open_folder(folder: &Path) {
    let program = if cfg!(windows) { "explorer" } else { "xdg-open" };
    let _ = std::process::Command::new(program).arg(folder).spawn();
}

/// Move the files to the Recycle Bin, with their .json sidecars (unless another
/// image shares the name). Returns the names that couldn't be moved.
pub fn delete_files(paths: &[PathBuf]) -> Vec<String> {
    let mut failed = Vec::new();
    for p in paths {
        if trash::delete(p).is_err() {
            failed.push(p.file_name().unwrap_or_default().to_string_lossy().to_string());
            continue;
        }
        let sidecar = p.with_extension("json");
        let stem = p.file_stem().unwrap_or_default().to_string_lossy().to_string();
        let shared = std::fs::read_dir(p.parent().unwrap_or(Path::new(".")))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .any(|q| q.file_stem().is_some_and(|s| s.to_string_lossy() == stem) && is_image(&q));
        if sidecar.is_file() && !shared {
            let _ = trash::delete(&sidecar);
        }
    }
    failed
}
