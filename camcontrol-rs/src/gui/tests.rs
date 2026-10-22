//! Window tests: the real App driven by egui's test harness (clicks, keys,
//! mouse), with no camera and everything written to a temporary folder
//! (as tests/test_views.py and test_parts.py in the Python app).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Key, Modifiers, PointerButton, Pos2};
use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;
use image::{Rgb, RgbImage};

use super::stripes::PanelId;
use super::{Action, App};
use crate::draw;

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("camcontrol-rs-gui-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Window tests take turns: they set process-wide environment variables.
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct Test {
    h: Harness<'static, App>,
    dir: PathBuf,
    _turn: std::sync::MutexGuard<'static, ()>,
}

impl std::ops::Deref for Test {
    type Target = Harness<'static, App>;
    fn deref(&self) -> &Self::Target {
        &self.h
    }
}

impl std::ops::DerefMut for Test {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.h
    }
}

fn start(tag: &str, fake_camera: bool) -> Test {
    let turn = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = temp_dir(tag);
    // Safety: tests hold LOCK while these are set and read.
    unsafe {
        std::env::set_var("CAMCONTROL_NO_CAMERAS", "1");
        std::env::set_var("CAMCONTROL_TEST_FOLDER", &dir);
        if fake_camera {
            std::env::set_var("CAMCONTROL_FAKE_CAMERA", "1");
        } else {
            std::env::remove_var("CAMCONTROL_FAKE_CAMERA");
        }
    }
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 850.0)).with_max_steps(20).build_eframe(|cc| App::new(cc));
    h.run_ok();
    assert!(h.state().saved.folder.contains("camcontrol-rs-gui"), "tests must write to a temporary folder");
    Test { h, dir, _turn: turn }
}

fn harness(tag: &str) -> (Test, PathBuf) {
    let t = start(tag, false);
    let dir = t.dir.clone();
    (t, dir)
}

fn drawn_panda(path: &PathBuf) {
    let mut img = RgbImage::from_pixel(400, 400, Rgb([30, 30, 30]));
    draw::fill_circle_px(&mut img, 200, 200, 150, [180, 220, 240]);
    for s in [-1i64, 1] {
        draw::fill_circle_px(&mut img, 200 + s * 75, 200, 35, [30, 120, 200]);
    }
    img.save(path).unwrap();
}

