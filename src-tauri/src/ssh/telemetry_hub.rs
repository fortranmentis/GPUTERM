//! Everything that consumes a telemetry reading in the backend, called once
//! per reading from `system_monitor::emit_telemetry`. Running here rather
//! than in the webview means it works whether or not any window is open.

use tauri::AppHandle;

use crate::alerts;

use super::session::AppState;
use super::system_monitor::{now_epoch_seconds, RemoteTelemetry};

pub fn observe(app: &AppHandle, state: &AppState, telemetry: &RemoteTelemetry) {
    let session_id = telemetry.session_id();
    // A monitor thread that was just stopped can deliver one last reading
    // after its session is torn down; it must not bring the history back.
    let host_label = state
        .active_connections
        .lock()
        .ok()
        .and_then(|connections| connections.get(session_id).map(|c| c.profile.name.clone()));
    let Some(host_label) = host_label else {
        return;
    };
    let ignored_fs = state
        .telemetry_settings
        .lock()
        .map(|settings| settings.disk_ignore_fs_types.clone())
        .unwrap_or_default();
    let now = now_epoch_seconds();
    if let Some(sample) = telemetry.history_sample(&ignored_fs) {
        state.telemetry_history.record(session_id, now, sample);
    }
    if let Some(input) = telemetry.alert_input(&ignored_fs) {
        let events = state
            .alerts
            .evaluate_telemetry(session_id, &host_label, &input, now);
        alerts::deliver(app, events);
    }
}
