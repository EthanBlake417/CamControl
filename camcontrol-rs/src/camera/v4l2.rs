//! Camera controls through V4L2 (Linux). Opened as a second handle on the
//! device (/dev/videoN), beside the one streaming frames; V4L2 allows that.

use v4l::control::{Control, Value};
use v4l::Device;

use super::{Controls, DeviceKey, Range};

const EXPOSURE_AUTO: u32 = 0x009a0901; // V4L2_CID_EXPOSURE_AUTO: 1 = manual
const EXPOSURE_MANUAL: i64 = 1;

fn cid(name: &str) -> Option<u32> {
    Some(match name {
        "brightness" => 0x00980900,
        "contrast" => 0x00980901,
        "saturation" => 0x00980902,
        "hue" => 0x00980903,
        "gamma" => 0x00980910,
        "gain" => 0x00980913,
        "sharpness" => 0x0098091b,
        "exposure" => 0x009a0902, // V4L2_CID_EXPOSURE_ABSOLUTE, 100 µs units
        "focus" => 0x009a090a,
        "zoom" => 0x009a090d,
        _ => return None,
    })
}

pub struct V4lControls {
    dev: Device,
    ranges: Vec<(u32, Range)>,
}

impl V4lControls {
    /// The controls of the n-th capture device with this name.
    pub fn open(key: &DeviceKey) -> Option<Self> {
        let mut seen = 0;
        for index in 0..64 {
            let Ok(dev) = Device::new(index) else { continue };
            let Ok(caps) = dev.query_caps() else { continue };
            if caps.card != key.name {
                continue;
            }
            if seen < key.n {
                seen += 1;
                continue;
            }
            let ranges = dev
                .query_controls()
                .unwrap_or_default()
                .into_iter()
                .map(|d| (d.id, Range { min: d.minimum, max: d.maximum, step: (d.step as i64).max(1), default: d.default }))
                .collect();
            return Some(Self { dev, ranges });
        }
        None
    }
}

impl Controls for V4lControls {
    fn range(&self, name: &str) -> Option<Range> {
        let id = cid(name)?;
        self.ranges.iter().find(|r| r.0 == id).map(|r| r.1)
    }

    fn get(&self, name: &str) -> Result<i64, String> {
        let id = cid(name).ok_or(format!("no {name} control"))?;
        match self.dev.control(id).map_err(|e| e.to_string())?.value {
            Value::Integer(v) => Ok(v),
            Value::Boolean(b) => Ok(b as i64),
            other => Err(format!("unexpected value {other:?}")),
        }
    }

    fn set(&self, name: &str, value: i64, _no_flags: bool) -> Result<(), String> {
        let id = cid(name).ok_or(format!("no {name} control"))?;
        if name == "exposure" && self.ranges.iter().any(|r| r.0 == EXPOSURE_AUTO) {
            let _ = self.dev.set_control(Control { id: EXPOSURE_AUTO, value: Value::Integer(EXPOSURE_MANUAL) });
        }
        self.dev.set_control(Control { id, value: Value::Integer(value) }).map_err(|e| e.to_string())
    }

    fn exposure_is_log2(&self) -> bool {
        false
    }
}
