//! Runs a camera on its own thread so the window never freezes
//! (camcontrol/gui/camera_worker.py and camera_link.py).
//!
//! The GUI sends commands; the thread applies them between frames and sends
//! events back. Only the newest frame is kept for the GUI (frames are dropped
//! if it falls behind); video recording gets every frame. With a flat-field
//! set, live frames, captures, videos and time-lapse images are corrected
//! here first.

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use image::RgbImage;
use nokhwa::utils::CameraIndex;
use serde_json::{json, Map, Value};

use super::{exposure_seconds, Camera, DeviceKey, Mode, Range};
use crate::capture::{self, CaptureOptions};
use crate::processing::flatfield::FlatField;
use crate::video::{self, VideoRecorder};

const PROGRESS_EVERY: f64 = 0.5; // seconds between recording progress events

#[derive(Clone, Debug)]
pub struct CaptureRequest {
    pub n_frames: usize,
    pub scale: f64,
    pub fmt: String,
    pub folder: PathBuf,
    pub name: String,
}

/// When to take each time-lapse image, and what's been taken.
#[derive(Clone, Debug)]
pub struct TimeLapse {
    pub interval: f64,
    /// Images to take; 0 = until stopped.
    pub count: usize,
    pub make_video: bool,
    pub video_fps: f64,
    pub video_format: String,
    pub start: f64,
    pub paths: Vec<PathBuf>,
}

pub fn now_epoch() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

impl TimeLapse {
    pub fn new(interval: f64, count: usize, make_video: bool, video_fps: f64, video_format: &str) -> Self {
        Self { interval, count, make_video, video_fps, video_format: video_format.into(), start: now_epoch(), paths: Vec::new() }
    }

    /// Fixed schedule from the start, so small delays don't add up.
    pub fn next_due(&self) -> f64 {
        self.start + self.paths.len() as f64 * self.interval
    }

    pub fn done(&self) -> bool {
        self.count > 0 && self.paths.len() >= self.count
    }

    pub fn is_due(&self) -> bool {
        !self.done() && now_epoch() >= self.next_due()
    }
}

pub enum Cmd {
    Control(String, f64),
    Capture(CaptureRequest),
    Flat(Option<Arc<FlatField>>),
    Record { folder: PathBuf, name: String, fmt: String, fps: f64, max_seconds: f64 },
    RecordStop,
    TimeLapse(TimeLapse, CaptureRequest),
    TimeLapseStop,
}

#[derive(Clone, Debug)]
pub struct OpenInfo {
    pub name: String,
    pub size: (u32, u32),
    pub modes: Vec<Mode>,
    pub ranges: BTreeMap<String, Option<Range>>,
    pub exposure_log2: bool,
}

#[derive(Clone, Debug)]
pub enum Ev {
    Opened(OpenInfo),
    OpenFailed(String),
    Settings(BTreeMap<String, i64>),
    CaptureStarted(usize, f64),
    CaptureDone(PathBuf),
    Error(String),
    /// Recording now?, video path (None: nothing written).
    RecordingChanged(bool, Option<PathBuf>),
    RecordingProgress(f64, u64),
    /// taken, total (0 = no limit), next due (epoch s), last path
    TimeLapseProgress(usize, usize, f64, PathBuf),
    TimeLapseFinished(String),
}

/// The newest frame and a count, so the GUI only takes new ones.
#[derive(Default)]
pub struct Latest {
    pub frame: Option<Arc<RgbImage>>,
    pub count: u64,
}

struct Thread {
    events: Sender<Ev>,
    latest: Arc<Mutex<Latest>>,
    wake: Arc<dyn Fn() + Send + Sync>,
    flat: Option<Arc<FlatField>>,
    recorder: Option<VideoRecorder>,
    last_progress: Option<Instant>,
    timelapse: Option<(TimeLapse, CaptureRequest)>,
}

impl Thread {
    fn send(&self, ev: Ev) {
        let _ = self.events.send(ev);
        (self.wake)();
    }

