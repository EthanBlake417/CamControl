//! Headless camera check: lists cameras, opens one, prints its modes and
//! controls, and saves one frame. `cargo run --example probe -- [index] [out.png]`

use nokhwa::Camera;
use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{ApiBackend, CameraIndex, RequestedFormat, RequestedFormatType};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let index: u32 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(0);
    let out = args.get(2).cloned().unwrap_or("probe.png".into());
    for info in nokhwa::query(ApiBackend::Auto).expect("can't list cameras") {
        println!("camera {}: {}", info.index(), info.human_name());
    }
    let request = RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestResolution);
    let mut camera = Camera::new(CameraIndex::Index(index), request).expect("can't open camera");
    for f in camera.compatible_camera_formats().unwrap_or_default() {
        println!("  mode {f}");
    }
    println!("opened at {}", camera.camera_format());
    for c in camera.camera_controls().unwrap_or_default() {
        println!("  control {c}");
    }
    camera.open_stream().expect("can't start stream");
    let start = std::time::Instant::now();
    let mut frame = None;
    for _ in 0..30 {
        frame = Some(camera.frame().expect("no frame"));
    }
    println!("30 frames in {:.2} s", start.elapsed().as_secs_f64());
    let image = frame
        .unwrap()
        .decode_image::<RgbFormat>()
        .expect("can't decode");
    image.save(&out).expect("can't save");
    println!("saved {out} ({}x{})", image.width(), image.height());
}
