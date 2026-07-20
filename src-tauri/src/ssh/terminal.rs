use crate::ssh::credentials::CredentialStore;
use crate::ssh::session::{open_ssh_session, SshConnection};
use crate::ssh::session::{
    profile_from_request, target_for_active_session, target_from_request, upsert_profile,
    ActiveConnection, AppState, OpsSessions, SessionConnectRequest, SshTarget, TerminalSessionInfo,
};
use crate::ssh::system_monitor;
use serde::Serialize;
use ssh2::{Channel, ExtendedData, Session};
use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, State};

pub struct TerminalHandle {
    pub session_id: String,
    // Holds the connection (and any jump-host tunnel) alive for the session.
    pub connection: SshConnection,
    pub channel: Arc<Mutex<Channel>>,
    pub stop: Arc<AtomicBool>,
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
    let target = target_from_request(&profile, &request)?;

    if let Some(password) = target.password.clone() {
        state.credentials.set_password(&profile.id, password);
    } else {
        state.credentials.clear_password(&profile.id);
    }

    // Persist the jump-host password under the jump profile id so reconnects
    // (telemetry, SFTP, resource details) can re-resolve the tunnel.
    if let Some(proxy) = target.proxy.as_ref() {
        if let Some(proxy_password) = proxy.password.clone() {
            state.credentials.set_password(&proxy.session_id, proxy_password);
        }
    }

    let cols = request.cols.unwrap_or(120).max(1);
    let rows = request.rows.unwrap_or(32).max(1);
    let connect_target = target.clone();
    let opened = tauri::async_runtime::spawn_blocking(move || {
        open_terminal_channel(&connect_target, cols, rows)
    })
    .await
    .map_err(|error| format!("Terminal connect task failed: {}", error))?;

    let (connection, channel) = match opened {
        Ok(pair) => pair,
        Err(error) => {
            state.credentials.clear_password(&profile.id);
            return Err(error);
        }
    };

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
        connection,
        channel: Arc::clone(&channel),
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
    start_terminal_reader(
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
                connection,
                channel: Arc::clone(&channel),
                stop: Arc::clone(&stop),
            },
        );
    }

    let reader_state = TerminalReaderState::from_app_state(&state);
    start_terminal_reader(
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
    let (channel, stop) = {
        let terminals = state
            .terminals
            .lock()
            .map_err(|_| "Terminal state is unavailable".to_string())?;
        let Some(handle) = terminals.get(&terminal_id) else {
            // Input can already be queued in the webview when a remote shell
            // closes. Treat that normal lifecycle race as a no-op.
            return Ok(());
        };
        (Arc::clone(&handle.channel), Arc::clone(&handle.stop))
    };
    if stop.load(Ordering::SeqCst) {
        return Ok(());
    }

    let mut channel = channel
        .lock()
        .map_err(|_| "Terminal channel is unavailable".to_string())?;
    if channel.eof() {
        return Ok(());
    }
    match write_all_nonblocking(&mut channel, data.as_bytes()) {
        Ok(()) => Ok(()),
        Err(error) if is_closed_channel_error(&error) => Ok(()),
        Err(error) => Err(format!("Failed to write to remote terminal: {}", error)),
    }
}

#[tauri::command]
pub fn terminal_resize(
    state: State<AppState>,
    terminal_id: String,
    cols: u32,
    rows: u32,
) -> Result<(), String> {
    let channel = {
        let terminals = state
            .terminals
            .lock()
            .map_err(|_| "Terminal state is unavailable".to_string())?;
        let Some(handle) = terminals.get(&terminal_id) else {
            return Ok(());
        };
        Arc::clone(&handle.channel)
    };

    let mut channel = channel
        .lock()
        .map_err(|_| "Terminal channel is unavailable".to_string())?;
    match channel.request_pty_size(cols.max(1), rows.max(1), None, None) {
        Ok(()) => Ok(()),
        Err(error) if error.to_string().to_ascii_lowercase().contains("closed") => Ok(()),
        Err(error) => Err(format!("Failed to resize remote PTY: {}", error)),
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
    state.credentials.clear_password(&session_id);
    if let Ok(mut active) = state.active_connections.lock() {
        active.remove(&session_id);
    }
    Ok(())
}

const KEEPALIVE_INTERVAL_SECS: u32 = 30;
const MIN_IDLE_SLEEP: Duration = Duration::from_millis(2);
const MAX_IDLE_SLEEP: Duration = Duration::from_millis(30);

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

fn start_terminal_reader(
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
                if last_keepalive.elapsed() >= Duration::from_secs(1) {
                    let _ = session.keepalive_send();
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
    });
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
    if let Ok(mut channel) = handle.channel.lock() {
        let _ = channel.close();
    }
    let _ = handle
        .connection
        .session
        .disconnect(None, "GpuTerm terminal disconnected", None);
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
    connection.session.set_blocking(false);
    Ok((connection, channel))
}

fn is_closed_channel_error(error: &std::io::Error) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    message.contains("closed this channel")
        || message.contains("channel is closed")
        || message.contains("channel is not open")
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

#[cfg(test)]
mod tests {
    use super::{drain_utf8_stream, is_closed_channel_error};
    use std::io::{Error, ErrorKind};

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
}
