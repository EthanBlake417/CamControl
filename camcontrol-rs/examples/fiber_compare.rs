//! Compare the Rust fiber axis results with the Python app's, on the cases
//! written by a Python script: `cargo run --release --example fiber_compare -- <folder>`
//! (the folder holds the images and cases.json).

use std::time::Instant;

use camcontrol::processing::fiber_axis::{analyse, Detection};

fn angle_error(a: f64, b: f64) -> f64 {
    (((a - b) + 90.0).rem_euclid(180.0) - 90.0).abs()
}

fn main() {
    let folder = std::path::PathBuf::from(std::env::args().nth(1).expect("folder"));
    let cases: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(folder.join("cases.json")).unwrap()).unwrap();
    let (mut worst, mut diffs, mut py_err, mut rs_err) = (0.0f64, Vec::new(), Vec::new(), Vec::new());
    let (mut py_ms, mut rs_ms) = (0.0, 0.0);
    for case in cases.as_array().unwrap() {
        let file = case["file"].as_str().unwrap();
        let img = image::open(folder.join(file)).unwrap().to_rgb8();
        let t0 = Instant::now();
        let found = analyse(&img, None, case["method"].as_str().unwrap(), false, &Detection::default(),
                            case["centre_only"].as_bool().unwrap());
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        rs_ms += ms;
        py_ms += case["python_ms"].as_f64().unwrap();
        let py = case["python"].as_array().unwrap();
        let truth = case["truth"].as_f64();
        let mut line = format!("{file:40} {ms:6.0} ms ");
        if let Some(e) = case["python_error"].as_str() {
            line += &format!(" python failed ({e}); rust: {} fiber(s)", found.len());
        }
        if py.len() != found.len() {
            line += &format!(" COUNT python {} rust {}", py.len(), found.len());
        }
        for (p, f) in py.iter().zip(&found) {
            let ps = p["slow"].as_f64().unwrap();
            let d = angle_error(ps, f.slow_deg);
            worst = worst.max(d);
            diffs.push(d);
            line += &format!(" | py {:7.2} {:10} rs {:7.2} {:10} diff {d:5.2} centre {:4.1}px r {:+5.1}",
                             ps, p["type"].as_str().unwrap(), f.slow_deg, f.fiber_type,
                             (p["cx"].as_f64().unwrap() - f.cx).hypot(p["cy"].as_f64().unwrap() - f.cy),
                             f.radius - p["r"].as_f64().unwrap());
            if p["type"].as_str().unwrap() != f.fiber_type || p["clear"].as_bool() != Some(f.clear()) {
                line += " TYPE/CLEAR DIFFERS";
            }
            if let Some(t) = truth {
                py_err.push(angle_error(ps, t));
                rs_err.push(angle_error(f.slow_deg, t));
            }
        }
        println!("{line}");
    }
    diffs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = |v: &mut Vec<f64>| { v.sort_by(|a, b| a.partial_cmp(b).unwrap()); v[v.len() / 2] };
    println!("\nRust vs Python: median diff {:.3} deg, worst {worst:.2} deg over {} fibers", diffs[diffs.len() / 2], diffs.len());
    println!("Error vs truth: python median {:.3}, max {:.2}; rust median {:.3}, max {:.2}",
             med(&mut py_err.clone()), py_err.iter().cloned().fold(0.0, f64::max),
             med(&mut rs_err.clone()), rs_err.iter().cloned().fold(0.0, f64::max));
    println!("Time: python {:.1} s, rust {:.1} s", py_ms / 1000.0, rs_ms / 1000.0);
}
