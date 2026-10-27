//! Fiber search on every n-th frame of a video, one line per frame (a fresh
//! search each time, as when nothing is being followed):
//! `cargo run --release --example fiber_video -- <video> [every n] [png folder]`
//! With a png folder, each measured frame is saved there too (plain, for looking at).

use std::path::Path;
use std::time::Instant;

use camcontrol::processing::fiber_axis::{analyse, Detection};
use camcontrol::video::VideoReader;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = Path::new(args.get(1).expect("video path"));
    let every: usize = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(5);
    let out = args.get(3).map(Path::new);
    let mut video = VideoReader::open(path).expect("unreadable video");
    println!("{} frames at {:.1} fps", video.frame_count, video.fps);
    for i in (0..video.frame_count).step_by(every.max(1)) {
        let Some(img) = video.frame(i) else { continue };
        if let Some(dir) = out {
            img.save(dir.join(format!("f{i:04}.png"))).unwrap();
        }
        let t = Instant::now();
        let found = analyse(&img, None, "auto", false, &Detection::default(), false);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let list: Vec<String> = found.iter().map(|f| {
            let q = f.image_quality.map(|q| format!("{:+.0}% {}", 100.0 * q.contrast, q.grade().0.label())).unwrap_or("-".into());
            format!("({:.0},{:.0}) r{:.0} {} {:.1}° {} [{q}]", f.cx, f.cy, f.radius, f.fiber_type, f.slow_deg, if f.clear() { "clear" } else { "unclear" })
        }).collect();
        println!("{i:4} {:4.0}ms {}x{} n={} {}", ms, img.width(), img.height(), found.len(), list.join("  "));
    }
}
