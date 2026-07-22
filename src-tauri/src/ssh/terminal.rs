use crate::ssh::credentials::CredentialStore;
use crate::ssh::session::{open_ssh_session, SshConnection};
use crate::ssh::session::{
    profile_from_request, target_for_active_session, target_from_request, upsert_profile,
    ActiveConnection, AppState, OpsSessions, SessionConnectRequest, SshTarget, TerminalSessionInfo,
};
use crate::ssh::system_monitor;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use ssh2::{Channel, ExtendedData, Session};
use std::collections::HashMap;
use std::env;
use std::io::{ErrorKind, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, State};

pub struct TerminalHandle {
    pub session_id: String,
    backend: TerminalBackend,
    pub stop: Arc<AtomicBool>,
}

enum TerminalBackend {
    Remote {
        // Holds the connection (and any jump-host tunnel) alive for the session.
        connection: SshConnection,
        channel: Arc<Mutex<Channel>>,
    },
    Local {
        master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
        writer: Arc<Mutex<Box<dyn Write + Send>>>,
        child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    },
}

enum TerminalWriter {
    Remote(Arc<Mutex<Channel>>),
    Local(Arc<Mutex<Box<dyn Write + Send>>>),
}

enum TerminalResizer {
    Remote(Arc<Mutex<Channel>>),
    Local(Arc<Mutex<Box<dyn MasterPty + Send>>>),
}

struct OpenLocalTerminal {
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    reader: Box<dyn Read + Send>,
}

#[derive(Clone)]
struct TerminalReaderState {
    terminals: Arc<Mutex<HashMap<String, TerminalHandle>>>,
    active_connections: Arc<Mutex<HashMap<String, ActiveConnection>>>,
    telemetry_stops: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    ops_sessions: OpsSessions,
    remote_os_cache: Arc<Mutex<HashMap<String, system_monitor::RemoteOs>>>,
}