/// Run frames until cond holds (background jobs), or fail after a while.
fn wait(h: &mut Test, what: &str, cond: impl Fn(&App) -> bool) {
    let t = Instant::now();
    while !cond(h.state()) {
        assert!(t.elapsed() < Duration::from_secs(60), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
        h.step();
    }
    h.run_ok();
}

fn click_at(h: &mut Test, pos: Pos2, button: PointerButton) {
    let input = h.input_mut();
    input.events.push(egui::Event::PointerMoved(pos));
    input.events.push(egui::Event::PointerButton { pos, button, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.input_mut().events.push(egui::Event::PointerButton { pos, button, pressed: false, modifiers: Modifiers::NONE });
    h.step();
    h.step();
}

fn open(h: &mut Test, path: &PathBuf) {
    let ctx = h.state().ctx.clone();
    let a = h.state().active;
    h.state_mut().perform(&ctx, Action::OpenImage(a, path.clone()));
    h.run_ok();
}

#[test]
fn starts_empty_and_opens_an_image() {
    let (mut h, dir) = harness("open");
    assert_eq!(h.state().panes.len(), 1);
    assert!(h.state().source_text(h.state().active).0.starts_with("Empty"));
    let path = dir.join("panda.png");
    drawn_panda(&path);
    open(&mut h, &path);
    assert_eq!(h.state().source_text(h.state().active).0, "panda.png");
    h.get_by_label("panda.png"); // shown in the status bar
}

#[test]
fn find_axes_from_the_panel() {
    let (mut h, dir) = harness("axes");
    let path = dir.join("panda.png");
    drawn_panda(&path);
    open(&mut h, &path);
    h.press_key_modifiers(Modifiers::COMMAND, Key::Num5); // Fiber axis panel
    h.run_ok();
    assert!(h.state().panels.is_open(PanelId::Fiber));
    h.get_by_label("Find axes").click();
    wait(&mut h, "fiber axes", |a| !a.active_pane().fibers.is_empty());
    let f = &h.state().active_pane().fibers[0];
    assert_eq!(f.fiber_type, "Panda");
    assert!(f.slow_deg.min(180.0 - f.slow_deg) < 1.0, "{}", f.slow_deg);
}

#[test]
fn measuring_a_line_by_clicking() {
    let (mut h, dir) = harness("measure");
    let path = dir.join("panda.png");
    drawn_panda(&path);
    open(&mut h, &path);
    h.get_by_label("Line").click();
    h.run_ok();
    assert_eq!(h.state().tool.as_deref(), Some("line"));
    let view = &h.state().active_pane().view;
    let (a, b) = (view.to_screen(100.0, 200.0), view.to_screen(300.0, 200.0));
    click_at(&mut h, a, PointerButton::Primary);
    click_at(&mut h, b, PointerButton::Primary);
    h.run_ok();
    let ms = &h.state().active_pane().measurements;
    assert_eq!(ms.len(), 1, "a line should be added");
    let len = crate::measure::compute(&ms[0], &crate::calibration::Calibration::pixels()).unwrap().length.unwrap();
    assert!((len - 200.0).abs() < 2.0, "{len}");
    // Esc leaves the tool.
    h.press_key(Key::Escape);
    h.run_ok();
    assert_eq!(h.state().tool, None);
}

#[test]
fn counting_marks_by_clicking() {
    let (mut h, dir) = harness("count");
    let path = dir.join("panda.png");
    drawn_panda(&path);
    open(&mut h, &path);
    h.press_key_modifiers(Modifiers::COMMAND, Key::Num4);
    h.run_ok();
    h.get_by_label("Count").click();
    h.run_ok();
    assert!(h.state().counting);
    let p = h.state().active_pane().view.to_screen(200.0, 200.0);
    click_at(&mut h, p, PointerButton::Primary);
    click_at(&mut h, p + egui::vec2(40.0, 0.0), PointerButton::Primary);
    assert_eq!(h.state().active_pane().counter.marks.len(), 2);
    click_at(&mut h, p, PointerButton::Secondary); // removes the nearest
    assert_eq!(h.state().active_pane().counter.marks.len(), 1);
}

#[test]
fn views_add_layout_and_remove() {
    let (mut h, dir) = harness("views");
    let a = dir.join("a.png");
    drawn_panda(&a);
    h.press_key_modifiers(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::K);
    h.run_ok();
    assert_eq!(h.state().panes.len(), 2);
    assert_ne!(h.state().layout, "single");
    h.state_mut().open_paths_in_new_views(&[a.clone(), a.clone()]);
    h.run_ok();
    assert_eq!(h.state().panes.len(), 4);
    h.get_by_label("Side by side").click();
    h.run_ok();
    assert_eq!(h.state().layout, "row");
    let last = h.state().panes[3].id;
    h.state_mut().remove_view(last, false);
    h.run_ok();
    assert_eq!(h.state().panes.len(), 3);
    h.press_key_modifiers(Modifiers::COMMAND, Key::K); // one view
    h.run_ok();
    assert_eq!(h.state().layout, "single");
}

#[test]
fn shortcuts_toggle_overlays() {
    let (mut h, _dir) = harness("keys");
    let (g, c) = (h.state().saved.grid, h.state().saved.crosshair);
    h.press_key(Key::G);
    h.press_key(Key::C);
    h.run_ok();
    assert_eq!((h.state().saved.grid, h.state().saved.crosshair), (!g, !c));
}

#[test]
fn processing_shows_an_unsaved_result() {
    let (mut h, dir) = harness("process");
    let (a, b) = (dir.join("a.png"), dir.join("b.png"));
    drawn_panda(&a);
    drawn_panda(&b);
    h.state_mut().start_process("stack", vec![a, b], serde_json::json!({"align": false}));
    wait(&mut h, "focus stack", |app| app.active_pane().result.is_some());
    let text = h.state().source_text(h.state().active).0;
    assert!(text.contains("Focus stack of 2 images (unsaved)"), "{text}");
}

#[test]
fn settings_file_round_trip() {
    let (mut h, dir) = harness("settings");
    let app = h.state_mut();
    app.saved.name = "sample".into();
    app.saved.average_idx = 2;
    app.saved.grid = true;
    app.saved.grid_style.columns = 12;
    let data = app.current_settings();
    assert_eq!(data["capture"]["average"], 8);
    app.saved.name.clear();
    app.saved.average_idx = 0;
    app.saved.grid = false;
    app.saved.grid_style.columns = 8;
    let ctx = app.ctx.clone();
    let path = dir.join("s.json");
    super::settings::save_settings_file(&path, &data).unwrap();
    app.open_settings_path(&ctx, &path, false);
    assert_eq!((app.saved.name.as_str(), app.saved.average_idx, app.saved.grid, app.saved.grid_style.columns), ("sample", 2, true, 12));
}

#[test]
fn opening_another_image_asks_about_measurements() {
    let (mut h, dir) = harness("ask");
    let path = dir.join("p.png");
    drawn_panda(&path);
    open(&mut h, &path);
    let ctx = h.state().ctx.clone();
    let id = h.state().active;
    h.state_mut().view_event(id, super::view::ViewEvent::Drawn("line".into(), vec![(0.0, 0.0), (3.0, 4.0)]));
    let p2 = path.clone();
    h.state_mut().open_image_path(&ctx, id, p2);
    h.run_ok();
    assert!(h.state().modal_open());
    h.get_by_label("Yes").click();
    h.run_ok();
    assert!(h.state().active_pane().measurements.is_empty());
    let _ = Arc::new(());
}

// --- with the fake camera -------------------------------------------------------------------

fn files(dir: &PathBuf, ext: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == ext)).collect();
    v.sort();
    v
}

fn live(tag: &str) -> Test {
    let mut t = start(tag, true);
    // First run: the first camera opens in the first view, live.
    wait(&mut t, "the fake camera", |a| a.link().is_some_and(|l| l.open) && a.active_pane().image().is_some());
    assert!(t.state().active_pane().live);
    t
}

#[test]
fn camera_opens_live_and_captures() {
    let mut t = live("capture");
    assert_eq!(t.state().source_text(t.state().active).0, "Fake camera · Live");
    t.state_mut().saved.name = "shot".into();
    t.state_mut().saved.average_idx = 1; // 4 frames
    t.press_key(Key::Space);
    let dir = t.dir.clone();
    wait(&mut t, "the capture", |a| a.capturing.is_empty() && dir.join("shot-001.tif").is_file());
    let img = crate::capture::load_image(&dir.join("shot-001.tif")).unwrap();
    assert_eq!(img.dimensions(), (640, 480));
    let meta: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("shot-001.json")).unwrap()).unwrap();
    assert_eq!(meta["frames_averaged"], 4);
    assert_eq!(meta["exposure_value"], -6.0);
}