    fn run(&mut self, key: DeviceKey, index: Option<CameraIndex>, size: Option<(u32, u32)>, commands: Receiver<Cmd>, stop: Arc<AtomicBool>) {
        let Some(index) = index else {
            self.send(Ev::OpenFailed("Camera not found.".into()));
            return;
        };
        let mut cam = match Camera::open(&key, &index, size) {
            Ok(c) => c,
            Err(e) => {
                self.send(Ev::OpenFailed(e));
                return;
            }
        };
        self.send(Ev::Opened(OpenInfo {
            name: cam.name.clone(),
            size: cam.size,
            modes: cam.modes.clone(),
            ranges: cam.ranges.clone(),
            exposure_log2: cam.exposure_log2,
        }));
        self.send(Ev::Settings(cam.values()));
        while !stop.load(Ordering::Relaxed) {
            self.handle_commands(&mut cam, &commands);
            if self.timelapse.as_ref().is_some_and(|t| t.0.is_due()) {
                self.timelapse_capture(&mut cam);
            }
            let Some(mut frame) = cam.read() else { continue };
            if let Some(f) = &self.flat {
                frame = f.apply(&frame);
            }
            if self.recorder.is_some() {
                self.record(&frame);
            }
            let mut l = self.latest.lock().unwrap();
            l.frame = Some(Arc::new(frame));
            l.count += 1;
            drop(l);
            (self.wake)();
        }
        // Closing mid-recording still leaves a playable file.
        self.stop_recording();
        self.stop_timelapse(Some("Time-lapse stopped (app closed)."));
        cam.close();
    }

    fn handle_commands(&mut self, cam: &mut Camera, commands: &Receiver<Cmd>) {
        // For each control only the latest value matters (a dragged slider sends many).
        let mut latest: Vec<(String, f64)> = Vec::new();
        let mut captures = Vec::new();
        while let Ok(cmd) = commands.try_recv() {
            match cmd {
                Cmd::Control(n, v) => {
                    latest.retain(|(m, _)| *m != n);
                    latest.push((n, v));
                }
                Cmd::Capture(c) => captures.push(c),
                Cmd::Flat(f) => self.flat = f,
                Cmd::Record { folder, name, fmt, fps, max_seconds } => self.start_recording(folder, &name, &fmt, fps, max_seconds),
                Cmd::RecordStop => self.stop_recording(),
                Cmd::TimeLapse(t, o) => {
                    self.stop_timelapse(None);
                    self.timelapse = Some((t, o));
                }
                Cmd::TimeLapseStop => self.stop_timelapse(Some("Time-lapse stopped.")),
            }
        }
        for (name, value) in &latest {
            if let Err(e) = cam.set(name, *value) {
                self.send(Ev::Error(format!("Could not set {name}: {e}")));
            }
        }
        if !latest.is_empty() {
            self.send(Ev::Settings(cam.values()));
        }
        for c in captures {
            self.capture(cam, &c, Map::new(), "cap", true);
        }
    }

    /// Average, correct and save one image. Returns its path, or None on failure (reported).
    fn capture(&mut self, cam: &mut Camera, c: &CaptureRequest, extra: Map<String, Value>, prefix: &str, announce: bool) -> Option<PathBuf> {
        let exposure = cam.exposure();
        if announce {
            let per = exposure.map(|e| exposure_seconds(e as f64, cam.exposure_log2)).unwrap_or(0.0).max(1.0 / 25.0);
            self.send(Ev::CaptureStarted(c.n_frames, c.n_frames as f64 * per));
        }
        let mut frames = Vec::new();
        for _ in 0..c.n_frames * 3 + 5 {
            if let Some(f) = cam.grab_exposure() {
                frames.push(f);
                if frames.len() == c.n_frames {
                    break;
                }
            }
        }
        if frames.is_empty() {
            self.send(Ev::Error("Capture failed: no valid frames from camera.".into()));
            return None;
        }
        let mut image = capture::average(&frames);
        let mut extra = extra;
        if cam.frames_summed > 1 {
            extra.insert("frames_summed_per_exposure".into(), json!(cam.frames_summed));
        }
        if let Some(f) = &self.flat {
            image = f.apply(&image);
            extra.insert("flat_field".into(), json!(f.name));
        }
        let opts = CaptureOptions {
            exposure: exposure.map(|e| e as f64),
            exposure_seconds: exposure.map(|e| exposure_seconds(e as f64, cam.exposure_log2)),
            gain: cam.gain().map(|g| g as f64),
            frames_averaged: frames.len(),
            scale: c.scale,
            fmt: c.fmt.clone(),
            folder: c.folder.clone(),
            name: c.name.clone(),
            prefix: prefix.into(),
            extra,
        };
        match capture::save_capture(&image, &opts) {
            Ok(path) => {
                if announce {
                    self.send(Ev::CaptureDone(path.clone()));
                }
                Some(path)
            }
            Err(e) => {
                self.send(Ev::Error(format!("Capture failed: {e}")));
                None
            }
        }
    }

