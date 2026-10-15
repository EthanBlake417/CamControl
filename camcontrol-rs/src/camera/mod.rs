//! Open a USB (UVC) camera, grab frames, and set exposure / gain / etc.
//! (camcontrol/camera.py and uvc_controls.py).
//!
//! Frames come through nokhwa (Media Foundation on Windows, V4L2 on Linux).
//! Controls go through the platform's own camera control interfaces
//! (DirectShow IAMCameraControl / IAMVideoProcAmp on Windows, V4L2 controls
//! on Linux), so auto exposure can be switched off and the MC802's quirks
//! handled, as in the Python app:
//!
//! - Windows exposure values are log2(seconds); the MC802 exposes about 10x
//!   longer than that (exposure_seconds). After HD2 has used the camera,
//!   manual exposure writes are ignored until one write with no flags goes
//!   through, so set() does that first every time.
//! - Exposures past the camera's longest are made by adding frames in
//!   software: EXTRA_EXPOSURE_STEPS more steps, 2, 4, 8 or 16 frames summed.
//! - Linux exposure (exposure_time_absolute) is in 100 µs units; auto exposure
//!   is switched to manual when exposure is set.

pub mod fake;
pub mod worker;
#[cfg(windows)]
mod dshow;
#[cfg(target_os = "linux")]
mod v4l2;

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use image::RgbImage;
use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{ApiBackend, CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType};

/// Controls the app offers, in the order shown.
pub const ADJUSTABLE: [&str; 5] = ["exposure", "gain", "contrast", "saturation", "sharpness"];
/// Exposure steps offered past the camera's maximum, done by adding frames (Windows' log2 scale only).
pub const EXTRA_EXPOSURE_STEPS: i64 = 4;
/// The MC802's real exposure range, used if it reports a broken one.
const EXPOSURE_RANGE: (i64, i64) = (-13, -3);
const EXPOSURE_DEFAULT: i64 = -6;
const OPEN_TIMEOUT: Duration = Duration::from_secs(6);

/// A camera as listed: its name and which one of that name it is (0 for the first).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct DeviceKey {
    pub name: String,
    pub n: usize,
}

impl DeviceKey {
    pub fn label(&self) -> String {
        if self.n == 0 { self.name.clone() } else { format!("{} ({})", self.name, self.n + 1) }
    }
}

/// The cameras the system lists, as (key, nokhwa index).
pub fn camera_list() -> Vec<(DeviceKey, CameraIndex)> {
    let mut out: Vec<(DeviceKey, CameraIndex)> = Vec::new();
    if fake::enabled() {
        out.push((DeviceKey { name: fake::NAME.into(), n: 0 }, CameraIndex::Index(u32::MAX)));
    }
    if std::env::var_os("CAMCONTROL_NO_CAMERAS").is_some() {
        return out; // tests: never touch a real camera
    }
    for info in nokhwa::query(ApiBackend::Auto).unwrap_or_default() {
        let name = info.human_name();
        let n = out.iter().filter(|(k, _)| k.name == name).count();
        out.push((DeviceKey { name, n }, info.index().clone()));
    }
    out
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
    pub fourcc: String,
    pub fps: f64,
}