#[test]
fn exposure_slider_reaches_the_camera() {
    let mut t = live("slider");
    if let Some(l) = t.state_mut().link_mut() {
        l.send(crate::camera::worker::Cmd::Control("exposure".into(), -4.0));
    }
    wait(&mut t, "the new exposure", |a| a.link().and_then(|l| l.values.get("exposure").copied()) == Some(-4));
    // Past the camera's longest exposure: frames are added (2 here).
    if let Some(l) = t.state_mut().link_mut() {
        l.send(crate::camera::worker::Cmd::Control("exposure".into(), -2.0));
    }
    wait(&mut t, "a long exposure", |a| a.link().and_then(|l| l.values.get("exposure").copied()) == Some(-2));
    let r = t.state().link().unwrap().ranges["exposure"].unwrap();
    assert_eq!((r.min, r.max), (-13, 1));
}

#[test]
fn records_a_video() {
    let mut t = live("video");
    t.state_mut().saved.recording.video_format = "AVI (higher quality, large)".into();
    t.state_mut().request_recording(true);
    wait(&mut t, "recording to start", |a| a.link().is_some_and(|l| l.recording));
    std::thread::sleep(Duration::from_millis(1200));
    t.state_mut().request_recording(false);
    wait(&mut t, "recording to stop", |a| a.link().is_some_and(|l| !l.recording));
    let avi = files(&t.dir, "avi");
    assert_eq!(avi.len(), 1, "{avi:?}");
    let r = crate::video::VideoReader::open(&avi[0]).unwrap();
    assert!((20..=45).contains(&r.frame_count), "{} frames", r.frame_count);
}