impl TerminalReaderState {
    fn from_app_state(state: &AppState) -> Self {
        Self {
            terminals: Arc::clone(&state.terminals),
            active_connections: Arc::clone(&state.active_connections),
            telemetry_stops: Arc::clone(&state.telemetry_stops),
            ops_sessions: Arc::clone(&state.ops_sessions),
            remote_os_cache: Arc::clone(&state.remote_os_cache),
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TerminalOutputPayload {
    session_id: String,
    terminal_id: String,
    data: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TerminalClosedPayload {
    session_id: String,
    terminal_id: String,
    session_closed: bool,
    message: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalPaneInfo {
    session_id: String,
    terminal_id: String,
}

#[tauri::command]
pub async fn connect_terminal(
    app: AppHandle,
    state: State<'_, AppState>,
    request: SessionConnectRequest,
) -> Result<TerminalSessionInfo, String> {
    let profile = profile_from_request(&request);
    let cols = request.cols.unwrap_or(120).max(1);
    let rows = request.rows.unwrap_or(32).max(1);

    if profile.is_local {
        return connect_local_terminal(app, &state, profile, cols, rows).await;
    }

    let target = target_from_request(&profile, &request)?;
    let credentials = state.credentials.clone();
    let reuse_stored_credentials = request.reuse_stored_credentials;
    let clear_missing_credentials = request.id.is_some() && !reuse_stored_credentials;
    let opened = tauri::async_runtime::spawn_blocking(move || {
        let mut target = target;
        let mut credential_warnings = Vec::new();
        if reuse_stored_credentials {
            fill_stored_credentials(&mut target, &credentials, &mut credential_warnings);
        }
        let (connection, channel) =
            open_terminal_channel(&target, cols, rows).map_err(|error| {
                if credential_warnings.is_empty() {
                    error
                } else {
                    format!(
                        "{} Saved credentials were unavailable: {}",
                        error,
                        credential_warnings.join(" ")
                    )
                }
            })?;
        Ok::<_, String>((connection, channel, target, credential_warnings))
    })
    .await
    .map_err(|error| format!("Terminal connect task failed: {}", error))?;

    let (connection, channel, target, mut credential_warnings) = match opened {
        Ok(pair) => pair,
        Err(error) => return Err(error),
    };

    // Only persist credentials after authentication succeeds. A local vault
    // write failure must not tear down a working terminal, so return it as a
    // warning while retaining the password in the in-memory write-through cache.
    let credentials = state.credentials.clone();
    let credential_target = target.clone();
    let storage_warning = match tauri::async_runtime::spawn_blocking(move || {
        sync_target_credentials(&credential_target, &credentials, clear_missing_credentials)
    })
    .await
    {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(error),
        Err(error) => Some(format!("Credential storage task failed: {}", error)),
    };
    if let Some(warning) = storage_warning {
        credential_warnings.push(warning);
    }
    let credential_warning =
        (!credential_warnings.is_empty()).then(|| credential_warnings.join(" "));

    // Persist before replacing any live pane so a storage failure cannot
    // leave an untracked SSH channel behind.
    upsert_profile(profile.clone())?;
    stop_existing_session(&state, &profile.id);

    let terminal_id = uuid::Uuid::new_v4().to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let channel = Arc::new(Mutex::new(channel));
    let reader_session = connection.session.clone();
    let handle = TerminalHandle {
        session_id: profile.id.clone(),
        backend: TerminalBackend::Remote {
            connection,
            channel: Arc::clone(&channel),
        },
        stop: Arc::clone(&stop),
    };

    // Register the replacement terminal before the active profile. A reader
    // from the previous connection can then never mistake the replacement
    // handshake for a session with no remaining terminal panes.
    {
        let mut terminals = state
            .terminals
            .lock()
            .map_err(|_| "Terminal state is unavailable".to_string())?;
        terminals.insert(terminal_id.clone(), handle);
    }

    {
        let mut active = state
            .active_connections
            .lock()
            .map_err(|_| "Active session state is unavailable".to_string())?;
        active.insert(
            profile.id.clone(),
            ActiveConnection {
                profile: profile.clone(),
            },
        );
    }

    let reader_state = TerminalReaderState::from_app_state(&state);
    start_remote_terminal_reader(
        app.clone(),
        profile.id.clone(),
        terminal_id.clone(),
        reader_session,
        channel,
        stop,
        reader_state,
    );
    start_system_monitor(app, &state, target);

    Ok(TerminalSessionInfo {
        session_id: profile.id.clone(),
        terminal_id,
        profile,
        credential_warning,
    })
}

async fn connect_local_terminal(
    app: AppHandle,
    state: &AppState,
    profile: crate::ssh::session::SessionProfile,
    cols: u32,
    rows: u32,
) -> Result<TerminalSessionInfo, String> {
    let opened = tauri::async_runtime::spawn_blocking(move || open_local_terminal(cols, rows))
        .await
        .map_err(|error| format!("Local terminal task failed: {}", error))??;

    upsert_profile(profile.clone())?;
    stop_existing_session(state, &profile.id);

    let terminal_id = uuid::Uuid::new_v4().to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let OpenLocalTerminal {
        master,
        writer,
        child,
        reader,
    } = opened;
    let handle = TerminalHandle {
        session_id: profile.id.clone(),
        backend: TerminalBackend::Local {
            master,
            writer,
            child,
        },
        stop: Arc::clone(&stop),
    };

    {
        let mut terminals = state
            .terminals
            .lock()
            .map_err(|_| "Terminal state is unavailable".to_string())?;
        terminals.insert(terminal_id.clone(), handle);
    }
    {
        let mut active = state
            .active_connections
            .lock()
            .map_err(|_| "Active session state is unavailable".to_string())?;
        active.insert(
            profile.id.clone(),
            ActiveConnection {
                profile: profile.clone(),
            },
        );
    }

    start_local_terminal_reader(
        app.clone(),
        profile.id.clone(),
        terminal_id.clone(),
        reader,
        stop,
        TerminalReaderState::from_app_state(state),
    );
    start_local_system_monitor(app, state, profile.id.clone());

    Ok(TerminalSessionInfo {
        session_id: profile.id.clone(),
        terminal_id,
        profile,
        credential_warning: None,
    })
}

#[tauri::command]
pub async fn create_terminal_split(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: String,
    cols: u32,
    rows: u32,
) -> Result<TerminalPaneInfo, String> {
    let is_local = {
        let active = state
            .active_connections
            .lock()
            .map_err(|_| "Active session state is unavailable".to_string())?;
        active
            .get(&session_id)
            .map(|connection| connection.profile.is_local)
            .ok_or_else(|| "No active terminal session is available".to_string())?
    };

    if is_local {
        let opened = tauri::async_runtime::spawn_blocking(move || {
            open_local_terminal(cols.max(1), rows.max(1))
        })
        .await
        .map_err(|error| format!("Local terminal split task failed: {}", error))??;
        let terminal_id = uuid::Uuid::new_v4().to_string();
        let stop = Arc::new(AtomicBool::new(false));
        let OpenLocalTerminal {
            master,
            writer,
            child,
            reader,
        } = opened;

        {
            let mut terminals = state
                .terminals
                .lock()
                .map_err(|_| "Terminal state is unavailable".to_string())?;
            terminals.insert(
                terminal_id.clone(),
                TerminalHandle {
                    session_id: session_id.clone(),
                    backend: TerminalBackend::Local {
                        master,
                        writer,
                        child,
                    },
                    stop: Arc::clone(&stop),
                },
            );
        }
        start_local_terminal_reader(
            app,
            session_id.clone(),
            terminal_id.clone(),
            reader,
            stop,
            TerminalReaderState::from_app_state(&state),
        );
        return Ok(TerminalPaneInfo {
            session_id,
            terminal_id,
        });
    }

    let target = target_for_active_session(&state, &session_id)?;
    let connect_target = target.clone();
    let opened = tauri::async_runtime::spawn_blocking(move || {
        open_terminal_channel(&connect_target, cols.max(1), rows.max(1))
    })
    .await
    .map_err(|error| format!("Terminal split task failed: {}", error))??;
    let (connection, channel) = opened;
    let terminal_id = uuid::Uuid::new_v4().to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let channel = Arc::new(Mutex::new(channel));
    let reader_session = connection.session.clone();

    {
        let mut terminals = state
            .terminals
            .lock()
            .map_err(|_| "Terminal state is unavailable".to_string())?;
        terminals.insert(
            terminal_id.clone(),
            TerminalHandle {
                session_id: session_id.clone(),
                backend: TerminalBackend::Remote {
                    connection,
                    channel: Arc::clone(&channel),
                },
                stop: Arc::clone(&stop),
            },
        );
    }

    let reader_state = TerminalReaderState::from_app_state(&state);
    start_remote_terminal_reader(
        app,
        session_id.clone(),
        terminal_id.clone(),
        reader_session,
        channel,
        stop,
        reader_state,
    );

    Ok(TerminalPaneInfo {
        session_id,
        terminal_id,
    })
}

#[tauri::command]
pub fn terminal_write(
    state: State<AppState>,
    terminal_id: String,
    data: String,
) -> Result<(), String> {
    let (writer, stop) = {
        let terminals = state
            .terminals
            .lock()
            .map_err(|_| "Terminal state is unavailable".to_string())?;
        let Some(handle) = terminals.get(&terminal_id) else {
            // Input can already be queued in the webview when a shell
            // closes. Treat that normal lifecycle race as a no-op.
            return Ok(());
        };
        let writer = match &handle.backend {
            TerminalBackend::Remote { channel, .. } => TerminalWriter::Remote(Arc::clone(channel)),
            TerminalBackend::Local { writer, .. } => TerminalWriter::Local(Arc::clone(writer)),
        };
        (writer, Arc::clone(&handle.stop))
    };
    if stop.load(Ordering::SeqCst) {
        return Ok(());
    }

    match writer {
        TerminalWriter::Remote(channel) => {
            let mut channel = channel
                .lock()
                .map_err(|_| "Terminal channel is unavailable".to_string())?;
            if channel.eof() {
                return Ok(());
            }
            match write_all_nonblocking(&mut *channel, data.as_bytes()) {
                Ok(()) => Ok(()),
                Err(error) if is_closed_channel_error(&error) => Ok(()),
                Err(error) => Err(format!("Failed to write to remote terminal: {}", error)),
            }
        }
        TerminalWriter::Local(writer) => {
            let mut writer = writer
                .lock()
                .map_err(|_| "Local terminal input is unavailable".to_string())?;
            writer
                .write_all(data.as_bytes())
                .and_then(|_| writer.flush())
                .map_err(|error| format!("Failed to write to local terminal: {}", error))
        }
    }
}

#[tauri::command]
pub fn terminal_resize(
    state: State<AppState>,
    terminal_id: String,
    cols: u32,
    rows: u32,
) -> Result<(), String> {
    let resizer = {
        let terminals = state
            .terminals
            .lock()
            .map_err(|_| "Terminal state is unavailable".to_string())?;
        let Some(handle) = terminals.get(&terminal_id) else {
            return Ok(());
        };
        match &handle.backend {
            TerminalBackend::Remote { channel, .. } => TerminalResizer::Remote(Arc::clone(channel)),
            TerminalBackend::Local { master, .. } => TerminalResizer::Local(Arc::clone(master)),
        }
    };

    match resizer {
        TerminalResizer::Remote(channel) => {
            let mut channel = channel
                .lock()
                .map_err(|_| "Terminal channel is unavailable".to_string())?;
            match resize_remote_pty_nonblocking(&mut channel, cols.max(1), rows.max(1)) {
                Ok(()) => Ok(()),
                Err(error) if error.to_string().to_ascii_lowercase().contains("closed") => Ok(()),
                Err(error) => Err(format!("Failed to resize remote PTY: {}", error)),
            }
        }
        TerminalResizer::Local(master) => master
            .lock()
            .map_err(|_| "Local PTY is unavailable".to_string())?
            .resize(pty_size(cols, rows))
            .map_err(|error| format!("Failed to resize local PTY: {}", error)),
    }
}

#[tauri::command]
pub fn disconnect_terminal_pane(state: State<AppState>, terminal_id: String) -> Result<(), String> {
    let handle = {
        let mut terminals = state
            .terminals
            .lock()
            .map_err(|_| "Terminal state is unavailable".to_string())?;
        terminals.remove(&terminal_id)
    };
    if let Some(handle) = handle {
        stop_terminal_handle(handle);
    }
    Ok(())
}

#[tauri::command]
pub fn disconnect_terminal(state: State<AppState>, session_id: String) -> Result<(), String> {
    stop_existing_session(&state, &session_id);
    // Passwords remain only in memory until GpuTerm exits so a disconnected
    // saved profile can reconnect on double-click without writing secrets to disk.
    if let Ok(mut active) = state.active_connections.lock() {
        active.remove(&session_id);
    }
    Ok(())
}

const KEEPALIVE_INTERVAL_SECS: u32 = 30;
const MIN_IDLE_SLEEP: Duration = Duration::from_millis(2);
const MAX_IDLE_SLEEP: Duration = Duration::from_millis(30);
const NONBLOCKING_RETRY_SLEEP: Duration = Duration::from_millis(8);
const NONBLOCKING_OPERATION_TIMEOUT: Duration = Duration::from_secs(5);
const LIBSSH2_ERROR_EAGAIN: i32 = -37;

/// Appends `bytes` to `pending`, drains everything decodable into a String
/// (invalid sequences become U+FFFD), and leaves at most 3 trailing bytes of a
/// possibly-incomplete UTF-8 sequence in `pending`.
fn drain_utf8_stream(pending: &mut Vec<u8>, bytes: &[u8]) -> String {
    pending.extend_from_slice(bytes);
    let mut output = String::new();
    let mut cursor = 0;
    loop {
        match std::str::from_utf8(&pending[cursor..]) {
            Ok(valid) => {
                output.push_str(valid);
                pending.clear();
                return output;
            }
            Err(error) => {
                let valid_up_to = error.valid_up_to();
                output.push_str(
                    std::str::from_utf8(&pending[cursor..cursor + valid_up_to])
                        .expect("valid_up_to marks a valid UTF-8 prefix"),
                );
                cursor += valid_up_to;
                match error.error_len() {
                    Some(invalid_len) => {
                        output.push('\u{FFFD}');
                        cursor += invalid_len;
                    }
                    None => {
                        // Incomplete trailing sequence: keep it for the next chunk.
                        let remainder = pending.split_off(cursor);
                        *pending = remainder;
                        return output;
                    }
                }
            }
        }
    }
}

fn start_remote_terminal_reader(
    app: AppHandle,
    session_id: String,
    terminal_id: String,
    session: Session,
    channel: Arc<Mutex<Channel>>,
    stop: Arc<AtomicBool>,
    reader_state: TerminalReaderState,
) {
    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        let mut pending = Vec::new();
        let mut close_message = None;
        let mut idle_sleep = MIN_IDLE_SLEEP;
        let mut last_keepalive = Instant::now();

        while !stop.load(Ordering::SeqCst) {
            let read_result = {
                let mut channel = match channel.lock() {
                    Ok(channel) => channel,
                    Err(_) => {
                        close_message = Some("Terminal channel is unavailable".to_string());
                        break;
                    }
                };
                let result = channel.read(&mut buffer);
                if matches!(result, Ok(0)) && channel.eof() {
                    close_message = Some("Remote shell closed".to_string());
                    break;
                }
                result
            };

            let idle = match read_result {
                Ok(bytes_read) if bytes_read > 0 => {
                    idle_sleep = MIN_IDLE_SLEEP;
                    let data = drain_utf8_stream(&mut pending, &buffer[..bytes_read]);
                    if !data.is_empty() {
                        let _ = app.emit(
                            "terminal-output",
                            TerminalOutputPayload {
                                session_id: session_id.clone(),
                                terminal_id: terminal_id.clone(),
                                data,
                            },
                        );
                    }
                    false
                }
                Ok(_) => true,
                Err(error)
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) =>
                {
                    true
                }
                Err(error) => {
                    close_message = Some(format!("Terminal stream failed: {}", error));
                    break;
                }
            };

            if idle {
                if last_keepalive.elapsed() >= Duration::from_secs(KEEPALIVE_INTERVAL_SECS.into()) {
                    // A libssh2 send that returns EAGAIN must be retried with
                    // the same operation before another packet is submitted.
                    // Hold the channel mutex so terminal input/resize cannot
                    // interleave with a partially sent keepalive packet.
                    let keepalive_result = {
                        let _channel_guard = match channel.lock() {
                            Ok(channel) => channel,
                            Err(_) => {
                                close_message = Some("Terminal channel is unavailable".to_string());
                                break;
                            }
                        };
                        send_keepalive_nonblocking(&session)
                    };
                    if let Err(error) = keepalive_result {
                        close_message = Some(format!("SSH keepalive failed: {}", error));
                        break;
                    }
                    last_keepalive = Instant::now();
                }
                thread::sleep(idle_sleep);
                idle_sleep = (idle_sleep * 2).min(MAX_IDLE_SLEEP);
            }
        }

        if !pending.is_empty() {
            let data = String::from_utf8_lossy(&pending).to_string();
            let _ = app.emit(
                "terminal-output",
                TerminalOutputPayload {
                    session_id: session_id.clone(),
                    terminal_id: terminal_id.clone(),
                    data,
                },
            );
        }

        if let Ok(mut channel) = channel.lock() {
            let _ = channel.close();
        }

        finish_terminal_reader(&app, session_id, terminal_id, close_message, &reader_state);
    });
}

fn start_local_terminal_reader(
    app: AppHandle,
    session_id: String,
    terminal_id: String,
    mut reader: Box<dyn Read + Send>,
    stop: Arc<AtomicBool>,
    reader_state: TerminalReaderState,
) {
    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        let mut pending = Vec::new();
        let mut close_message = None;

        while !stop.load(Ordering::SeqCst) {
            match reader.read(&mut buffer) {
                Ok(0) => {
                    close_message = Some("Local shell closed".to_string());
                    break;
                }
                Ok(bytes_read) => {
                    let data = drain_utf8_stream(&mut pending, &buffer[..bytes_read]);
                    if !data.is_empty() {
                        let _ = app.emit(
                            "terminal-output",
                            TerminalOutputPayload {
                                session_id: session_id.clone(),
                                terminal_id: terminal_id.clone(),
                                data,
                            },
                        );
                    }
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => {
                    if !stop.load(Ordering::SeqCst) {
                        close_message = Some(format!("Local terminal stream failed: {}", error));
                    }
                    break;
                }
            }
        }

        if !pending.is_empty() {
            let data = String::from_utf8_lossy(&pending).to_string();
            let _ = app.emit(
                "terminal-output",
                TerminalOutputPayload {
                    session_id: session_id.clone(),
                    terminal_id: terminal_id.clone(),
                    data,
                },
            );
        }
        finish_terminal_reader(&app, session_id, terminal_id, close_message, &reader_state);
    });
}

fn finish_terminal_reader(
    app: &AppHandle,
    session_id: String,
    terminal_id: String,
    close_message: Option<String>,
    reader_state: &TerminalReaderState,
) {
    let session_closed = if let Ok(mut registered) = reader_state.terminals.lock() {
        registered.remove(&terminal_id);
        !registered
            .values()
            .any(|handle| handle.session_id == session_id)
    } else {
        false
    };
    if session_closed {
        if let Ok(mut active) = reader_state.active_connections.lock() {
            active.remove(&session_id);
        }
        if let Ok(mut stops) = reader_state.telemetry_stops.lock() {
            if let Some(monitor_stop) = stops.remove(&session_id) {
                monitor_stop.store(true, Ordering::SeqCst);
            }
        }
        crate::ssh::session::drop_ops_session(&reader_state.ops_sessions, &session_id);
        if let Ok(mut cache) = reader_state.remote_os_cache.lock() {
            cache.remove(&session_id);
        }
    }
    let _ = app.emit(
        "terminal-closed",
        TerminalClosedPayload {
            session_id,
            terminal_id,
            session_closed,
            message: close_message,
        },
    );
}

fn start_system_monitor(app: AppHandle, state: &AppState, target: crate::ssh::session::SshTarget) {
    let stop = Arc::new(AtomicBool::new(false));
    if let Ok(mut stops) = state.telemetry_stops.lock() {
        if let Some(previous) = stops.remove(&target.session_id) {
            previous.store(true, Ordering::SeqCst);
        }
        stops.insert(target.session_id.clone(), Arc::clone(&stop));
    }
    system_monitor::start(app, target, stop, Arc::clone(&state.telemetry_settings));
}

fn start_local_system_monitor(app: AppHandle, state: &AppState, session_id: String) {
    let stop = Arc::new(AtomicBool::new(false));
    if let Ok(mut stops) = state.telemetry_stops.lock() {
        if let Some(previous) = stops.remove(&session_id) {
            previous.store(true, Ordering::SeqCst);
        }
        stops.insert(session_id.clone(), Arc::clone(&stop));
    }
    system_monitor::start_local(app, session_id, stop, Arc::clone(&state.telemetry_settings));
}

fn stop_existing_session(state: &AppState, session_id: &str) {
    crate::ssh::session::drop_ops_session(&state.ops_sessions, session_id);
    if let Ok(mut cache) = state.remote_os_cache.lock() {
        cache.remove(session_id);
    }

    if let Ok(mut stops) = state.telemetry_stops.lock() {
        if let Some(stop) = stops.remove(session_id) {
            stop.store(true, Ordering::SeqCst);
        }
    }

    let handles = if let Ok(mut terminals) = state.terminals.lock() {
        let terminal_ids = terminals
            .iter()
            .filter_map(|(terminal_id, handle)| {
                (handle.session_id == session_id).then_some(terminal_id.clone())
            })
            .collect::<Vec<_>>();
        terminal_ids
            .into_iter()
            .filter_map(|terminal_id| terminals.remove(&terminal_id))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    for handle in handles {
        stop_terminal_handle(handle);
    }
}

fn stop_terminal_handle(handle: TerminalHandle) {
    handle.stop.store(true, Ordering::SeqCst);
    match handle.backend {
        TerminalBackend::Remote {
            connection,
            channel,
        } => {
            if let Ok(mut channel) = channel.lock() {
                let _ = channel.close();
            }
            let _ = connection
                .session
                .disconnect(None, "GpuTerm terminal disconnected", None);
        }
        TerminalBackend::Local { child, .. } => {
            if let Ok(mut child) = child.lock() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

fn fill_stored_credentials(
    target: &mut SshTarget,
    credentials: &impl CredentialStore,
    errors: &mut Vec<String>,
) {
    if target.password.is_none() {
        match credentials.get_password(&target.session_id) {
            Ok(password) => target.password = password,
            Err(error) => errors.push(error),
        }
    }
    if let Some(proxy) = target.proxy.as_mut() {
        fill_stored_credentials(proxy, credentials, errors);
    }
}

fn sync_target_credentials(
    target: &SshTarget,
    credentials: &impl CredentialStore,
    clear_missing: bool,
) -> Result<(), String> {
    let mut errors = Vec::new();
    sync_target_credentials_inner(target, credentials, clear_missing, &mut errors);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join(" "))
    }
}

fn sync_target_credentials_inner(
    target: &SshTarget,
    credentials: &impl CredentialStore,
    clear_missing: bool,
    errors: &mut Vec<String>,
) {
    if let Some(password) = target.password.clone() {
        if let Err(error) = credentials.set_password(&target.session_id, password) {
            errors.push(error);
        }
    } else if clear_missing {
        if let Err(error) = credentials.clear_password(&target.session_id) {
            errors.push(error);
        }
    }
    if let Some(proxy) = target.proxy.as_ref() {
        sync_target_credentials_inner(proxy, credentials, clear_missing, errors);
    }
}

fn pty_size(cols: u32, rows: u32) -> PtySize {
    PtySize {
        rows: rows.max(1).min(u16::MAX as u32) as u16,
        cols: cols.max(1).min(u16::MAX as u32) as u16,
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn default_local_shell() -> String {
    #[cfg(target_os = "windows")]
    {
        env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
    }
    #[cfg(not(target_os = "windows"))]
    {
        env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
    }
}

fn open_local_terminal(cols: u32, rows: u32) -> Result<OpenLocalTerminal, String> {
    open_local_terminal_with_shell(default_local_shell(), cols, rows)
}

fn open_local_terminal_with_shell(
    shell: String,
    cols: u32,
    rows: u32,
) -> Result<OpenLocalTerminal, String> {
    let pair = native_pty_system()
        .openpty(pty_size(cols, rows))
        .map_err(|error| format!("Failed to allocate local PTY: {}", error))?;
    let mut command = CommandBuilder::new(shell);
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    command.env("TERM_PROGRAM", "GpuTerm");
    if let Some(home) = env::var_os(if cfg!(target_os = "windows") {
        "USERPROFILE"
    } else {
        "HOME"
    }) {
        command.cwd(home);
    }

    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| format!("Failed to start local shell: {}", error))?;
    drop(pair.slave);
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| format!("Failed to open local terminal output: {}", error))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|error| format!("Failed to open local terminal input: {}", error))?;

    Ok(OpenLocalTerminal {
        master: Arc::new(Mutex::new(pair.master)),
        writer: Arc::new(Mutex::new(writer)),
        child: Arc::new(Mutex::new(child)),
        reader,
    })
}

fn open_terminal_channel(
    target: &SshTarget,
    cols: u32,
    rows: u32,
) -> Result<(SshConnection, Channel), String> {
    let connection = open_ssh_session(target)?;
    connection
        .session
        .set_keepalive(true, KEEPALIVE_INTERVAL_SECS);
    let mut channel = connection
        .session
        .channel_session()
        .map_err(|error| format!("Failed to open SSH channel: {}", error))?;
    channel
        .request_pty(
            "xterm-256color",
            None,
            Some((cols.max(1), rows.max(1), 0, 0)),
        )
        .map_err(|error| format!("Failed to allocate remote PTY: {}", error))?;
    channel
        .handle_extended_data(ExtendedData::Merge)
        .map_err(|error| format!("Failed to configure SSH stderr stream: {}", error))?;
    channel
        .shell()
        .map_err(|error| format!("Failed to start remote shell: {}", error))?;
    // libssh2's API flag and the actual TCP socket must both be nonblocking.
    // Leaving the socket blocking makes the reader hold the channel mutex
    // until its receive timeout and can turn key bursts into `transport read`
    // disconnects, especially on Windows and through ProxyJump tunnels.
    connection.set_blocking(false)?;
    Ok((connection, channel))
}

fn is_closed_channel_error(error: &std::io::Error) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    message.contains("closed this channel")
        || message.contains("channel is closed")
        || message.contains("channel is not open")
}

fn write_all_nonblocking(writer: &mut impl Write, mut bytes: &[u8]) -> std::io::Result<()> {
    let started = Instant::now();
    while !bytes.is_empty() {
        match writer.write(bytes) {
            Ok(0) => {
                if started.elapsed() > NONBLOCKING_OPERATION_TIMEOUT {
                    return Err(std::io::Error::new(
                        ErrorKind::TimedOut,
                        "terminal write timed out",
                    ));
                }
                thread::sleep(NONBLOCKING_RETRY_SLEEP);
            }
            Ok(count) => bytes = &bytes[count..],
            Err(error)
                if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) =>
            {
                if started.elapsed() > NONBLOCKING_OPERATION_TIMEOUT {
                    return Err(std::io::Error::new(
                        ErrorKind::TimedOut,
                        "terminal write timed out",
                    ));
                }
                thread::sleep(NONBLOCKING_RETRY_SLEEP);
            }
            Err(error) => return Err(error),
        }
    }
    // Do not call `Channel::flush()` here. In libssh2 that API discards
    // queued incoming channel data and adjusts the receive window; it is not
    // an outgoing socket flush. Calling it after each key can throw away the
    // remote echo and collide with the reader's nonblocking state machine.
    Ok(())
}

