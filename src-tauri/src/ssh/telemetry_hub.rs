//! Everything that consumes a telemetry reading in the backend, called once
//! per reading from `system_monitor::emit_telemetry`. Running here rather
//! than in the webview means it works whether or not any window is open.

use tauri::AppHandle;

use super::session::AppState;
use super::system_monitor::{now_epoch_seconds, RemoteTelemetry};

pub fn observe(_app: &AppHandle, state: &AppState, telemetry: &RemoteTelemetry) {
    let session_id = telemetry.session_id();
    // A monitor thread that was just stopped can deliver one last reading
    // after its session is torn down; it must not bring the history back.
    let live = state
        .active_connections
        .lock()
        .map(|connections| connections.contains_key(session_id))
        .unwrap_or(false);
    if !live {
        return;
    }
    let ignored_fs = state
        .telemetry_settings
        .lock()
        .map(|settings| settings.disk_ignore_fs_types.clone())
        .unwrap_or_default();
    if let Some(sample) = telemetry.history_sample(&ignored_fs) {
        state
            .telemetry_history
            .record(session_id, now_epoch_seconds(), sample);
    }
}
