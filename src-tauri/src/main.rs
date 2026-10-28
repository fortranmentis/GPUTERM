// Tauri is a GUI application. Without this release-only subsystem attribute,
// Windows opens an attached console whose lifetime also controls the app.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // MCP clients launch the same executable to reach the running app.
    if std::env::args().nth(1).as_deref() == Some("--mcp-stdio") {
        std::process::exit(gputerm_lib::run_mcp_relay());
    }
    gputerm_lib::run();
}
