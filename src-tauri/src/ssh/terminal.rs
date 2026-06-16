use crate::ssh::credentials::CredentialStore;
use crate::ssh::gpu_monitor;
use crate::ssh::session::{
    profile_from_request, target_from_request, upsert_profile, ActiveConnection, AppState,
    SessionConnectRequest, TerminalSessionInfo,
};
use crate::ssh::session::open_ssh_session;
use serde::Serialize;
use ssh2::{Channel, ExtendedData, Session};
use std::io::{ErrorKind, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, State};

pub struct TerminalHandle {
    pub session: Session,
    pub channel: Arc<Mutex<Channel>>,
    pub stop: Arc<AtomicBool>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TerminalOutputPayload {
    session_id: String,
    data: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TerminalClosedPayload {
    session_id: String,
    message: Option<String>,
}

#[tauri::command]
pub fn connect_terminal(
    app: AppHandle,
    state: State<AppState>,
    request: SessionConnectRequest,
) -> Result<TerminalSessionInfo, String> {
    let profile = profile_from_request(&request);
    let target = target_from_request(&profile, &request);

    if let Some(password) = target.password.clone() {
        state.credentials.set_password(&profile.id, password);
    } else {
        state.credentials.clear_password(&profile.id);
    }

    let session = match open_ssh_session(&target) {
        Ok(session) => session,
        Err(error) => {
            state.credentials.clear_password(&profile.id);
            return Err(error);
        }
    };

    session.set_keepalive(false, 30);
    let mut channel = session
        .channel_session()
        .map_err(|error| format!("Failed to open SSH channel: {}", error))?;
    let cols = request.cols.unwrap_or(120).max(1);
    let rows = request.rows.unwrap_or(32).max(1);
    channel
        .request_pty("xterm-256color", None, Some((cols, rows, 0, 0)))
        .map_err(|error| format!("Failed to allocate remote PTY: {}", error))?;
    channel
        .handle_extended_data(ExtendedData::Merge)
        .map_err(|error| format!("Failed to configure SSH stderr stream: {}", error))?;
    channel
        .shell()
        .map_err(|error| format!("Failed to start remote shell: {}", error))?;

    session.set_blocking(false);

    stop_existing_session(&state, &profile.id);

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

    let stop = Arc::new(AtomicBool::new(false));
    let channel = Arc::new(Mutex::new(channel));
    let handle = TerminalHandle {
        session,
        channel: Arc::clone(&channel),
        stop: Arc::clone(&stop),
    };

    {
        let mut terminals = state
            .terminals
            .lock()
            .map_err(|_| "Terminal state is unavailable".to_string())?;
        terminals.insert(profile.id.clone(), handle);
    }

    upsert_profile(profile.clone())?;
    start_terminal_reader(app.clone(), profile.id.clone(), channel, stop);
    start_gpu_monitor(app, &state, target);

    Ok(TerminalSessionInfo {
        session_id: profile.id.clone(),
        profile,
    })
}

#[tauri::command]
pub fn terminal_write(
    state: State<AppState>,
    session_id: String,
    data: String,
) -> Result<(), String> {
    let channel = {
        let terminals = state
            .terminals
            .lock()
            .map_err(|_| "Terminal state is unavailable".to_string())?;
        terminals
            .get(&session_id)
            .map(|handle| Arc::clone(&handle.channel))
            .ok_or_else(|| "No active terminal is available".to_string())?
    };

    let mut channel = channel
        .lock()
        .map_err(|_| "Terminal channel is unavailable".to_string())?;
    write_all_nonblocking(&mut channel, data.as_bytes())
        .map_err(|error| format!("Failed to write to remote terminal: {}", error))
}

#[tauri::command]
pub fn terminal_resize(
    state: State<AppState>,
    session_id: String,
    cols: u32,
    rows: u32,
) -> Result<(), String> {
    let channel = {
        let terminals = state
            .terminals
            .lock()
            .map_err(|_| "Terminal state is unavailable".to_string())?;
        terminals
            .get(&session_id)
            .map(|handle| Arc::clone(&handle.channel))
            .ok_or_else(|| "No active terminal is available".to_string())?
    };

    let mut channel = channel
        .lock()
        .map_err(|_| "Terminal channel is unavailable".to_string())?;
    channel
        .request_pty_size(cols.max(1), rows.max(1), None, None)
        .map_err(|error| format!("Failed to resize remote PTY: {}", error))
}

#[tauri::command]
pub fn disconnect_terminal(state: State<AppState>, session_id: String) -> Result<(), String> {
    stop_existing_session(&state, &session_id);
    state.credentials.clear_password(&session_id);
    if let Ok(mut active) = state.active_connections.lock() {
        active.remove(&session_id);
    }
    Ok(())
}

fn start_terminal_reader(
    app: AppHandle,
    session_id: String,
    channel: Arc<Mutex<Channel>>,
    stop: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        let mut close_message = None;

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

            match read_result {
                Ok(bytes_read) if bytes_read > 0 => {
                    let data = String::from_utf8_lossy(&buffer[..bytes_read]).to_string();
                    let _ = app.emit(
                        "terminal-output",
                        TerminalOutputPayload {
                            session_id: session_id.clone(),
                            data,
                        },
                    );
                }
                Ok(_) => {
                    thread::sleep(Duration::from_millis(12));
                }
                Err(error)
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) =>
                {
                    thread::sleep(Duration::from_millis(12));
                }
                Err(error) => {
                    close_message = Some(format!("Terminal stream failed: {}", error));
                    break;
                }
            }
        }