    fn start_recording(&mut self, folder: PathBuf, name: &str, fmt: &str, fps: f64, max_seconds: f64) {
        self.stop_recording();
        if let Err(e) = std::fs::create_dir_all(&folder) {
            self.send(Ev::Error(format!("Recording failed: {e}")));
            return;
        }
        let path = capture::next_capture_path(&folder, name, video::format_ext(fmt), "vid");
        self.recorder = Some(VideoRecorder::new(path.clone(), fmt, fps, max_seconds));
        self.last_progress = None;
        self.send(Ev::RecordingChanged(true, Some(path)));
    }

    fn record(&mut self, frame: &RgbImage) {
        let rec = self.recorder.as_mut().unwrap();
        if let Err(e) = rec.add(frame) {
            self.send(Ev::Error(format!("Recording failed: {e}")));
            self.stop_recording();
            return;
        }
        let (elapsed, written, finished) = (rec.elapsed(), rec.frames_written, rec.finished());
        if self.last_progress.is_none_or(|t| t.elapsed().as_secs_f64() >= PROGRESS_EVERY) {
            self.last_progress = Some(Instant::now());
            self.send(Ev::RecordingProgress(elapsed, written));
        }
        if finished {
            self.stop_recording();
        }
    }

    fn stop_recording(&mut self) {
        let Some(mut rec) = self.recorder.take() else { return };
        if let Err(e) = rec.close() {
            self.send(Ev::Error(format!("Recording failed: {e}")));
        }
        self.send(Ev::RecordingProgress(rec.elapsed(), rec.frames_written));
        self.send(Ev::RecordingChanged(false, (rec.frames_written > 0).then(|| rec.path.clone())));
    }

    fn timelapse_capture(&mut self, cam: &mut Camera) {
        let (tl, o) = self.timelapse.clone().unwrap();
        let mut extra = Map::new();
        extra.insert("timelapse".into(), json!({"index": tl.paths.len() + 1, "interval_s": tl.interval}));
        let Some(path) = self.capture(cam, &o, extra, "tl", false) else {
            self.stop_timelapse(Some("Time-lapse stopped after an error."));
            return;
        };
        let t = &mut self.timelapse.as_mut().unwrap().0;
        t.paths.push(path.clone());
        let (n, count, due, done) = (t.paths.len(), t.count, t.next_due(), t.done());
        self.send(Ev::TimeLapseProgress(n, count, due, path));
        if done {
            self.stop_timelapse(None);
        }
    }

    fn stop_timelapse(&mut self, reason: Option<&str>) {
        let Some((tl, o)) = self.timelapse.take() else { return };
        let n = tl.paths.len();
        let mut message = format!("{} {n} image{} saved.", reason.unwrap_or("Time-lapse finished."), if n == 1 { "" } else { "s" });
        if tl.make_video && n >= 2 {
            let out = capture::next_capture_path(&o.folder, &o.name, video::format_ext(&tl.video_format), "timelapse");
            match video::images_to_video(&tl.paths, &out, &tl.video_format, tl.video_fps) {
                Ok(p) => message += &format!(" Video: {}", p.file_name().unwrap_or_default().to_string_lossy()),
                Err(e) => message += &format!(" The video couldn't be made: {e}"),
            }
        }
        self.send(Ev::TimeLapseFinished(message));
    }
}

/// One camera in use by a view: its thread, and what it has told us.
pub struct CameraLink {
    pub key: DeviceKey,
    commands: Option<Sender<Cmd>>,
    events: Option<Receiver<Ev>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    pub latest: Arc<Mutex<Latest>>,
    pub open: bool,
    pub name: String,
    pub frame_size: Option<(u32, u32)>,
    pub modes: Vec<Mode>,
    pub ranges: BTreeMap<String, Option<Range>>,
    pub values: BTreeMap<String, i64>,
    pub exposure_log2: bool,
    /// text, color (None: normal), tooltip
    pub status: (String, Option<[u8; 3]>, String),
    pub recording: bool,
    pub timelapse: bool,
    frame_times: VecDeque<Instant>,
    pub shown: u64,
}

