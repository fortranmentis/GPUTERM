use crate::ssh::credentials::CredentialStore;
use crate::ssh::session::{
    profile_from_request, target_from_request, upsert_profile, ActiveConnection, AppState,
    SessionConnectRequest, TerminalSessionInfo,
};
use crate::ssh::session::{open_ssh_session, SshConnection};
use crate::ssh::system_monitor;
use serde::Serialize;
use ssh2::{Channel, ExtendedData, Session};
use std::io::{ErrorKind, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, State};

pub struct TerminalHandle {
    // Holds the connection (and any jump-host tunnel) alive for the session.
    pub connection: SshConnection,
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
    let opened = tauri::async_runtime::spawn_blocking(
        move || -> Result<(SshConnection, Channel), String> {
            let connection = open_ssh_session(&connect_target)?;
            connection.session.set_keepalive(true, KEEPALIVE_INTERVAL_SECS);
            let mut channel = connection
                .session
                .channel_session()
                .map_err(|error| format!("Failed to open SSH channel: {}", error))?;
            channel
                .request_pty("xterm-256color", None, Some((cols, rows, 0, 0)))
                .map_err(|error| format!("Failed to allocate remote PTY: {}", error))?;
            channel
                .handle_extended_data(ExtendedData::Merge)
                .map_err(|error| format!("Failed to configure SSH stderr stream: {}", error))?;
            channel
                .shell()
                .map_err(|error| format!("Failed to start remote shell: {}", error))?;
            connection.session.set_blocking(false);
            Ok((connection, channel))
        },
    )
    .await
    .map_err(|error| format!("Terminal connect task failed: {}", error))?;

    let (connection, channel) = match opened {
        Ok(pair) => pair,
        Err(error) => {
            state.credentials.clear_password(&profile.id);
            return Err(error);
        }
    };

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
    let reader_session = connection.session.clone();
    let handle = TerminalHandle {
        connection,
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
    start_terminal_reader(app.clone(), profile.id.clone(), reader_session, channel, stop);
    start_system_monitor(app, &state, target);

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
    session: Session,
    channel: Arc<Mutex<Channel>>,
    stop: Arc<AtomicBool>,
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
                    data,
                },
            );
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

    if let Ok(mut terminals) = state.terminals.lock() {
        if let Some(handle) = terminals.remove(session_id) {
            handle.stop.store(true, Ordering::SeqCst);
            if let Ok(mut channel) = handle.channel.lock() {
                let _ = channel.close();
            }
            let _ = handle
                .connection
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

#[cfg(test)]
mod tests {
    use super::drain_utf8_stream;

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
}