        if let Ok(mut channel) = channel.lock() {
            let _ = channel.close();
        }
        let _ = app.emit(
            "terminal-closed",
            TerminalClosedPayload {
                session_id,
                message: close_message,
            },
        );
    });
}

fn start_gpu_monitor(app: AppHandle, state: &AppState, target: crate::ssh::session::SshTarget) {
    let stop = Arc::new(AtomicBool::new(false));
    if let Ok(mut stops) = state.gpu_stops.lock() {
        if let Some(previous) = stops.remove(&target.session_id) {
            previous.store(true, Ordering::SeqCst);
        }
        stops.insert(target.session_id.clone(), Arc::clone(&stop));
    }
    gpu_monitor::start(app, target, stop);
}

fn stop_existing_session(state: &AppState, session_id: &str) {
    if let Ok(mut stops) = state.gpu_stops.lock() {
        if let Some(stop) = stops.remove(session_id) {
            stop.store(true, Ordering::SeqCst);
        }
    }

    if let Ok(mut terminals) = state.terminals.lock() {
        if let Some(handle) = terminals.remove(session_id) {
            handle.stop.store(true, Ordering::SeqCst);
            if let Ok(mut channel) = handle.channel.lock() {
                let _ = channel.close();
            }
            let _ = handle
                .session
                .disconnect(None, "GpuTerm terminal disconnected", None);
        }
    }
}

fn write_all_nonblocking(channel: &mut Channel, mut bytes: &[u8]) -> std::io::Result<()> {
    let started = Instant::now();
    while !bytes.is_empty() {
        match channel.write(bytes) {
            Ok(0) => {
                if started.elapsed() > Duration::from_secs(5) {
                    return Err(std::io::Error::new(
                        ErrorKind::TimedOut,
                        "terminal write timed out",
                    ));
                }
                thread::sleep(Duration::from_millis(8));
            }
            Ok(count) => bytes = &bytes[count..],
            Err(error)
                if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) =>
            {
                if started.elapsed() > Duration::from_secs(5) {
                    return Err(std::io::Error::new(
                        ErrorKind::TimedOut,
                        "terminal write timed out",
                    ));
                }
                thread::sleep(Duration::from_millis(8));
            }
            Err(error) => return Err(error),
        }
    }
    channel.flush()
}