impl CameraLink {
    pub fn new(key: DeviceKey) -> Self {
        Self {
            key,
            commands: None,
            events: None,
            stop: Arc::new(AtomicBool::new(false)),
            handle: None,
            latest: Arc::new(Mutex::new(Latest::default())),
            open: false,
            name: String::new(),
            frame_size: None,
            modes: Vec::new(),
            ranges: BTreeMap::new(),
            values: BTreeMap::new(),
            exposure_log2: cfg!(windows),
            status: ("Connecting...".into(), None, String::new()),
            recording: false,
            timelapse: false,
            frame_times: VecDeque::new(),
            shown: 0,
        }
    }

    /// Open the camera (index None: not plugged in; the thread just says so).
    pub fn start(&mut self, index: Option<CameraIndex>, size: Option<(u32, u32)>, flat: Option<Arc<FlatField>>, wake: Arc<dyn Fn() + Send + Sync>) {
        self.stop();
        self.open = false;
        self.status = ("Connecting...".into(), None, String::new());
        self.frame_times.clear();
        let (ctx, crx) = mpsc::channel();
        let (etx, erx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        self.latest = Arc::new(Mutex::new(Latest::default()));
        self.shown = 0;
        let mut t = Thread { events: etx, latest: self.latest.clone(), wake, flat, recorder: None, last_progress: None, timelapse: None };
        let (key, s) = (self.key.clone(), stop.clone());
        self.handle = Some(std::thread::Builder::new()
            .name(format!("camera {}", self.key.label()))
            .spawn(move || t.run(key, index, size, crx, s))
            .expect("can't start a camera thread"));
        self.commands = Some(ctx);
        self.events = Some(erx);
        self.stop = stop;
    }

    /// Close the camera (a recording is finished and saved).
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        self.commands = None;
        self.events = None;
        self.open = false;
        self.recording = false;
        self.timelapse = false;
    }

    pub fn send(&self, cmd: Cmd) {
        if let Some(c) = &self.commands {
            let _ = c.send(cmd);
        }
    }

    pub fn has_worker(&self) -> bool {
        self.commands.is_some()
    }

    /// Events since last time, with this link's own state updated.
    pub fn poll(&mut self) -> Vec<Ev> {
        let Some(rx) = &self.events else { return Vec::new() };
        let events: Vec<Ev> = rx.try_iter().collect();
        for ev in &events {
            match ev {
                Ev::Opened(info) => {
                    self.open = true;
                    self.name = info.name.clone();
                    self.frame_size = Some(info.size);
                    self.modes = info.modes.clone();
                    self.ranges = info.ranges.clone();
                    self.exposure_log2 = info.exposure_log2;
                    self.values.clear();
                    self.status = ("Connected".into(), Some([0, 150, 0]), info.name.clone());
                }
                Ev::OpenFailed(m) => {
                    self.open = false;
                    self.frame_size = None;
                    self.modes.clear();
                    self.status = ("Not connected".into(), Some([200, 0, 0]), m.clone());
                }
                Ev::Settings(v) => self.values.extend(v.iter().map(|(k, v)| (k.clone(), *v))),
                Ev::RecordingChanged(on, _) => self.recording = *on,
                Ev::TimeLapseFinished(_) => self.timelapse = false,
                _ => {}
            }
        }
        events
    }

    /// The newest frame, if it's one not taken yet.
    pub fn take_frame(&mut self) -> Option<Arc<RgbImage>> {
        let l = self.latest.lock().unwrap();
        if l.count == self.shown {
            return None;
        }
        self.shown = l.count;
        let f = l.frame.clone();
        drop(l);
        self.frame_times.push_back(Instant::now());
        if self.frame_times.len() > 30 {
            self.frame_times.pop_front();
        }
        f
    }

    pub fn last_frame(&self) -> Option<Arc<RgbImage>> {
        self.latest.lock().unwrap().frame.clone()
    }

    pub fn fps(&self) -> Option<f64> {
        let (a, b) = (self.frame_times.front()?, self.frame_times.back()?);
        let span = b.duration_since(*a).as_secs_f64();
        (self.frame_times.len() > 1 && span > 0.0).then(|| (self.frame_times.len() - 1) as f64 / span)
    }
}

impl Drop for CameraLink {
    fn drop(&mut self) {
        self.stop();
    }
}
