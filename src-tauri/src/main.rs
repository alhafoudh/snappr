// Prevents a console window opening alongside the app on Windows in release
// builds. Harmless on macOS, and Tauri's own template carries it.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    snappr_lib::run()
}
