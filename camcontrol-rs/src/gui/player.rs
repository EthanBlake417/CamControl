//! Video playback (video_player.py): a bar under the views with play/pause,
//! frame steps and a seek slider. Frames go to a view, so a paused frame can
//! be zoomed, measured, counted or saved like any image.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use image::RgbImage;

use crate::video::VideoReader;

pub fn format_duration(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    let (h, m, s) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

pub enum PlayerAction {
    Frame(Arc<RgbImage>),
    Close,
}

pub struct Player {
    reader: VideoReader,
    pub pane: u64,
    pub position: usize,
    pub playing: bool,
    next_due: Instant,
}

impl Player {
    /// Open a video and read its first frame (paused).
    pub fn open(path: &Path, pane: u64) -> Result<(Self, Arc<RgbImage>), String> {
        let mut reader = VideoReader::open(path)?;
        let first = reader.frame(0).ok_or("Could not read the first frame.")?;
        Ok((Self { reader, pane, position: 0, playing: false, next_due: Instant::now() }, Arc::new(first)))
    }

    pub fn frame_count(&self) -> usize {
        self.reader.frame_count
    }

    pub fn fps(&self) -> f64 {
        self.reader.fps
    }

    pub fn play(&mut self) -> Option<Arc<RgbImage>> {
        self.playing = true;
        self.next_due = Instant::now();
        if self.position + 1 >= self.frame_count() {
            return self.seek(0); // at the end: start again
        }
        None
    }

    pub fn pause(&mut self) {
        self.playing = false;
    }

    pub fn seek(&mut self, index: usize) -> Option<Arc<RgbImage>> {
        let index = index.min(self.frame_count().saturating_sub(1));
        match self.reader.frame(index) {
            Some(f) => {
                self.position = index;
                Some(Arc::new(f))
            }
            None => {
                // Frame counts in headers can be a little high: stop at the real end.
                self.reader.frame_count = index.max(1);
                self.playing = false;
                None
            }
        }
    }

    /// Next frame when it's due while playing.
    pub fn tick(&mut self, ctx: &egui::Context) -> Option<Arc<RgbImage>> {
        if !self.playing {
            return None;
        }
        let now = Instant::now();
        let period = Duration::from_secs_f64(1.0 / self.fps());
        if now < self.next_due {
            ctx.request_repaint_after(self.next_due - now);
            return None;
        }
        self.next_due = (self.next_due + period).max(now - period);
        ctx.request_repaint_after(period);
        if self.position + 1 >= self.frame_count() {
            self.playing = false;
            return None;
        }
        let f = self.seek(self.position + 1);
        if f.is_none() {
            self.playing = false;
        }
        f
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) -> Vec<PlayerAction> {
        let mut out = Vec::new();
        ui.horizontal(|ui| {
            if ui.button(if self.playing { "⏸" } else { "▶" }).on_hover_text("Play / pause").clicked() {
                if self.playing {
                    self.pause();
                } else if let Some(f) = self.play() {
                    out.push(PlayerAction::Frame(f));
                }
            }
            if ui.button("⏮").on_hover_text("Previous frame").clicked() {
                self.pause();
                if let Some(f) = self.seek(self.position.saturating_sub(1)) {
                    out.push(PlayerAction::Frame(f));
                }
            }
            if ui.button("⏭").on_hover_text("Next frame").clicked() {
                self.pause();
                if let Some(f) = self.seek(self.position + 1) {
                    out.push(PlayerAction::Frame(f));
                }
            }
            let n = self.frame_count();
            let label = format!("{} / {}   frame {} of {n}", format_duration(self.position as f64 / self.fps()),
                                format_duration(n as f64 / self.fps()), self.position + 1);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Close video").clicked() {
                    out.push(PlayerAction::Close);
                }
                ui.label(label);
                let mut pos = self.position;
                ui.spacing_mut().slider_width = ui.available_width().max(60.0);
                let r = ui.add(egui::Slider::new(&mut pos, 0..=n.saturating_sub(1)).show_value(false))
                    .on_hover_text("Drag to jump to a point in the video.");
                if r.changed() && pos != self.position {
                    if let Some(f) = self.seek(pos) {
                        out.push(PlayerAction::Frame(f));
                    }
                }
            });
        });
        out
    }
}