#[test]
fn records_the_fiber_axes_on_the_video() {
    let mut t = live("videoaxes");
    t.state_mut().saved.recording.video_format = "AVI (higher quality, large)".into();
    assert!(t.state().saved.recording.draw_analysis, "on by default");
    let axis = crate::processing::fiber_axis::FiberAxis {
        cx: 320.0, cy: 240.0, radius: 120.0, slow_deg: 5.0, elongation: 0.5, fiber_type: "Panda".into(), n_parts: 2,
        method: "rods".into(), axis_x: None, axis_y: None, parts: Vec::new(), part_centres: Vec::new(), threshold: 0.0,
        quality: Some(3.0), coverage: Some(0.9), rod_radii: Vec::new(), turned_gain: None,
    };
    t.state_mut().active_pane_mut().fibers = vec![axis];
    t.state_mut().request_recording(true);
    wait(&mut t, "recording to start", |a| a.link().is_some_and(|l| l.recording && l.overlay_sent.is_some()));
    std::thread::sleep(Duration::from_millis(800));
    t.state_mut().request_recording(false);
    wait(&mut t, "recording to stop", |a| a.link().is_some_and(|l| !l.recording));
    let avi = files(&t.dir, "avi");
    let mut r = crate::video::VideoReader::open(&avi[0]).unwrap();
    let last = r.frame(r.frame_count - 1).unwrap();
    // The green readout in the top-left corner (JPEG frames, so roughly green).
    let green = (0..60).flat_map(|y| (0..300).map(move |x| (x, y)))
        .filter(|&(x, y)| { let p = last.get_pixel(x, y).0; p[1] > 120 && p[1] > p[0] + 60 && p[1] > p[2] + 60 })
        .count();
    assert!(green > 50, "{green} green pixels");
}

#[test]
fn exposure_set_as_a_time() {
    let mut t = live("exposurems");
    let send = |t: &mut Test, us: f64| {
        if let Some(l) = t.state_mut().link_mut() {
            l.send(crate::camera::worker::Cmd::Control(crate::camera::EXPOSURE_US.into(), us));
        }
    };
    let get = |a: &App, k: &str| a.link().and_then(|l| l.values.get(k).copied());
    // 900 ms: between the camera's steps (625 ms and 1.25 s).
    send(&mut t, 900_000.0);
    wait(&mut t, "900 ms", |a| get(a, "exposure_us") == Some(900_000));
    assert_eq!(get(t.state(), "exposure"), Some(-4));
    // 3 s: past the longest step, 3 frames added.
    send(&mut t, 3_000_000.0);
    wait(&mut t, "3 s", |a| get(a, "exposure_us") == Some(3_000_000));
    // A whole step again resets the brightening.
    if let Some(l) = t.state_mut().link_mut() {
        l.send(crate::camera::worker::Cmd::Control("exposure".into(), -5.0));
    }
    wait(&mut t, "step -5", |a| get(a, "exposure_us") == Some(312_500));
}