/// The distinct frame sizes, largest first, with the fastest fps at each.
pub fn frame_sizes(modes: &[Mode]) -> Vec<(u32, u32, Option<f64>)> {
    let mut best: BTreeMap<(u32, u32), f64> = BTreeMap::new();
    for m in modes {
        let e = best.entry((m.width, m.height)).or_insert(0.0);
        *e = e.max(m.fps);
    }
    let mut v: Vec<_> = best.into_iter().map(|((w, h), f)| (w, h, (f > 0.0).then_some(f))).collect();
    v.sort_by_key(|s| (std::cmp::Reverse(s.0 * s.1), std::cmp::Reverse(s.0)));
    v
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Range {
    pub min: i64,
    pub max: i64,
    pub step: i64,
    pub default: i64,
}

/// A camera's control interface on this platform.
pub trait Controls {
    /// The range, or None if the camera doesn't have this control.
    fn range(&self, name: &str) -> Option<Range>;
    fn get(&self, name: &str) -> Result<i64, String>;
    /// Set in manual mode (auto off); `no_flags` sends the value without any mode (MC802 workaround).
    fn set(&self, name: &str, value: i64, no_flags: bool) -> Result<(), String>;
    /// Whether exposure values are log2(seconds) (Windows) rather than 100 µs units (Linux).
    fn exposure_is_log2(&self) -> bool;
}

struct NoControls;

impl Controls for NoControls {
    fn range(&self, _: &str) -> Option<Range> {
        None
    }
    fn get(&self, name: &str) -> Result<i64, String> {
        Err(format!("This camera has no {name} control"))
    }
    fn set(&self, name: &str, _: i64, _: bool) -> Result<(), String> {
        Err(format!("This camera has no {name} control"))
    }
    fn exposure_is_log2(&self) -> bool {
        cfg!(windows)
    }
}

fn open_controls(key: &DeviceKey) -> Box<dyn Controls> {
    #[cfg(windows)]
    if let Some(c) = dshow::DsControls::open(key) {
        return Box::new(c);
    }
    #[cfg(target_os = "linux")]
    if let Some(c) = v4l2::V4lControls::open(key) {
        return Box::new(c);
    }
    let _ = key;
    Box::new(NoControls)
}

/// Approximate real exposure for an exposure value. Windows: the MC802's
/// 10 x 2^value seconds; Linux: value x 100 µs.
pub fn exposure_seconds(value: f64, log2: bool) -> f64 {
    if log2 { 10.0 * 2f64.powf(value) } else { value * 1e-4 }
}

pub fn format_exposure(value: f64, log2: bool) -> String {
    let s = exposure_seconds(value, log2);
    if s >= 1.0 { format!("{s:.2} s") } else { format!("{:.0} ms", s * 1000.0) }
}

pub struct Camera {
    cam: Source,
    pub key: DeviceKey,
    pub name: String,
    pub size: (u32, u32),
    pub modes: Vec<Mode>,
    controls: Box<dyn Controls>,
    pub ranges: BTreeMap<String, Option<Range>>,
    hw_exposure_max: Option<i64>,
    pub frames_summed: u32,
    recent: VecDeque<RgbImage>,
    sum: Vec<i32>,
    pub exposure_log2: bool,
}

fn fourcc_name(f: FrameFormat) -> String {
    match f {
        FrameFormat::MJPEG => "MJPG".into(),
        FrameFormat::YUYV => "YUY2".into(),
        FrameFormat::NV12 => "NV12".into(),
        FrameFormat::GRAY => "GRAY".into(),
        FrameFormat::RAWRGB => "RGB".into(),
        other => format!("{other:?}"),
    }
}

enum Source {
    Real(nokhwa::Camera),
    Fake(fake::FakeSource),
}

impl Camera {
    /// Open the camera at size (None: its largest), MJPG where offered there.
    pub fn open(key: &DeviceKey, index: &CameraIndex, size: Option<(u32, u32)>) -> Result<Self, String> {
        if key.name == fake::NAME && fake::enabled() {
            let (source, controls) = fake::open();
            let modes = vec![Mode { width: 640, height: 480, fourcc: "MJPG".into(), fps: 25.0 }];
            return Self::finish_open(key, Source::Fake(source), modes, Box::new(controls));
        }
        let probe = RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestFrameRate);
        let mut cam = nokhwa::Camera::new(index.clone(), probe)
            .map_err(|e| format!("Could not open camera {}. Close HD2 / OBS and try again. ({e})", key.label()))?;
        let formats: Vec<CameraFormat> = cam.compatible_camera_formats().unwrap_or_default();
        let modes: Vec<Mode> = formats
            .iter()
            .map(|f| Mode { width: f.width(), height: f.height(), fourcc: fourcc_name(f.format()), fps: f.frame_rate() as f64 })
            .collect();
        let sizes = frame_sizes(&modes);
        let want = size.or(sizes.first().map(|s| (s.0, s.1)));
        if let Some((w, h)) = want {
            // At that size: MJPG first (usually the fast one), then the fastest.
            let chosen = formats
                .iter()
                .filter(|f| (f.width(), f.height()) == (w, h))
                .max_by_key(|f| (f.format() == FrameFormat::MJPEG, f.frame_rate()));
            if let Some(f) = chosen {
                let _ = cam.set_camera_requset(RequestedFormat::new::<RgbFormat>(RequestedFormatType::Exact(*f)));
            }
        }
        cam.open_stream().map_err(|e| format!("Could not start camera {}: {e}", key.label()))?;
        Self::finish_open(key, Source::Real(cam), modes, open_controls(key))
    }

    fn finish_open(key: &DeviceKey, cam: Source, modes: Vec<Mode>, controls: Box<dyn Controls>) -> Result<Self, String> {
        let exposure_log2 = controls.exposure_is_log2();
        let mut c = Self {
            cam,
            key: key.clone(),
            name: key.name.clone(),
            size: (0, 0),
            modes,
            controls,
            ranges: BTreeMap::new(),
            hw_exposure_max: None,
            frames_summed: 1,
            recent: VecDeque::new(),
            sum: Vec::new(),
            exposure_log2,
        };
        // At long exposures the first reads can time out; keep trying for a while.
        let deadline = Instant::now() + OPEN_TIMEOUT;
        let mut frame = None;
        while frame.is_none() && Instant::now() < deadline {
            frame = c.read_raw();
        }
        let Some(frame) = frame else {
            return Err(format!("Camera {} opened but returned no frame.", key.label()));
        };
        c.size = frame.dimensions();

        for name in ADJUSTABLE {
            let r = c.controls.range(name).filter(|_| c.controls.get(name).is_ok());
            c.ranges.insert(name.into(), r);
        }
        if let Some(Some(exp)) = c.ranges.get("exposure").cloned() {
            let mut exp = exp;
            if c.exposure_log2 {
                // A nonsense reported range (e.g. 3..3, not even containing the value): use the measured one.
                let cur = c.controls.get("exposure").unwrap_or(exp.min);
                if !(exp.min < exp.max && exp.min <= cur && cur <= exp.max) {
                    exp = Range { min: EXPOSURE_RANGE.0, max: EXPOSURE_RANGE.1, step: 1, default: EXPOSURE_DEFAULT };
                }
                c.hw_exposure_max = Some(exp.max);
                exp.max += EXTRA_EXPOSURE_STEPS;
            }
            c.ranges.insert("exposure".into(), Some(exp));
            // Re-apply the current exposure, so it's in effect even if HD2 left manual writes ignored.
            if let Ok(v) = c.get("exposure") {
                let _ = c.set("exposure", v as f64);
            }
        }
        Ok(c)
    }

    /// The next frame, or None if no valid frame arrived (timeouts give black frames).
    pub fn read_raw(&mut self) -> Option<RgbImage> {
        let frame = match &mut self.cam {
            Source::Real(cam) => cam.frame().ok()?.decode_image::<RgbFormat>().ok()?,
            Source::Fake(f) => f.read(),
        };
        if frame.as_raw().iter().all(|&v| v == 0) {
            return None;
        }
        Some(frame)
    }

    /// The next live frame; during a long exposure, the sum of the last frames_summed frames.
    pub fn read(&mut self) -> Option<RgbImage> {
        let frame = self.read_raw()?;
        if self.frames_summed == 1 {
            return Some(frame);
        }
        if self.sum.len() != frame.as_raw().len() {
            self.reset_sum();
            self.sum = vec![0; frame.as_raw().len()];
        }
        for (s, &v) in self.sum.iter_mut().zip(frame.as_raw()) {
            *s += v as i32;
        }
        self.recent.push_back(frame.clone());
        if self.recent.len() > self.frames_summed as usize {
            let old = self.recent.pop_front().unwrap();
            for (s, &v) in self.sum.iter_mut().zip(old.as_raw()) {
                *s -= v as i32;
            }
        }
        // Until enough frames have arrived, scale up so brightness is right.
        let scale = self.frames_summed as f64 / self.recent.len() as f64;
        let (w, h) = frame.dimensions();
        RgbImage::from_raw(w, h, self.sum.iter().map(|&s| (s as f64 * scale).clamp(0.0, 255.0) as u8).collect())
    }

    /// One complete exposure from fresh frames (for captures).
    pub fn grab_exposure(&mut self) -> Option<RgbImage> {
        let n = self.frames_summed as usize;
        if n == 1 {
            return self.read_raw();
        }
        let mut total: Option<Vec<i32>> = None;
        let mut got = 0;
        let mut dims = (0, 0);
        for _ in 0..n * 2 + 3 {
            let Some(f) = self.read_raw() else { continue };
            dims = f.dimensions();
            let t = total.get_or_insert_with(|| vec![0; f.as_raw().len()]);
            for (s, &v) in t.iter_mut().zip(f.as_raw()) {
                *s += v as i32;
            }
            got += 1;
            if got == n {
                break;
            }
        }
        let total = total?;
        let k = n as f64 / got as f64; // a frame missed: scale to the full exposure
        RgbImage::from_raw(dims.0, dims.1, total.iter().map(|&s| (s as f64 * k).clamp(0.0, 255.0) as u8).collect())
    }

    fn reset_sum(&mut self) {
        self.recent.clear();
        self.sum.clear();
    }

    pub fn get(&self, name: &str) -> Result<i64, String> {
        if self.ranges.get(name).copied().flatten().is_none() {
            return Err(format!("This camera has no {name} control"));
        }
        let mut v = self.controls.get(name)?;
        if name == "exposure" && self.frames_summed > 1 {
            v += self.frames_summed.trailing_zeros() as i64; // + log2(frames summed)
        }
        Ok(v)
    }

    /// Set a control (clamped to its range). Returns the value read back.
    pub fn set(&mut self, name: &str, value: f64) -> Result<i64, String> {
        let r = self.ranges.get(name).copied().flatten().ok_or(format!("This camera has no {name} control"))?;
        let mut v = (value.round() as i64).clamp(r.min, r.max);
        if name == "exposure" {
            if let Some(hw) = self.hw_exposure_max {
                // Past the camera's maximum: longest real exposure, frames added.
                let extra = (v - hw).max(0);
                if (1u32 << extra) != self.frames_summed {
                    self.frames_summed = 1 << extra;
                    self.reset_sum();
                }
                v -= extra;
                self.controls.set(name, v, true)?; // the workaround: a no-flags write first
            }
        }
        self.controls.set(name, v, false)?;
        self.get(name)
    }

    pub fn values(&self) -> BTreeMap<String, i64> {
        ADJUSTABLE
            .iter()
            .filter(|n| self.ranges.get(**n).copied().flatten().is_some())
            .filter_map(|n| self.get(n).ok().map(|v| (n.to_string(), v)))
            .collect()
    }

    pub fn exposure(&self) -> Option<i64> {
        self.get("exposure").ok()
    }

    pub fn gain(&self) -> Option<i64> {
        self.get("gain").ok()
    }

    pub fn close(&mut self) {
        if let Source::Real(cam) = &mut self.cam {
            let _ = cam.stop_stream();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_largest_first_with_fastest_fps() {
        let m = |w, h, f: &str, fps| Mode { width: w, height: h, fourcc: f.into(), fps };
        let s = frame_sizes(&[m(640, 480, "YUY2", 30.0), m(1920, 1080, "YUY2", 7.0), m(1920, 1080, "MJPG", 30.0)]);
        assert_eq!(s, vec![(1920, 1080, Some(30.0)), (640, 480, Some(30.0))]);
        assert_eq!(format_exposure(-4.0, true), "625 ms");
        assert_eq!(format_exposure(-2.0, true), "2.50 s");
        assert_eq!(DeviceKey { name: "Cam".into(), n: 1 }.label(), "Cam (2)");
    }
}