fn is_ssh_would_block(error: &ssh2::Error) -> bool {
    matches!(
        error.code(),
        ssh2::ErrorCode::Session(code) if code == LIBSSH2_ERROR_EAGAIN
    )
}

fn send_keepalive_nonblocking(session: &Session) -> Result<(), ssh2::Error> {
    let started = Instant::now();
    loop {
        match session.keepalive_send() {
            Ok(_) => return Ok(()),
            Err(error)
                if is_ssh_would_block(&error)
                    && started.elapsed() <= NONBLOCKING_OPERATION_TIMEOUT =>
            {
                thread::sleep(NONBLOCKING_RETRY_SLEEP);
            }
            Err(error) => return Err(error),
        }
    }
}

fn resize_remote_pty_nonblocking(
    channel: &mut Channel,
    cols: u32,
    rows: u32,
) -> Result<(), ssh2::Error> {
    let started = Instant::now();
    loop {
        match channel.request_pty_size(cols, rows, None, None) {
            Ok(()) => return Ok(()),
            Err(error)
                if is_ssh_would_block(&error)
                    && started.elapsed() <= NONBLOCKING_OPERATION_TIMEOUT =>
            {
                thread::sleep(NONBLOCKING_RETRY_SLEEP);
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        drain_utf8_stream, fill_stored_credentials, is_closed_channel_error, is_ssh_would_block,
        open_local_terminal_with_shell, pty_size, write_all_nonblocking,
    };
    use crate::ssh::credentials::{CredentialStore, MemoryCredentialStore};
    use crate::ssh::session::SshTarget;
    use std::io::{Error, ErrorKind, Read, Write};

    #[derive(Default)]
    struct BurstyWriter {
        attempts: usize,
        bytes: Vec<u8>,
        flushes: usize,
    }

    impl Write for BurstyWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.attempts += 1;
            if self.attempts == 1 {
                return Err(Error::new(ErrorKind::WouldBlock, "busy"));
            }
            let count = bytes.len().min(1);
            self.bytes.extend_from_slice(&bytes[..count]);
            Ok(count)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            self.flushes += 1;
            Err(Error::other("SSH channel flush must not be called"))
        }
    }

    #[test]
    fn passes_through_complete_utf8() {
        let mut pending = Vec::new();
        let text = "한글 테스트 メモリ 😀";
        assert_eq!(drain_utf8_stream(&mut pending, text.as_bytes()), text);
        assert!(pending.is_empty());
    }

    #[test]
    fn reassembles_multibyte_sequences_split_at_every_boundary() {
        let text = "한글 테스트";
        let bytes = text.as_bytes();
        for split in 1..bytes.len() {
            let mut pending = Vec::new();
            let mut output = drain_utf8_stream(&mut pending, &bytes[..split]);
            output.push_str(&drain_utf8_stream(&mut pending, &bytes[split..]));
            assert_eq!(output, text, "split at byte {}", split);
            assert!(pending.is_empty(), "split at byte {}", split);
        }
    }

    #[test]
    fn reassembles_four_byte_emoji_split_chunks() {
        let bytes = "😀".as_bytes();
        for split in [1, 2, 3] {
            let mut pending = Vec::new();
            let first = drain_utf8_stream(&mut pending, &bytes[..split]);
            assert_eq!(first, "", "split at byte {}", split);
            assert_eq!(pending.len(), split);
            let second = drain_utf8_stream(&mut pending, &bytes[split..]);
            assert_eq!(second, "😀", "split at byte {}", split);
            assert!(pending.is_empty());
        }
    }

    #[test]
    fn replaces_invalid_bytes_mid_stream() {
        let mut pending = Vec::new();
        let mut input = b"ok".to_vec();
        input.push(0xFF);
        input.extend_from_slice("한".as_bytes());
        assert_eq!(drain_utf8_stream(&mut pending, &input), "ok\u{FFFD}한");
        assert!(pending.is_empty());
    }

    #[test]
    fn keeps_truncated_lead_byte_pending() {
        let mut pending = Vec::new();
        let mut input = b"tail: ".to_vec();
        input.push(0xED); // first byte of a 3-byte sequence
        assert_eq!(drain_utf8_stream(&mut pending, &input), "tail: ");
        assert_eq!(pending, vec![0xED]);
    }

    #[test]
    fn recognizes_normal_closed_channel_write_race() {
        let closed = Error::other("We have already closed this channel");
        let unrelated = Error::new(ErrorKind::BrokenPipe, "network cable disconnected");
        assert!(is_closed_channel_error(&closed));
        assert!(!is_closed_channel_error(&unrelated));
    }

    #[test]
    fn retries_key_bursts_without_flushing_incoming_ssh_data() {
        let mut writer = BurstyWriter::default();
        write_all_nonblocking(&mut writer, b"as").unwrap();
        assert_eq!(writer.bytes, b"as");
        assert_eq!(writer.flushes, 0);
        assert!(writer.attempts >= 3);
    }

    #[test]
    fn recognizes_libssh2_nonblocking_retry_code() {
        let retry = ssh2::Error::new(ssh2::ErrorCode::Session(-37), "operation would block");
        let fatal = ssh2::Error::new(ssh2::ErrorCode::Session(-43), "transport read");
        assert!(is_ssh_would_block(&retry));
        assert!(!is_ssh_would_block(&fatal));
    }

    #[test]
    fn clamps_local_pty_dimensions() {
        let minimum = pty_size(0, 0);
        assert_eq!(minimum.cols, 1);
        assert_eq!(minimum.rows, 1);

        let maximum = pty_size(u32::MAX, u32::MAX);
        assert_eq!(maximum.cols, u16::MAX);
        assert_eq!(maximum.rows, u16::MAX);
    }

    #[test]
    fn restores_target_and_jump_passwords_from_memory() {
        let credentials = MemoryCredentialStore::default();
        credentials
            .set_password("target", "target-secret".to_string())
            .unwrap();
        credentials
            .set_password("jump", "jump-secret".to_string())
            .unwrap();
        let mut target = SshTarget {
            session_id: "target".to_string(),
            host: "target.example".to_string(),
            port: 22,
            username: "target-user".to_string(),
            password: None,
            private_key_path: None,
            proxy: Some(Box::new(SshTarget {
                session_id: "jump".to_string(),
                host: "jump.example".to_string(),
                port: 22,
                username: "jump-user".to_string(),
                password: None,
                private_key_path: None,
                proxy: None,
            })),
        };

        let mut errors = Vec::new();
        fill_stored_credentials(&mut target, &credentials, &mut errors);
        assert!(errors.is_empty());
        assert_eq!(target.password.as_deref(), Some("target-secret"));
        assert_eq!(
            target.proxy.as_ref().unwrap().password.as_deref(),
            Some("jump-secret")
        );
    }

    #[cfg(unix)]
    #[test]
    fn exchanges_data_with_a_local_shell_pty() {
        let opened = open_local_terminal_with_shell("/bin/sh".to_string(), 80, 24).unwrap();
        {
            let mut writer = opened.writer.lock().unwrap();
            writer
                .write_all(b"printf '__GPUTERM_LOCAL_PTY_OK__\\n'; exit\\n")
                .unwrap();
            writer.flush().unwrap();
        }

        let mut reader = opened.reader;
        let mut output = Vec::new();
        let mut buffer = [0_u8; 1024];
        while !String::from_utf8_lossy(&output).contains("__GPUTERM_LOCAL_PTY_OK__") {
            let count = reader.read(&mut buffer).unwrap();
            assert!(count > 0, "local shell closed before emitting the marker");
            output.extend_from_slice(&buffer[..count]);
        }
        assert!(String::from_utf8_lossy(&output).contains("__GPUTERM_LOCAL_PTY_OK__"));
    }
}