#[test]
fn time_lapse_with_video() {
    let mut t = live("timelapse");
    let r = &mut t.state_mut().saved.recording;
    (r.tl_interval, r.tl_unit, r.tl_count, r.tl_video, r.tl_video_fps) = (0.5, 0, 3, true, 5);
    t.state_mut().saved.name = "tl".into();
    t.state_mut().request_timelapse(true);
    wait(&mut t, "the time-lapse", |a| a.link().is_some_and(|l| !l.timelapse) && a.tl_status.contains("finished"));
    assert_eq!(files(&t.dir, "tif").len(), 3);
    let mp4 = files(&t.dir, "mp4");
    assert_eq!(mp4.len(), 1, "{}", t.state().tl_status);
    assert_eq!(crate::video::VideoReader::open(&mp4[0]).unwrap().frame_count, 3);
}

#[test]
fn live_fiber_axes_and_moving_the_camera() {
    let mut t = live("fiberlive");
    t.state_mut().fiber.live = true;
    wait(&mut t, "live fiber axes", |a| !a.active_pane().fibers.is_empty());
    assert_eq!(t.state().active_pane().fibers[0].fiber_type, "Panda");
    // A second view takes the camera; the first keeps its last frame, frozen.
    let first = t.state().active;
    let key = t.state().active_pane().camera.as_ref().unwrap().key.clone();
    let second = t.state_mut().add_view().unwrap();
    t.state_mut().set_pane_camera(second, Some(key), false);
    t.run_ok();
    assert!(t.state().pane(first).camera.is_none() && !t.state().pane(first).live);
    assert!(t.state().pane(second).live);
    wait(&mut t, "frames in the new view", |a| a.pane(second).image().is_some());
}

/// Pictures of the window, rendered offscreen (no screen capture), for checking
/// the layout by eye: `CAMCONTROL_SHOTS=<dir> cargo test --release --lib render_shots -- --ignored`
#[test]
#[ignore]
fn render_shots() {
    let Some(out) = std::env::var_os("CAMCONTROL_SHOTS").map(PathBuf::from) else { return };
    std::fs::create_dir_all(&out).unwrap();
    let turn = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = temp_dir("shots");
    unsafe {
        std::env::set_var("CAMCONTROL_NO_CAMERAS", "1");
        std::env::set_var("CAMCONTROL_TEST_FOLDER", &dir);
        std::env::set_var("CAMCONTROL_FAKE_CAMERA", "1");
    }
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 850.0)).with_max_steps(20).wgpu().build_eframe(|cc| App::new(cc));
    let mut t = Test { h: { h.run_ok(); h }, dir: dir.clone(), _turn: turn };
    wait(&mut t, "the fake camera", |a| a.active_pane().image().is_some());
    let shot = |t: &mut Test, name: &str| {
        for _ in 0..3 {
            t.step();
        }
        t.render().unwrap().save(out.join(format!("{name}.png"))).unwrap();
    };
    shot(&mut t, "1_live");
    let fibers = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../Fiber Types.png");
    if fibers.is_file() {
        let id = t.state_mut().add_view().unwrap();
        let ctx = t.state().ctx.clone();
        t.state_mut().perform(&ctx, Action::OpenImage(id, fibers));
        t.state_mut().panels.set_open(PanelId::Fiber, true);
        t.state_mut().find_fiber_axes(false);
        wait(&mut t, "fiber axes", |a| a.panes.iter().all(|p| !p.fibers.is_empty()));
        shot(&mut t, "2_views_fiber");
    }
    t.state_mut().panels.set_open(PanelId::Counting, true);
    t.state_mut().saved.capture_tab = 2;
    shot(&mut t, "3_counting_timelapse");
    t.state_mut().panels.set_open(PanelId::Measurements, true);
    t.state_mut().saved.capture_tab = 1;
    shot(&mut t, "4_measure_video");
}
