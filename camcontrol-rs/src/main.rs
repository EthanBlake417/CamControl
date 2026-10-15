#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // no console window in release builds

fn main() -> eframe::Result {
    camcontrol::run()
}
