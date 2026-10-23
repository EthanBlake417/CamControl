//! The apple core of each fiber in some images, with the rods for comparison:
//! `cargo run --release --example apple_core -- <images...>`

use camcontrol::processing::fiber_axis::{analyse, apple_core, measure_axis, Detection};

fn main() {
    for path in std::env::args().skip(1) {
        let img = camcontrol::capture::load_image(std::path::Path::new(&path)).expect("unreadable image");
        let name = std::path::Path::new(&path).file_name().unwrap().to_string_lossy().to_string();
        for f in analyse(&img, None, "auto", false, &Detection::default(), false) {
            let rods = measure_axis(&img, f.cx, f.cy, f.radius, "rods", false, &Detection::default());
            let core = apple_core(&img, f.cx, f.cy, f.radius, Some(rods.slow_deg).filter(|_| rods.quality.unwrap_or(0.0) >= 1.8));
            let diff = |a: f64, b: f64| { let d = (a - b).rem_euclid(180.0); d.min(180.0 - d) };
            println!("{name:28} auto {:>10} {:6.1} {:5} {:5} | rods {:6.1} {:5} q {:.2} cov {:.2} | core {}", f.fiber_type, f.slow_deg, f.method, f.clear(),
                     rods.slow_deg, rods.clear(), rods.quality.unwrap_or(0.0), rods.coverage.unwrap_or(0.0),
                     core.map(|c| {
                         let near = if diff(c.slow_deg, rods.slow_deg) <= diff(c.fast_deg, rods.slow_deg) { c.slow_deg } else { c.fast_deg };
                         format!("{:6.1} {:5} sym {:.2}/{:.2} pinch {:.3}/{:.3} | nearest rods {:6.1} ({:4.1} off)", c.slow_deg, c.clear(), c.symmetry.0, c.symmetry.1, c.pinch.0, c.pinch.1, near, diff(near, rods.slow_deg))
                     }).unwrap_or("-".into()));
        }
    }
}
