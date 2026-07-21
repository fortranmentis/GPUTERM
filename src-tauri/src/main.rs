// Tauri is a GUI application. Without this release-only subsystem attribute,
// Windows opens an attached console whose lifetime also controls the app.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    gputerm_lib::run();
}
