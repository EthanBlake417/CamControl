//! A pretend camera for tests and demos (as tests/fakes.py in the Python app):
//! listed as "Fake camera" when CAMCONTROL_FAKE_CAMERA is set. It sends
//! 640x480 frames at about 25 fps (a drawn Panda fiber turning slowly, with
//! noise) and has the MC802's controls, kept in memory; exposure and gain
//! change the brightness.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use image::RgbImage;

use super::{Controls, Range};

pub const NAME: &str = "Fake camera";
const SIZE: (u32, u32) = (640, 480);
const FRAME_TIME: Duration = Duration::from_millis(40);

pub fn enabled() -> bool {
    std::env::var_os("CAMCONTROL_FAKE_CAMERA").is_some()
}

fn ranges() -> BTreeMap<&'static str, Range> {
    let r = |min, max, default| Range { min, max, step: 1, default };
    [("exposure", r(-13, -3, -6)), ("gain", r(0, 63, 48)), ("contrast", r(0, 15, 8)), ("saturation", r(0, 15, 8)), ("sharpness", r(0, 7, 3))]
        .into_iter()
        .collect()
}

pub struct FakeSource {
    values: Arc<Mutex<BTreeMap<String, i64>>>,
    frame: u32,
    next: Instant,
    seed: u32,
}

pub struct FakeControls {
    values: Arc<Mutex<BTreeMap<String, i64>>>,
}

pub fn open() -> (FakeSource, FakeControls) {
    let values: BTreeMap<String, i64> = ranges().into_iter().map(|(k, r)| (k.to_string(), r.default)).collect();
    let values = Arc::new(Mutex::new(values));
    (FakeSource { values: values.clone(), frame: 0, next: Instant::now(), seed: 1 }, FakeControls { values })
}

impl FakeSource {
    pub fn read(&mut self) -> RgbImage {
        let now = Instant::now();
        if now < self.next {
            std::thread::sleep(self.next - now);
        }
        self.next = self.next.max(now) + FRAME_TIME;
        self.frame += 1;
        let v = self.values.lock().unwrap().clone();
        let exposure = *v.get("exposure").unwrap_or(&-6) as f32;
        let gain = *v.get("gain").unwrap_or(&48) as f32;
        let k = (2f32.powf(exposure + 6.0) * (0.5 + gain / 96.0)).clamp(0.05, 4.0);
        let a = (self.frame as f32 * 0.5).to_radians();
        let (cx, cy, r) = (320.0, 240.0, 180.0);
        let rods = [(cx + 90.0 * a.cos(), cy - 90.0 * a.sin()), (cx - 90.0 * a.cos(), cy + 90.0 * a.sin())];
        let mut img = RgbImage::new(SIZE.0, SIZE.1);
        for (x, y, p) in img.enumerate_pixels_mut() {
            self.seed = self.seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let noise = ((self.seed >> 24) as f32 - 128.0) / 32.0;
            let (fx, fy) = (x as f32, y as f32);
            let base: [f32; 3] = if (fx - cx).hypot(fy - cy) > r {
                [20.0, 20.0, 20.0]
            } else if rods.iter().any(|&(rx, ry)| (fx - rx).hypot(fy - ry) < 42.0) {
                [200.0, 120.0, 30.0]
            } else {
                [180.0, 220.0, 240.0]
            };
            p.0 = base.map(|b| (b * k + noise).clamp(1.0, 255.0) as u8);
        }
        img
    }
}

impl Controls for FakeControls {
    fn range(&self, name: &str) -> Option<Range> {
        ranges().get(name).copied()
    }

    fn get(&self, name: &str) -> Result<i64, String> {
        self.values.lock().unwrap().get(name).copied().ok_or(format!("no {name} control"))
    }

    fn set(&self, name: &str, value: i64, _no_flags: bool) -> Result<(), String> {
        let r = self.range(name).ok_or(format!("no {name} control"))?;
        self.values.lock().unwrap().insert(name.into(), value.clamp(r.min, r.max));
        Ok(())
    }

    fn exposure_is_log2(&self) -> bool {
        true
    }
}
