//! Calibration: how big one image pixel is, separately in X and Y.
//!
//! Minimal for now: measurements use PIXELS until Phase 2 adds measuring a
//! stage micrometer. X and Y are kept separate because the camera's pixels
//! aren't confirmed to be square.

#[derive(Clone, Debug, PartialEq)]
pub struct Calibration {
    pub name: String,
    /// Units per pixel, horizontally.
    pub x: f64,
    /// Units per pixel, vertically.
    pub y: f64,
    pub unit: String,
}

impl Calibration {
    pub fn pixels() -> Self {
        Self { name: "pixels".into(), x: 1.0, y: 1.0, unit: "px".into() }
    }
}
