//! Fiber axes of one image, with timings:
//! `cargo run --release --example fiber_one -- <image> [method] [--center]`

use std::time::Instant;

use camcontrol::processing::fiber_axis::{analyse, find_fibers, Detection};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).expect("image path");
    let method = args.get(2).filter(|a| !a.starts_with("--")).map(String::as_str).unwrap_or("auto");
    let center = args.iter().any(|a| a == "--center");
    let img = camcontrol::capture::load_image(std::path::Path::new(path)).expect("unreadable image");
    let t = Instant::now();
    let circles = find_fibers(&img, camcontrol::processing::fiber_axis::MAX_FIBERS);
    println!("find_fibers: {:.0} ms -> {:?}", t.elapsed().as_secs_f64() * 1000.0,
             circles.iter().map(|c| format!("({:.0}, {:.0}) r {:.0}", c.0, c.1, c.2)).collect::<Vec<_>>());
    let t = Instant::now();
    let found = analyse(&img, None, method, false, &Detection::default(), center);
    println!("analyse ({method}): {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    for f in &found {
        println!("  ({:.1}, {:.1}) r {:.1}: {} slow {:.2} clear {} quality {:?} coverage {:?} turned {:?} center brightness {:?}",
                 f.cx, f.cy, f.radius, f.fiber_type, f.slow_deg, f.clear(), f.quality, f.coverage, f.turned_gain, f.centre_brightness);
        if let Some(q) = &f.image_quality {
            println!("    image quality: {}", q.details().replace('\n', "; "));
        }
    }
    // A live update: following the fibers just found (as Update live does).
    let previous: Vec<(f64, f64, f64)> = found.iter().map(|f| (f.cx, f.cy, f.radius)).collect();
    let t = Instant::now();
    let followed = camcontrol::processing::fiber_axis::analyse_tracking(&img, None, Some(&previous), method, false, &Detection::default(), center);
    println!("live update (following): {:.0} ms, angles {:?}", t.elapsed().as_secs_f64() * 1000.0,
             followed.iter().map(|f| format!("{:.2}", f.slow_deg)).collect::<Vec<_>>());
    let t = Instant::now();
    for &(x, y, r) in &previous {
        camcontrol::processing::fiber_quality::fiber_quality(&img, x, y, r, &[]);
    }
    println!("image quality alone (one after another): {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    if let Some(out) = std::env::var_os("FIBER_OUT") {
        camcontrol::gui::annotate::render_fiber_axes(&img, &found, camcontrol::gui::annotate::FiberDrawing { show_parts: true, ..Default::default() }).save(out).unwrap();
    }
}

