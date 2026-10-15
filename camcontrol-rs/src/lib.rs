//! CamControl, Rust port: live view, capture, measurement and image processing
//! for USB (UVC) microscope cameras on Windows and Linux, as one executable.
//! A port of the Python app in the folder above (camcontrol/).



pub mod calibration;
pub mod camera;
pub mod capture;
pub mod draw;
pub mod export;
pub mod imgproc;
pub mod measure;
pub mod gui;
pub mod processing;
pub mod video;

use eframe::egui;

fn icon() -> Option<egui::IconData> {
    let png = image::load_from_memory(include_bytes!("../../assets/get_logo.png")).ok()?.to_rgba8();
    Some(egui::IconData { width: png.width(), height: png.height(), rgba: png.into_raw() })
}

/// Open the main window; returns when it is closed.
pub fn run() -> eframe::Result {
    let mut viewport = egui::ViewportBuilder::default().with_title("CamControl").with_inner_size([1400.0, 850.0]);
    if let Some(icon) = icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native("CamControl", options, Box::new(|cc| Ok(Box::new(gui::App::new(cc)))))
}
