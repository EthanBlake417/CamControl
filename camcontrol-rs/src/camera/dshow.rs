//! Camera controls through DirectShow (IAMCameraControl / IAMVideoProcAmp),
//! the interfaces HD2 and the driver's own settings dialog use. Opened as a
//! second handle on the device, beside the one streaming frames.

use windows::core::{Interface, BSTR, PCWSTR};
use windows::Win32::Media::DirectShow::{IAMCameraControl, IAMVideoProcAmp, IBaseFilter, ICreateDevEnum};
use windows::Win32::Media::MediaFoundation::{CLSID_SystemDeviceEnum, CLSID_VideoInputDeviceCategory};
use windows::Win32::System::Com::StructuredStorage::IPropertyBag;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, IEnumMoniker, IMoniker, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Variant::VARIANT;

use super::{Controls, DeviceKey, Range};

const FLAG_MANUAL: i32 = 0x2;

/// (interface: true = camera control, false = video proc amp, property id), from strmif.h.
fn lookup(name: &str) -> Option<(bool, i32)> {
    Some(match name {
        "pan" => (true, 0),
        "tilt" => (true, 1),
        "zoom" => (true, 3),
        "exposure" => (true, 4),
        "focus" => (true, 6),
        "brightness" => (false, 0),
        "contrast" => (false, 1),
        "hue" => (false, 2),
        "saturation" => (false, 3),
        "sharpness" => (false, 4),
        "gamma" => (false, 5),
        "white_balance" => (false, 6),
        "gain" => (false, 9),
        _ => return None,
    })
}

/// All DirectShow video input devices: (friendly name, moniker).
fn devices() -> Vec<(String, IMoniker)> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED); // harmless if already initialised
        let Ok(dev_enum) = CoCreateInstance::<_, ICreateDevEnum>(&CLSID_SystemDeviceEnum, None, CLSCTX_INPROC_SERVER) else {
            return Vec::new();
        };
        let mut enum_opt: Option<IEnumMoniker> = None;
        if dev_enum.CreateClassEnumerator(&CLSID_VideoInputDeviceCategory, &mut enum_opt, 0).is_err() {
            return Vec::new();
        }
        let Some(e) = enum_opt else { return Vec::new() }; // S_FALSE: none
        let mut out = Vec::new();
        loop {
            let mut m: [Option<IMoniker>; 1] = [None];
            let mut fetched = 0u32;
            if e.Next(&mut m, Some(&mut fetched)).is_err() || fetched == 0 {
                break;
            }
            let Some(moniker) = m[0].take() else { break };
            let name = moniker
                .BindToStorage::<_, _, IPropertyBag>(None, None)
                .ok()
                .and_then(|bag| {
                    let mut v = VARIANT::default();
                    let prop: Vec<u16> = "FriendlyName\0".encode_utf16().collect();
                    bag.Read(PCWSTR(prop.as_ptr()), &mut v, None).ok()?;
                    BSTR::try_from(&v).ok().map(|b| b.to_string())
                })
                .unwrap_or_default();
            out.push((name, moniker));
        }
        out
    }
}

pub struct DsControls {
    camera: Option<IAMCameraControl>,
    amp: Option<IAMVideoProcAmp>,
}

impl DsControls {
    /// The controls of the camera with this name (the n-th of that name).
    pub fn open(key: &DeviceKey) -> Option<Self> {
        let list = devices();
        let moniker = list.iter().filter(|(n, _)| *n == key.name).nth(key.n).map(|(_, m)| m)?;
        unsafe {
            let filter: IBaseFilter = moniker.BindToObject(None, None).ok()?;
            Some(Self { camera: filter.cast().ok(), amp: filter.cast().ok() })
        }
    }

    fn range_of(&self, cam: bool, prop: i32) -> Option<(i32, i32, i32, i32)> {
        let (mut lo, mut hi, mut step, mut def, mut caps) = (0, 0, 0, 0, 0);
        unsafe {
            if cam {
                self.camera.as_ref()?.GetRange(prop, &mut lo, &mut hi, &mut step, &mut def, &mut caps).ok()?;
            } else {
                self.amp.as_ref()?.GetRange(prop, &mut lo, &mut hi, &mut step, &mut def, &mut caps).ok()?;
            }
        }
        Some((lo, hi, step, def))
    }
}

impl Controls for DsControls {
    fn range(&self, name: &str) -> Option<Range> {
        let (cam, prop) = lookup(name)?;
        let (lo, hi, step, def) = self.range_of(cam, prop)?;
        Some(Range { min: lo as i64, max: hi as i64, step: step.max(1) as i64, default: def as i64 })
    }

    fn get(&self, name: &str) -> Result<i64, String> {
        let (cam, prop) = lookup(name).ok_or(format!("no {name} control"))?;
        let (mut v, mut flags) = (0, 0);
        unsafe {
            if cam {
                self.camera.as_ref().ok_or("no camera controls")?.Get(prop, &mut v, &mut flags)
            } else {
                self.amp.as_ref().ok_or("no video controls")?.Get(prop, &mut v, &mut flags)
            }
        }
        .map_err(|e| e.to_string())?;
        Ok(v as i64)
    }

    fn set(&self, name: &str, value: i64, no_flags: bool) -> Result<(), String> {
        let (cam, prop) = lookup(name).ok_or(format!("no {name} control"))?;
        let flags = if no_flags { 0 } else { FLAG_MANUAL };
        unsafe {
            if cam {
                self.camera.as_ref().ok_or("no camera controls")?.Set(prop, value as i32, flags)
            } else {
                self.amp.as_ref().ok_or("no video controls")?.Set(prop, value as i32, flags)
            }
        }
        .map_err(|e| e.to_string())
    }

    fn exposure_is_log2(&self) -> bool {
        true
    }
}
