//! GpuTerm as an MCP server. The running app listens on a loopback port
//! (`server`); MCP clients start `gputerm --mcp-stdio` (`relay`), which
//! forwards their stdio to that port. Tools read the app's own state, so
//! they see exactly the hosts the user has connected.

pub mod approval;
pub mod audit;
pub mod protocol;
pub mod relay;
pub mod server;
pub mod tools;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use crate::ssh::local_fs::{modify_app_settings, read_app_settings};
use crate::ssh::session::config_dir;

/// Off by default; set from the MCP dialog.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct McpSettings {
    pub enabled: bool,
    /// Offer run_command, read_file and kill_process (each still asks).
    pub allow_actions: bool,
    /// Profiles MCP clients must not see.
    pub hidden_profiles: Vec<String>,
}

#[derive(Default)]
pub struct McpState {
    pub settings: Arc<Mutex<McpSettings>>,
    server: Mutex<Option<server::ServerHandle>>,
    pub approvals: approval::PendingApprovals,
    /// Connected clients, by connection number.
    pub clients: Arc<Mutex<BTreeMap<u64, String>>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpStatus {
    pub settings: McpSettings,
    pub running: bool,
    pub port: Option<u16>,
    pub clients: Vec<String>,
    /// What MCP clients should launch, with `--mcp-stdio`.
    pub executable: Option<String>,
    pub audit_log: String,
    pub error: Option<String>,
}

/// Where the running server publishes its port and token for the relay.
pub fn endpoint_path() -> PathBuf {
    config_dir().join("mcp-endpoint.json")
}

pub fn current_settings(app: &AppHandle) -> McpSettings {
    app.state::<McpState>()
        .settings
        .lock()
        .map(|settings| settings.clone())
        .unwrap_or_default()
}

/// Loads the saved settings and starts the server if it was on.
pub fn restore(app: &AppHandle) {
    let settings = read_app_settings()
        .ok()
        .and_then(|settings| settings.mcp)
        .unwrap_or_default();
    let state = app.state::<McpState>();
    if let Ok(mut slot) = state.settings.lock() {
        *slot = settings.clone();
    }
    // A file left by a crash would point the relay at a dead port.
    let _ = std::fs::remove_file(endpoint_path());
    if settings.enabled {
        if let Err(error) = start(app) {
            eprintln!("GpuTerm MCP server failed to start: {error}");
        }
    }
}

fn start(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<McpState>();
    let mut slot = state
        .server
        .lock()
        .map_err(|_| "MCP server state is unavailable".to_string())?;
    if slot.is_none() {
        *slot = Some(server::start(app.clone())?);
    }
    Ok(())
}

/// Stops the server, if running, and removes its endpoint file.
pub fn stop(app: &AppHandle) {
    let state = app.state::<McpState>();
    let handle = state.server.lock().ok().and_then(|mut slot| slot.take());
    if let Some(handle) = handle {
        handle.stop();
    }
}

fn status(app: &AppHandle, error: Option<String>) -> McpStatus {
    let state = app.state::<McpState>();
    let port = state
        .server
        .lock()
        .ok()
        .and_then(|slot| slot.as_ref().map(|handle| handle.port));
    McpStatus {
        settings: current_settings(app),
        running: port.is_some(),
        port,
        clients: state
            .clients
            .lock()
            .map(|clients| clients.values().cloned().collect())
            .unwrap_or_default(),
        executable: std::env::current_exe()
            .ok()
            .map(|path| path.to_string_lossy().into_owned()),
        audit_log: audit::log_path().to_string_lossy().into_owned(),
        error,
    }
}

#[tauri::command]
pub fn get_mcp_status(app: AppHandle) -> McpStatus {
    status(&app, None)
}

/// Saves the settings and starts or stops the server to match.
#[tauri::command]
pub fn update_mcp_settings(
    app: AppHandle,
    state: State<'_, McpState>,
    settings: McpSettings,
) -> Result<McpStatus, String> {
    modify_app_settings(|saved| saved.mcp = Some(settings.clone()))?;
    if let Ok(mut slot) = state.settings.lock() {
        *slot = settings.clone();
    }
    let error = if settings.enabled {
        start(&app).err()
    } else {
        stop(&app);
        None
    };
    Ok(status(&app, error))
}
