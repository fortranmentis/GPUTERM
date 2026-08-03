use crate::llm::instance::LlmInstance;
use crate::llm::monitor::MonitorHandle;
use crate::ssh::agent_monitor::AgentQuotaHistories;
use crate::ssh::credentials::{CredentialStore, CredentialVaultStatus, SecureCredentialStore};
use crate::ssh::system_monitor::{RemoteOs, SystemMonitorSettings};
use crate::ssh::terminal::TerminalHandle;
use serde::{Deserialize, Serialize};
use ssh2::{Channel, HashType, HostKeyType, Session};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tauri::State;
use uuid::Uuid;

/// Shared per-session SSH connections reused for short operations
/// (directory listings, stat calls, resource details). `ssh2::Session` is
/// internally synchronized but operations on one session must be serialized,
/// hence the per-entry mutex.
pub type OpsSessions = Arc<Mutex<HashMap<String, Arc<Mutex<SshConnection>>>>>;
pub type AgentQuotaRefreshes = Arc<Mutex<HashMap<String, HashSet<String>>>>;

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Default)]
pub struct AppState {
    pub terminals: Arc<Mutex<HashMap<String, TerminalHandle>>>,
    pub active_connections: Arc<Mutex<HashMap<String, ActiveConnection>>>,
    pub telemetry_stops: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    pub telemetry_settings: Arc<Mutex<SystemMonitorSettings>>,
    pub credentials: SecureCredentialStore,
    pub ops_sessions: OpsSessions,
    pub transfer_cancels: Mutex<HashMap<String, Arc<AtomicBool>>>,
    /// Remote OS per session, detected once by the detail path so popover
    /// ticks skip the probe round-trips; cleared alongside the ops session.
    pub remote_os_cache: Arc<Mutex<HashMap<String, RemoteOs>>>,
    /// Provider names queued by the UI for an immediate account-quota refresh.
    pub agent_quota_refreshes: AgentQuotaRefreshes,
    /// In-memory AGY quota history keyed by monitored host/account identity.
    pub agent_quota_histories: AgentQuotaHistories,
    /// Registered Ollama/vLLM instances. Unlike everything above this is not
    /// keyed by SSH session: an LLM runtime is a standalone HTTP endpoint.
    pub llm_instances: Arc<Mutex<Vec<LlmInstance>>>,
    /// Latest LLM runtime telemetry plus the poller's refresh trigger.
    pub llm_monitor: Arc<MonitorHandle>,
}

#[derive(Clone)]
pub struct ActiveConnection {
    pub profile: SessionProfile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionProfile {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    /// Opens a shell on the machine running GpuTerm instead of using SSH.
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_local: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_key_path: Option<String>,
    /// Id of another saved profile to tunnel through (ProxyJump).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_jump_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionConnectRequest {
    pub id: Option<String>,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    #[serde(default)]
    pub is_local: bool,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub private_key_path: Option<String>,
    #[serde(default)]
    pub proxy_jump_id: Option<String>,
    #[serde(default)]
    pub proxy_jump_password: Option<String>,
    /// Reuses credentials held only in memory for double-click reconnects.
    #[serde(default)]
    pub reuse_stored_credentials: bool,
    #[serde(default)]
    pub cols: Option<u32>,
    #[serde(default)]
    pub rows: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSessionInfo {
    pub session_id: String,
    pub terminal_id: String,
    pub profile: SessionProfile,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_warning: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SshTarget {
    pub session_id: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: Option<String>,
    pub private_key_path: Option<String>,
    /// Jump host to tunnel through, resolved from the saved profile chain.
    pub proxy: Option<Box<SshTarget>>,
}

#[tauri::command(async)]
pub fn load_sessions() -> Result<Vec<SessionProfile>, String> {
    read_profiles()
}

#[tauri::command(async)]
pub fn save_session(profile: SessionProfile) -> Result<Vec<SessionProfile>, String> {
    upsert_profile(normalize_profile(profile))
}

#[tauri::command]
pub async fn delete_session(
    state: State<'_, AppState>,
    id: String,
) -> Result<Vec<SessionProfile>, String> {
    let credentials = state.credentials.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut profiles = read_profiles()?;
        profiles.retain(|profile| profile.id != id);
        // Vault clean-up is best effort: a locked or not-yet-initialized vault
        // must not make profile deletion impossible. Any leftover entry is
        // unreachable once the profile is gone.
        let vault_error = credentials.clear_password(&id).err();
        write_profiles(&profiles)?;
        if let Some(error) = vault_error {
            eprintln!("GpuTerm: could not clear stored credential for a deleted profile: {error}");
        }
        Ok(profiles)
    })
    .await
    .map_err(|error| format!("Session deletion task failed: {}", error))?
}

/// Reports whether a profile has a password/passphrase in the local vault
/// without ever returning the secret to the webview.
#[tauri::command]
pub async fn has_saved_credential(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<bool, String> {
    Ok(state.credentials.has_saved_credential(&session_id))
}

#[tauri::command]
pub fn get_credential_vault_status(state: State<'_, AppState>) -> CredentialVaultStatus {
    state.credentials.status()
}

#[tauri::command]
pub async fn initialize_credential_vault(
    state: State<'_, AppState>,
    master_password: String,
) -> Result<CredentialVaultStatus, String> {
    let credentials = state.credentials.clone();
    tauri::async_runtime::spawn_blocking(move || credentials.initialize(master_password))
        .await
        .map_err(|error| format!("Credential vault initialization task failed: {}", error))?
}

#[tauri::command]
pub async fn unlock_credential_vault(
    state: State<'_, AppState>,
    master_password: String,
) -> Result<CredentialVaultStatus, String> {
    let credentials = state.credentials.clone();
    tauri::async_runtime::spawn_blocking(move || credentials.unlock(master_password))
        .await
        .map_err(|error| format!("Credential vault unlock task failed: {}", error))?
}

#[tauri::command]
pub async fn reset_credential_vault(
    state: State<'_, AppState>,
) -> Result<CredentialVaultStatus, String> {
    let credentials = state.credentials.clone();
    tauri::async_runtime::spawn_blocking(move || credentials.reset())
        .await
        .map_err(|error| format!("Credential vault reset task failed: {}", error))?
}

#[tauri::command]
pub async fn test_ssh_connection(
    state: State<'_, AppState>,
    request: SessionConnectRequest,
) -> Result<String, String> {
    let credentials = state.credentials.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let profile = profile_from_request(&request);
        if profile.is_local {
            return Ok("Local terminal is available".to_string());
        }
        let mut target = target_from_request(&profile, &request)?;
        if request.reuse_stored_credentials {
            restore_stored_credentials(&mut target, &credentials)?;
        }
        let connection = open_ssh_session(&target)?;
        let _ = connection
            .session
            .disconnect(None, "GpuTerm connection test complete", None);
        Ok(format!(
            "SSH connection to {}:{} succeeded",
            profile.host, profile.port
        ))
    })
    .await
    .map_err(|error| format!("Connection test task failed: {}", error))?
}

fn restore_stored_credentials(
    target: &mut SshTarget,
    credentials: &impl CredentialStore,
) -> Result<(), String> {
    if target.password.is_none() {
        target.password = credentials.get_password(&target.session_id)?;
    }
    if let Some(proxy) = target.proxy.as_mut() {
        restore_stored_credentials(proxy, credentials)?;
    }
    Ok(())
}

/// Runs `f` against the shared operations connection for `target`'s session,
/// creating or replacing the connection when missing or dead. Access is
/// serialized per session, so keep the callbacks short — bulk transfers use
/// their own dedicated connections.
pub fn with_ops_session<T>(
    ops: &OpsSessions,
    target: &SshTarget,
    timeout_ms: u32,
    f: impl FnOnce(&Session) -> Result<T, String>,
) -> Result<T, String> {
    let existing = {
        let map = ops
            .lock()
            .map_err(|_| "Operations connection state is unavailable".to_string())?;
        map.get(&target.session_id).cloned()
    };

    if let Some(entry) = existing {
        if let Ok(connection) = entry.lock() {
            connection.session.set_timeout(timeout_ms);
            // Cheap liveness probe before running a possibly non-idempotent
            // operation: opening a channel round-trips to the server. A dead
            // tunnel also fails here, so it is replaced below.
            let alive = match connection.session.channel_session() {
                Ok(mut channel) => {
                    let _ = channel.close();
                    true
                }
                Err(_) => false,
            };
            if alive {
                return f(&connection.session);
            }
        }
        if let Ok(mut map) = ops.lock() {
            map.remove(&target.session_id);
        }
    }

    let connection = open_ssh_session(target)?;
    connection.session.set_keepalive(true, 15);
    connection.session.set_timeout(timeout_ms);
    let entry = Arc::new(Mutex::new(connection));
    {
        let mut map = ops
            .lock()
            .map_err(|_| "Operations connection state is unavailable".to_string())?;
        map.insert(target.session_id.clone(), Arc::clone(&entry));
    }
    let connection = entry
        .lock()
        .map_err(|_| "Operations connection is unavailable".to_string())?;
    f(&connection.session)
}

pub fn drop_ops_session(ops: &OpsSessions, session_id: &str) {
    if let Ok(mut map) = ops.lock() {
        map.remove(session_id);
    }
}

pub fn profile_from_request(request: &SessionConnectRequest) -> SessionProfile {
    normalize_profile(SessionProfile {
        id: request
            .id
            .as_ref()
            .filter(|id| !id.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| Uuid::new_v4().to_string()),
        name: request.name.clone(),
        host: request.host.clone(),
        port: request.port,
        username: request.username.clone(),
        is_local: request.is_local,
        private_key_path: normalize_optional_string(request.private_key_path.clone()),
        proxy_jump_id: normalize_optional_string(request.proxy_jump_id.clone()),
    })
}

pub fn target_from_request(
    profile: &SessionProfile,
    request: &SessionConnectRequest,
) -> Result<SshTarget, String> {
    if profile.is_local {
        return Err("Local terminal sessions do not use SSH targets".to_string());
    }
    // The form supplies a single jump-host password, applied to the immediate
    // jump host; deeper hops (rare) fall back to key/agent auth.
    let jump_password = normalize_optional_string(request.proxy_jump_password.clone());
    let immediate_jump = profile.proxy_jump_id.as_deref();
    let password_for = |id: &str| -> Option<String> {
        if immediate_jump == Some(id) {
            jump_password.clone()
        } else {
            None
        }
    };
    let proxy = resolve_proxy_chain(profile.proxy_jump_id.as_deref(), &password_for)?;
    Ok(SshTarget {
        session_id: profile.id.clone(),
        host: profile.host.clone(),
        port: profile.port,
        username: profile.username.clone(),
        password: normalize_optional_string(request.password.clone()),
        private_key_path: profile.private_key_path.clone(),
        proxy,
    })
}

pub fn target_for_active_session(state: &AppState, session_id: &str) -> Result<SshTarget, String> {
    let profile = {
        let active = state
            .active_connections
            .lock()
            .map_err(|_| "Active session state is unavailable".to_string())?;
        active
            .get(session_id)
            .map(|connection| connection.profile.clone())
            .ok_or_else(|| "No active SSH session is available".to_string())?
    };

    if profile.is_local {
        return Err("This operation is not available for a local terminal".to_string());
    }

    // Jump-host passwords entered at connect time are kept in the credential
    // store keyed by the jump profile id, so reconnects find them here.
    let password_for = |id: &str| state.credentials.get_password(id).ok().flatten();
    let proxy = resolve_proxy_chain(profile.proxy_jump_id.as_deref(), &password_for)?;
    Ok(SshTarget {
        session_id: profile.id.clone(),
        host: profile.host,
        port: profile.port,
        username: profile.username,
        password: state.credentials.get_password(session_id).ok().flatten(),
        private_key_path: profile.private_key_path,
        proxy,
    })
}

const MAX_PROXY_DEPTH: usize = 3;

/// Resolves a jump-host chain from saved profiles into nested `SshTarget`s.
/// Each hop's password comes from `password_for`; hops without one fall back
/// to key/agent auth.
fn resolve_proxy_chain(
    proxy_jump_id: Option<&str>,
    password_for: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<Box<SshTarget>>, String> {
    let Some(start) = proxy_jump_id else {
        return Ok(None);
    };
    let profiles = read_profiles()?;
    build_proxy_target(start, &profiles, &mut Vec::new(), password_for)
}

fn build_proxy_target(
    proxy_jump_id: &str,
    profiles: &[SessionProfile],
    visited: &mut Vec<String>,
    password_for: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<Box<SshTarget>>, String> {
    if visited.len() >= MAX_PROXY_DEPTH {
        return Err(format!(
            "Jump host chain is too deep (limit {}). Check for a loop.",
            MAX_PROXY_DEPTH
        ));
    }
    if visited.iter().any(|id| id == proxy_jump_id) {
        return Err("Jump host chain contains a loop.".to_string());
    }
    let profile = profiles
        .iter()
        .find(|profile| profile.id == proxy_jump_id)
        .ok_or_else(|| {
            format!(
                "Jump host profile not found: {}. It may have been deleted.",
                proxy_jump_id
            )
        })?;
    visited.push(proxy_jump_id.to_string());
    let proxy = match profile.proxy_jump_id.as_deref() {
        Some(next) => build_proxy_target(next, profiles, visited, password_for)?,
        None => None,
    };
    Ok(Some(Box::new(SshTarget {
        session_id: profile.id.clone(),
        host: profile.host.clone(),
        port: profile.port,
        username: profile.username.clone(),
        password: password_for(&profile.id),
        private_key_path: profile.private_key_path.clone(),
        proxy,
    })))
}

pub fn upsert_profile(profile: SessionProfile) -> Result<Vec<SessionProfile>, String> {
    let mut profiles = read_profiles()?;
    if let Some(existing) = profiles.iter_mut().find(|item| item.id == profile.id) {
        *existing = profile;
    } else {
        profiles.push(profile);
    }
    profiles.sort_by_key(|profile| profile.name.to_lowercase());
    write_profiles(&profiles)?;
    Ok(profiles)
}

/// An open SSH session plus, when the target is reached through a jump host,
/// the tunnel keeping the bastion connection and its forwarder thread alive.
///
/// Field order matters: `session` is dropped before `_transport` and `_tunnel`
/// so libssh2's disconnect packet still travels through a live socket/tunnel.
/// Do not reorder.
pub struct SshConnection {
    pub session: Session,
    // A clone of the socket owned by `Session`. Besides keeping the transport
    // alive through `Session` teardown, this lets terminal users switch the OS
    // socket itself to nonblocking mode. `Session::set_blocking(false)` only
    // changes libssh2's API mode; it does not update the socket flags.
    _transport: TcpStream,
    _tunnel: Option<SshTunnel>,
}

impl SshConnection {
    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Changes both the libssh2 API mode and the underlying TCP socket mode.
    /// They must stay in sync: a nonblocking libssh2 session backed by a
    /// blocking socket can hold the channel lock until SO_RCVTIMEO expires and
    /// surface that timeout as the fatal `transport read` error.
    pub fn set_blocking(&self, blocking: bool) -> Result<(), String> {
        self._transport
            .set_nonblocking(!blocking)
            .map_err(|error| format!("Failed to configure SSH transport mode: {}", error))?;
        self.session.set_blocking(blocking);
        Ok(())
    }
}

struct SshTunnel {
    stop: Arc<AtomicBool>,
    _bastion: Box<SshConnection>,
}

impl Drop for SshTunnel {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

pub fn open_ssh_session(target: &SshTarget) -> Result<SshConnection, String> {
    // Direct connection: TCP straight to the target.
    // Proxied connection: connect the bastion first, open a direct-tcpip
    // channel to the target through it, and expose it as a local socket the
    // normal handshake path connects to.
    let (tcp, tunnel) = match &target.proxy {
        None => {
            let address = format!("{}:{}", target.host, target.port);
            let socket_addr = address
                .to_socket_addrs()
                .map_err(|error| format!("Network resolution failed for {}: {}", address, error))?
                .next()
                .ok_or_else(|| format!("No network address found for {}", address))?;
            let tcp = TcpStream::connect_timeout(&socket_addr, Duration::from_secs(10)).map_err(
                |error| {
                    format!(
                        "Network timeout or connection failure for {}: {}",
                        address, error
                    )
                },
            )?;
            (tcp, None)
        }
        Some(proxy) => open_tunneled_stream(target, proxy)?,
    };

    let _ = tcp.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = tcp.set_write_timeout(Some(Duration::from_secs(10)));
    let transport = tcp
        .try_clone()
        .map_err(|error| format!("Failed to retain SSH transport: {}", error))?;

    let mut session =
        Session::new().map_err(|error| format!("Failed to create SSH session: {}", error))?;
    session.set_tcp_stream(tcp);
    session.set_timeout(10_000);
    session.handshake().map_err(|error| {
        format!(
            "SSH handshake failed for {}:{}: {}",
            target.host, target.port, error
        )
    })?;

    verify_known_host(&session, target)?;
    authenticate(&session, target)?;

    if !session.authenticated() {
        return Err(
            "SSH authentication failed. Check username, password, private key, or SSH agent."
                .to_string(),
        );
    }

    Ok(SshConnection {
        session,
        _transport: transport,
        _tunnel: tunnel,
    })
}

/// Connects the bastion, opens a direct-tcpip channel to `target` through it,
/// and pumps that channel over a local loopback socket so the caller can run a
/// normal SSH handshake against the target.
fn open_tunneled_stream(
    target: &SshTarget,
    proxy: &SshTarget,
) -> Result<(TcpStream, Option<SshTunnel>), String> {
    let bastion = open_ssh_session(proxy).map_err(|error| {
        if error.starts_with(UNKNOWN_HOST_KEY_PREFIX) {
            // Keep the sentinel intact so the frontend can prompt for the
            // bastion's own host key.
            error
        } else {
            format!("Jump host {}:{}: {}", proxy.host, proxy.port, error)
        }
    })?;

    let channel = bastion
        .session
        .channel_direct_tcpip(&target.host, target.port, None)
        .map_err(|error| {
            format!(
                "Jump host could not open a channel to {}:{}: {}",
                target.host, target.port, error
            )
        })?;

    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|error| format!("Failed to open local tunnel socket: {}", error))?;
    let local_addr = listener
        .local_addr()
        .map_err(|error| format!("Failed to read local tunnel address: {}", error))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("Failed to configure local tunnel socket: {}", error))?;

    bastion.session.set_keepalive(true, 15);
    bastion.set_blocking(false)?;

    let stop = Arc::new(AtomicBool::new(false));
    let forwarder_stop = Arc::clone(&stop);
    thread::spawn(move || run_tunnel_forwarder(listener, channel, forwarder_stop));

    let tcp = TcpStream::connect_timeout(&local_addr, Duration::from_secs(10))
        .map_err(|error| format!("Failed to connect through jump host: {}", error))?;

    let tunnel = SshTunnel {
        stop,
        _bastion: Box::new(bastion),
    };
    Ok((tcp, Some(tunnel)))
}

/// Accepts one loopback connection and pumps bytes between it and the bastion's
/// direct-tcpip channel until either side closes or the stop flag is set.
fn run_tunnel_forwarder(listener: TcpListener, mut channel: Channel, stop: Arc<AtomicBool>) {
    use std::sync::atomic::Ordering;

    // Wait for the caller's connect(), but do not leak the thread if it never
    // arrives.
    let deadline = Instant::now() + Duration::from_secs(10);
    let socket = loop {
        if stop.load(Ordering::SeqCst) || Instant::now() > deadline {
            let _ = channel.close();
            return;
        }
        match listener.accept() {
            Ok((socket, _)) => break socket,
            Err(ref error) if error.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(3));
            }
            Err(_) => {
                let _ = channel.close();
                return;
            }
        }
    };

    pump_channel_socket(socket, channel, &stop);
}

/// Pumps bytes between one loopback socket and one direct-tcpip channel until
/// either side closes or the stop flag is set.
///
/// Both the session and the socket must already be nonblocking: a blocking read
/// on a `Channel` holds the `Session` mutex across the syscall and would stall
/// every other channel on the same connection.
fn pump_channel_socket(socket: TcpStream, mut channel: Channel, stop: &AtomicBool) {
    use std::sync::atomic::Ordering;

    if socket.set_nonblocking(true).is_err() {
        let _ = channel.close();
        return;
    }

    let mut socket = socket;
    let mut to_channel: Vec<u8> = Vec::new();
    let mut to_socket: Vec<u8> = Vec::new();
    let mut buffer = [0_u8; 32 * 1024];
    let mut idle_sleep = Duration::from_millis(2);
    let max_sleep = Duration::from_millis(15);

    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let mut progressed = false;

        // Socket -> channel (only read more once the pending buffer is flushed).
        if to_channel.is_empty() {
            match socket.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    to_channel.extend_from_slice(&buffer[..count]);
                    progressed = true;
                }
                Err(ref error)
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
                Err(_) => break,
            }
        }
        if !to_channel.is_empty() {
            match channel.write(&to_channel) {
                Ok(0) => {}
                Ok(count) => {
                    to_channel.drain(..count);
                    progressed = true;
                }
                Err(ref error)
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
                Err(_) => break,
            }
        }

        // Channel -> socket.
        if to_socket.is_empty() {
            match channel.read(&mut buffer) {
                Ok(0) => {
                    if channel.eof() {
                        break;
                    }
                }
                Ok(count) => {
                    to_socket.extend_from_slice(&buffer[..count]);
                    progressed = true;
                }
                Err(ref error)
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
                Err(_) => break,
            }
        }
        if !to_socket.is_empty() {
            match socket.write(&to_socket) {
                Ok(0) => {}
                Ok(count) => {
                    to_socket.drain(..count);
                    progressed = true;
                }
                Err(ref error)
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
                Err(_) => break,
            }
        }

        if progressed {
            idle_sleep = Duration::from_millis(2);
        } else {
            thread::sleep(idle_sleep);
            idle_sleep = (idle_sleep * 2).min(max_sleep);
        }
    }

    let _ = channel.close();
}

// ---------------------------------------------------------------------------
// Persistent local port forwarding
//
// Used by the LLM runtime monitor to poll a runtime bound to the remote host's
// loopback. The ProxyJump path above forwards exactly one connection for the
// length of a handshake; this one serves many, for as long as it is held.
// ---------------------------------------------------------------------------

/// `LIBSSH2_ERROR_EAGAIN`. The `ssh2` crate does not re-export it, and pulling
/// in `libssh2-sys` directly for one integer is not worth it. The test below
/// pins it against ssh2's own `io::ErrorKind::WouldBlock` mapping so a change
/// in either place fails loudly.
const LIBSSH2_ERROR_EAGAIN: i32 = -37;

/// Simultaneous forwarded connections per instance.
///
/// One is the steady state; a second appears briefly when the runtime closes an
/// idle keep-alive socket and the HTTP client dials a fresh one. Kept well below
/// sshd's default `MaxSessions 10`, since one SSH connection hosts every
/// forward for that host.
const MAX_FORWARD_CONNECTIONS: usize = 4;

/// Consecutive channel-open failures before a forwarder gives up.
///
/// A backstop for the case where the transport still answers a keepalive but
/// channels cannot be opened (an sshd at its `MaxSessions`, for instance), so a
/// forward can never be stuck alive-but-useless.
const MAX_CONSECUTIVE_OPEN_FAILURES: u32 = 5;

fn is_would_block(error: &ssh2::Error) -> bool {
    matches!(error.code(), ssh2::ErrorCode::Session(code) if code == LIBSSH2_ERROR_EAGAIN)
}

/// Whether the SSH transport is still usable.
///
/// `keepalive_send` is the cheapest thing that actually touches the socket. On a
/// nonblocking session it can report `EAGAIN`, which means the write is pending
/// rather than impossible, so that counts as alive.
fn session_alive(session: &Session) -> bool {
    match session.keepalive_send() {
        Ok(_) => true,
        Err(error) => is_would_block(&error),
    }
}

/// Opens a direct-tcpip channel on a **nonblocking** session.
///
/// This retry loop is the whole reason the function exists. The ProxyJump path
/// opens its channel while the session is still blocking and so never sees
/// `EAGAIN`; on a nonblocking session the very first call usually returns it.
/// Treating that as a failure would leave a bound port that never serves
/// anything — a silent dead tunnel rather than a visible error.
fn open_direct_channel(
    session: &Session,
    host: &str,
    port: u16,
    deadline: Instant,
) -> Result<Channel, String> {
    let mut wait = Duration::from_millis(2);
    loop {
        match session.channel_direct_tcpip(host, port, None) {
            Ok(channel) => return Ok(channel),
            Err(error) if is_would_block(&error) => {
                if Instant::now() >= deadline {
                    return Err(format!(
                        "Timed out opening an SSH channel to {}:{}",
                        host, port
                    ));
                }
                thread::sleep(wait);
                wait = (wait * 2).min(Duration::from_millis(20));
            }
            Err(error) => {
                return Err(format!(
                    "SSH channel to {}:{} was refused: {}",
                    host, port, error
                ));
            }
        }
    }
}

/// Clears an `AtomicBool` on every exit path, including a panic.
struct FlagGuard(Arc<AtomicBool>);

impl Drop for FlagGuard {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Decrements a counter on every exit path, including a panic.
struct CountGuard(Arc<std::sync::atomic::AtomicUsize>);

impl Drop for CountGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// A loopback port forwarded to `host:port` on the far side of an SSH
/// connection, serving connections until dropped.
pub struct PersistentForward {
    pub local_port: u16,
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    hop_dead: Arc<AtomicBool>,
    /// Keeps the SSH hop open for as long as the forward exists.
    _connection: Arc<SshConnection>,
}

impl PersistentForward {
    /// False once the forwarder thread has exited for any reason, so the caller
    /// can rebuild rather than polling a port nothing is listening on.
    pub fn is_alive(&self) -> bool {
        self.alive.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// True when the forwarder stopped because the SSH transport itself is gone.
    ///
    /// The distinction matters: rebuilding a forward on a dead connection would
    /// succeed in binding a port and then fail every request forever, so the
    /// caller has to know to reconnect rather than just re-forward.
    pub fn hop_dead(&self) -> bool {
        self.hop_dead.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Drop for PersistentForward {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Opens a connection dedicated to hosting port forwards.
///
/// Nonblocking mode is applied before returning and **must not be reversed**: a
/// blocking read on any channel would hold the session mutex and stall every
/// other forward on the same connection. For the same reason this must never be
/// an `ops_sessions` entry or a terminal's session, both of which are used in
/// blocking mode.
pub fn open_forwarding_session(target: &SshTarget) -> Result<Arc<SshConnection>, String> {
    let connection = open_ssh_session(target)?;
    connection.session.set_keepalive(true, 15);
    connection.set_blocking(false)?;
    Ok(Arc::new(connection))
}

/// Binds a loopback listener and starts forwarding it to `remote_host:remote_port`.
///
/// Returns as soon as the port is bound. The first channel is opened by the
/// forwarder thread on the first accepted socket, so this never waits on the
/// network.
pub fn open_persistent_forward(
    connection: &Arc<SshConnection>,
    remote_host: &str,
    remote_port: u16,
) -> Result<PersistentForward, String> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|error| format!("Failed to open a local tunnel socket: {}", error))?;
    let local_port = listener
        .local_addr()
        .map_err(|error| format!("Failed to read the local tunnel address: {}", error))?
        .port();
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("Failed to configure the local tunnel socket: {}", error))?;

    let stop = Arc::new(AtomicBool::new(false));
    let alive = Arc::new(AtomicBool::new(true));
    let hop_dead = Arc::new(AtomicBool::new(false));
    let thread_connection = Arc::clone(connection);
    let thread_stop = Arc::clone(&stop);
    let thread_alive = Arc::clone(&alive);
    let thread_hop_dead = Arc::clone(&hop_dead);
    let host = remote_host.to_string();
    thread::spawn(move || {
        run_persistent_forward(
            listener,
            thread_connection,
            host,
            remote_port,
            thread_stop,
            thread_alive,
            thread_hop_dead,
        )
    });

    Ok(PersistentForward {
        local_port,
        stop,
        alive,
        hop_dead,
        _connection: Arc::clone(connection),
    })
}

/// Accepts loopback connections until stopped, opening one direct-tcpip channel
/// per connection and pumping each on its own thread.
fn run_persistent_forward(
    listener: TcpListener,
    connection: Arc<SshConnection>,
    remote_host: String,
    remote_port: u16,
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    hop_dead: Arc<AtomicBool>,
) {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let _alive = FlagGuard(alive);
    let live = Arc::new(AtomicUsize::new(0));
    let mut open_failures: u32 = 0;

    while !stop.load(Ordering::SeqCst) {
        let socket = match listener.accept() {
            Ok((socket, _)) => socket,
            Err(ref error) if error.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(3));
                continue;
            }
            // The listener itself failed; nothing can be served any more.
            Err(_) => break,
        };

        if live.load(Ordering::SeqCst) >= MAX_FORWARD_CONNECTIONS {
            // Refuse rather than queue: the client retries, and an unbounded
            // channel count would eventually hit sshd's MaxSessions.
            drop(socket);
            continue;
        }

        let deadline = Instant::now() + Duration::from_secs(10);
        let channel =
            match open_direct_channel(&connection.session, &remote_host, remote_port, deadline) {
                Ok(channel) => {
                    open_failures = 0;
                    channel
                }
                Err(_) => {
                    drop(socket);
                    open_failures += 1;

                    // A refusal by the runtime and a dead SSH hop look the same
                    // from here, but they need opposite responses: the first is
                    // per-connection, the second means this forwarder can never
                    // serve anything again. Without distinguishing them a dead
                    // hop leaves the accept loop running, the port bound, and
                    // the caller polling a tunnel that will always fail.
                    if !session_alive(&connection.session) {
                        hop_dead.store(true, Ordering::SeqCst);
                        break;
                    }
                    if open_failures >= MAX_CONSECUTIVE_OPEN_FAILURES {
                        // The transport answers but channels cannot be opened.
                        // Give up on this forward without condemning the hop.
                        break;
                    }
                    continue;
                }
            };

        live.fetch_add(1, Ordering::SeqCst);
        let pump_stop = Arc::clone(&stop);
        let pump_live = Arc::clone(&live);
        thread::spawn(move || {
            let _count = CountGuard(pump_live);
            pump_channel_socket(socket, channel, &pump_stop);
        });
    }
}

/// The saved profiles, for callers outside this module that need names or ids.
pub fn list_profiles() -> Result<Vec<SessionProfile>, String> {
    read_profiles()
}

/// Whether every hop to `target` already has a trusted key on file.
///
/// A pure file read, so a background thread can decide *before* opening a socket
/// whether the connect could need the interactive host-key prompt it has no way
/// to show. It cannot detect a key *mismatch* — that needs the live key — which
/// correctly still surfaces from `verify_known_host` with its own loud message.
pub fn host_keys_trusted(target: &SshTarget) -> bool {
    let Ok(known_hosts) = read_known_hosts() else {
        return false;
    };
    let mut hop = Some(target);
    while let Some(current) = hop {
        let key = format!("{}:{}", current.host.to_lowercase(), current.port);
        if !known_hosts.contains_key(&key) {
            return false;
        }
        hop = current.proxy.as_deref();
    }
    true
}

/// Builds a target for a saved profile, whether or not it is connected.
///
/// Prefers the live connection's profile when there is one, so a reconnecting
/// poller uses the same resolved chain the terminal did.
pub fn target_for_profile(
    active: &Arc<Mutex<HashMap<String, ActiveConnection>>>,
    credentials: &SecureCredentialStore,
    profile_id: &str,
) -> Result<SshTarget, String> {
    let live = active
        .lock()
        .ok()
        .and_then(|connections| connections.get(profile_id).map(|item| item.profile.clone()));

    let profile = match live {
        Some(profile) => profile,
        None => read_profiles()?
            .into_iter()
            .find(|profile| profile.id == profile_id)
            .ok_or_else(|| "That SSH session profile no longer exists".to_string())?,
    };

    if profile.is_local {
        return Err("A local terminal profile cannot be used as a tunnel".to_string());
    }

    let password_for = |id: &str| credentials.get_password(id).ok().flatten();
    let proxy = resolve_proxy_chain(profile.proxy_jump_id.as_deref(), &password_for)?;
    Ok(SshTarget {
        session_id: profile.id.clone(),
        host: profile.host,
        port: profile.port,
        username: profile.username,
        password: credentials.get_password(profile_id).ok().flatten(),
        private_key_path: profile.private_key_path,
        proxy,
    })
}

/// A stable description of everything that affects where a connection lands.
///
/// The password is deliberately excluded: unlocking the vault must not tear down
/// a connection that is already working.
pub fn target_signature(target: &SshTarget) -> String {
    let mut signature = format!(
        "{}@{}:{}|{}",
        target.username,
        target.host.to_lowercase(),
        target.port,
        target.private_key_path.as_deref().unwrap_or("-")
    );
    let mut hop = target.proxy.as_deref();
    while let Some(current) = hop {
        let _ = write!(
            signature,
            ">{}@{}:{}",
            current.username,
            current.host.to_lowercase(),
            current.port
        );
        hop = current.proxy.as_deref();
    }
    signature
}

fn authenticate(session: &Session, target: &SshTarget) -> Result<(), String> {
    if let Some(private_key_path) = normalize_optional_string(target.private_key_path.clone()) {
        let private_key = Path::new(&private_key_path);
        if !private_key.exists() {
            return Err(format!(
                "Private key file was not found: {}",
                private_key_path
            ));
        }

        session
            .userauth_pubkey_file(
                &target.username,
                None,
                private_key,
                target.password.as_deref(),
            )
            .map_err(|_| {
                "SSH key authentication failed. Check username, private key, and passphrase."
                    .to_string()
            })?;
        return Ok(());
    }

    if let Some(password) = normalize_optional_string(target.password.clone()) {
        session
            .userauth_password(&target.username, &password)
            .map_err(|_| {
                "SSH password authentication failed. Check username and password.".to_string()
            })?;
        return Ok(());
    }

    session.userauth_agent(&target.username).map_err(|_| {
        "SSH agent authentication failed. Enter a password or private key path.".to_string()
    })
}

/// Prefix of the sentinel error raised when connecting to a host whose key has
/// never been seen. Format: `UNKNOWN_HOST_KEY:{fingerprint}|{key_type}|{host}:{port}`
/// — the fingerprint is hex and the key type contains no `|`, so the first two
/// `|` delimiters are unambiguous (IPv6 hosts may contain `:`).
pub const UNKNOWN_HOST_KEY_PREFIX: &str = "UNKNOWN_HOST_KEY:";

const KNOWN_KEY_TYPES: &[&str] = &[
    "ssh-rsa",
    "ssh-dss",
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp521",
    "ssh-ed25519",
    "unknown",
];

/// One known_hosts entry: either the new per-key-type map or the legacy flat
/// fingerprint recorded before key types were tracked.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum KnownHostEntry {
    PerType(BTreeMap<String, String>),
    Legacy(String),
}

type KnownHosts = HashMap<String, KnownHostEntry>;

#[derive(Debug, PartialEq)]
enum HostKeyDecision {
    Trusted,
    /// Legacy flat fingerprint matched; rewrite it under the negotiated key type.
    TrustedMigrateLegacy,
    Mismatch {
        expected: String,
    },
    Unknown,
}

fn host_key_type_name(key_type: HostKeyType) -> &'static str {
    match key_type {
        HostKeyType::Rsa => "ssh-rsa",
        HostKeyType::Dss => "ssh-dss",
        HostKeyType::Ecdsa256 => "ecdsa-sha2-nistp256",
        HostKeyType::Ecdsa384 => "ecdsa-sha2-nistp384",
        HostKeyType::Ecdsa521 => "ecdsa-sha2-nistp521",
        HostKeyType::Ed25519 => "ssh-ed25519",
        HostKeyType::Unknown => "unknown",
    }
}

fn evaluate_host_key(
    entry: Option<&KnownHostEntry>,
    key_type: &str,
    fingerprint: &str,
) -> HostKeyDecision {
    match entry {
        Some(KnownHostEntry::PerType(fingerprints)) => match fingerprints.get(key_type) {
            Some(expected) if expected == fingerprint => HostKeyDecision::Trusted,
            Some(expected) => HostKeyDecision::Mismatch {
                expected: expected.clone(),
            },
            // A key type we have never seen for this host (e.g. the server or
            // client started preferring a different algorithm) is a
            // trust-on-first-use case, not a mismatch.
            None => HostKeyDecision::Unknown,
        },
        Some(KnownHostEntry::Legacy(expected)) if expected == fingerprint => {
            HostKeyDecision::TrustedMigrateLegacy
        }
        // A legacy entry recorded no key type, so a differing fingerprint may
        // simply be another key of the same host — prompt instead of blocking.
        Some(KnownHostEntry::Legacy(_)) => HostKeyDecision::Unknown,
        None => HostKeyDecision::Unknown,
    }
}

#[tauri::command(async)]
pub fn trust_host_key(
    host: String,
    port: u16,
    key_type: String,
    fingerprint: String,
) -> Result<(), String> {
    let host_key = format!("{}:{}", host.trim().to_lowercase(), port);
    let key_type = key_type.trim().to_string();
    let fingerprint = fingerprint.trim().to_lowercase();
    if fingerprint.is_empty() || !fingerprint.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("Invalid host key fingerprint".to_string());
    }
    if !KNOWN_KEY_TYPES.contains(&key_type.as_str()) {
        return Err(format!("Unsupported host key type: {}", key_type));
    }
    let mut known_hosts = read_known_hosts()?;
    match known_hosts.get_mut(&host_key) {
        Some(KnownHostEntry::PerType(fingerprints)) => {
            fingerprints.insert(key_type, fingerprint);
        }
        _ => {
            // Absent or legacy: the user just explicitly trusted this typed
            // fingerprint, so it replaces the untyped record.
            let mut fingerprints = BTreeMap::new();
            fingerprints.insert(key_type, fingerprint);
            known_hosts.insert(host_key, KnownHostEntry::PerType(fingerprints));
        }
    }
    write_known_hosts(&known_hosts)
}

fn verify_known_host(session: &Session, target: &SshTarget) -> Result<(), String> {
    let fingerprint = session
        .host_key_hash(HashType::Sha256)
        .map(bytes_to_hex)
        .ok_or_else(|| "Unable to read remote host key fingerprint".to_string())?;
    let key_type = session
        .host_key()
        .map(|(_, key_type)| host_key_type_name(key_type))
        .ok_or_else(|| "Unable to determine remote host key type".to_string())?;
    let host_key = format!("{}:{}", target.host.to_lowercase(), target.port);
    let mut known_hosts = read_known_hosts()?;

    match evaluate_host_key(known_hosts.get(&host_key), key_type, &fingerprint) {
        HostKeyDecision::Trusted => Ok(()),
        HostKeyDecision::TrustedMigrateLegacy => {
            let mut fingerprints = BTreeMap::new();
            fingerprints.insert(key_type.to_string(), fingerprint);
            known_hosts.insert(host_key, KnownHostEntry::PerType(fingerprints));
            // The connection is already verified; a failed rewrite must not
            // block it. The migration will be retried on the next connect.
            let _ = write_known_hosts(&known_hosts);
            Ok(())
        }
        HostKeyDecision::Mismatch { expected } => Err(format!(
            "Host key mismatch for {} ({}). Expected {}, got {}. Inspect known_hosts.json before reconnecting.",
            host_key, key_type, expected, fingerprint
        )),
        HostKeyDecision::Unknown => Err(format!(
            "{}{}|{}|{}",
            UNKNOWN_HOST_KEY_PREFIX, fingerprint, key_type, host_key
        )),
    }
}

fn normalize_profile(mut profile: SessionProfile) -> SessionProfile {
    profile.id = if profile.id.trim().is_empty() {
        Uuid::new_v4().to_string()
    } else {
        profile.id.trim().to_string()
    };
    profile.host = profile.host.trim().to_string();
    profile.username = profile.username.trim().to_string();
    if profile.is_local {
        profile.host = "localhost".to_string();
        profile.port = 0;
        profile.username = "local".to_string();
        profile.private_key_path = None;
        profile.proxy_jump_id = None;
    }
    profile.private_key_path = normalize_optional_string(profile.private_key_path);
    profile.proxy_jump_id = normalize_optional_string(profile.proxy_jump_id)
        // A profile cannot be its own jump host.
        .filter(|proxy_id| proxy_id != &profile.id);
    if profile.name.trim().is_empty() && profile.is_local {
        profile.name = "Local terminal".to_string();
    } else if profile.name.trim().is_empty() {
        profile.name = format!("{}@{}", profile.username, profile.host);
    } else {
        profile.name = profile.name.trim().to_string();
    }
    profile
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
}

fn read_profiles() -> Result<Vec<SessionProfile>, String> {
    let path = sessions_path();
    if !path.exists() {
        return Ok(Vec::new());
    }

    let content = fs::read_to_string(&path)
        .map_err(|error| format!("Failed to read sessions file {}: {}", path.display(), error))?;
    serde_json::from_str(&content).map_err(|error| {
        format!(
            "Failed to parse sessions file {}: {}",
            path.display(),
            error
        )
    })
}

fn write_profiles(profiles: &[SessionProfile]) -> Result<(), String> {
    crate::ssh::credentials::write_json_file(&sessions_path(), &profiles, "sessions file")
}

fn read_known_hosts() -> Result<KnownHosts, String> {
    let path = known_hosts_path();
    if !path.exists() {
        return Ok(KnownHosts::new());
    }

    let content = fs::read_to_string(&path).map_err(|error| {
        format!(
            "Failed to read known_hosts file {}: {}",
            path.display(),
            error
        )
    })?;
    serde_json::from_str(&content).map_err(|error| {
        format!(
            "Failed to parse known_hosts file {}: {}",
            path.display(),
            error
        )
    })
}

fn write_known_hosts(known_hosts: &KnownHosts) -> Result<(), String> {
    crate::ssh::credentials::write_json_file(&known_hosts_path(), known_hosts, "known_hosts file")
}

fn sessions_path() -> PathBuf {
    config_dir().join("sessions.json")
}

fn known_hosts_path() -> PathBuf {
    config_dir().join("known_hosts.json")
}

pub(crate) fn config_dir() -> PathBuf {
    if cfg!(target_os = "windows") {
        if let Some(appdata) = env::var_os("APPDATA") {
            return PathBuf::from(appdata).join("GpuTerm");
        }
    }

    if cfg!(target_os = "macos") {
        if let Some(home) = env::var_os("HOME") {
            return PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("GpuTerm");
        }
    }

    if let Some(xdg_config_home) = env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg_config_home).join("GpuTerm");
    }

    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home).join(".config").join("GpuTerm");
    }

    env::temp_dir().join("GpuTerm")
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(&mut output, "{:02x}", byte);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh::credentials::MemoryCredentialStore;

    fn per_type(pairs: &[(&str, &str)]) -> KnownHostEntry {
        KnownHostEntry::PerType(
            pairs
                .iter()
                .map(|(key_type, fp)| (key_type.to_string(), fp.to_string()))
                .collect(),
        )
    }

    #[test]
    fn the_eagain_constant_matches_what_ssh2_itself_calls_would_block() {
        // `open_direct_channel`'s retry loop hinges on this number. If ssh2 ever
        // changes it, a tunnel would bind a port and silently never serve, so
        // pin it against ssh2's own io::ErrorKind mapping rather than trusting
        // the literal.
        let eagain = ssh2::Error::from_errno(ssh2::ErrorCode::Session(LIBSSH2_ERROR_EAGAIN));
        assert_eq!(
            std::io::Error::from(ssh2::Error::from_errno(ssh2::ErrorCode::Session(
                LIBSSH2_ERROR_EAGAIN
            )))
            .kind(),
            ErrorKind::WouldBlock
        );
        assert!(is_would_block(&eagain));

        // A real failure must not be mistaken for backpressure, or the loop
        // would spin until its deadline instead of reporting the refusal.
        let refused = ssh2::Error::from_errno(ssh2::ErrorCode::Session(-32));
        assert!(!is_would_block(&refused));
    }

    #[test]
    fn a_signature_ignores_the_password_but_tracks_every_hop() {
        let base = SshTarget {
            session_id: "s".to_string(),
            host: "Host.Example".to_string(),
            port: 22,
            username: "user".to_string(),
            password: Some("hunter2".to_string()),
            private_key_path: None,
            proxy: None,
        };
        let signature = target_signature(&base);
        // Unlocking the vault must not tear down a healthy connection.
        assert!(!signature.contains("hunter2"), "{signature}");
        assert!(signature.contains("host.example:22"), "{signature}");

        let mut without_password = base.clone();
        without_password.password = None;
        assert_eq!(target_signature(&without_password), signature);

        // Anything that changes where the connection lands must change it.
        let mut other_port = base.clone();
        other_port.port = 2222;
        assert_ne!(target_signature(&other_port), signature);

        let mut with_key = base.clone();
        with_key.private_key_path = Some("/keys/id_ed25519".to_string());
        assert_ne!(target_signature(&with_key), signature);

        let mut jumped = base.clone();
        jumped.proxy = Some(Box::new(SshTarget {
            session_id: "b".to_string(),
            host: "bastion".to_string(),
            port: 22,
            username: "jump".to_string(),
            password: Some("also-secret".to_string()),
            private_key_path: None,
            proxy: None,
        }));
        let jumped_signature = target_signature(&jumped);
        assert_ne!(jumped_signature, signature);
        assert!(!jumped_signature.contains("also-secret"), "{jumped_signature}");
    }

    #[test]
    fn trusts_matching_per_type_fingerprint() {
        let entry = per_type(&[("ssh-ed25519", "abc123")]);
        assert_eq!(
            evaluate_host_key(Some(&entry), "ssh-ed25519", "abc123"),
            HostKeyDecision::Trusted
        );
    }

    #[test]
    fn rejects_differing_fingerprint_for_same_key_type() {
        let entry = per_type(&[("ssh-ed25519", "abc123")]);
        assert_eq!(
            evaluate_host_key(Some(&entry), "ssh-ed25519", "def456"),
            HostKeyDecision::Mismatch {
                expected: "abc123".to_string()
            }
        );
    }

    #[test]
    fn treats_new_key_type_as_unknown_not_mismatch() {
        // The RSA -> ECDSA backend-switch incident: same host, key type never
        // recorded before, must prompt instead of blocking.
        let entry = per_type(&[("ssh-rsa", "abc123")]);
        assert_eq!(
            evaluate_host_key(Some(&entry), "ecdsa-sha2-nistp256", "def456"),
            HostKeyDecision::Unknown
        );
    }

    #[test]
    fn migrates_matching_legacy_fingerprint() {
        let entry = KnownHostEntry::Legacy("abc123".to_string());
        assert_eq!(
            evaluate_host_key(Some(&entry), "ssh-rsa", "abc123"),
            HostKeyDecision::TrustedMigrateLegacy
        );
    }

    #[test]
    fn treats_differing_legacy_fingerprint_as_unknown() {
        let entry = KnownHostEntry::Legacy("abc123".to_string());
        assert_eq!(
            evaluate_host_key(Some(&entry), "ecdsa-sha2-nistp256", "def456"),
            HostKeyDecision::Unknown
        );
    }

    #[test]
    fn treats_missing_entry_as_unknown() {
        assert_eq!(
            evaluate_host_key(None, "ssh-ed25519", "abc123"),
            HostKeyDecision::Unknown
        );
    }

    fn profile(id: &str, proxy: Option<&str>) -> SessionProfile {
        SessionProfile {
            id: id.to_string(),
            name: id.to_string(),
            host: format!("{}.example", id),
            port: 22,
            username: "user".to_string(),
            is_local: false,
            private_key_path: None,
            proxy_jump_id: proxy.map(str::to_string),
        }
    }

    #[test]
    fn normalizes_local_profiles_without_ssh_credentials() {
        let mut local = profile("local", Some("jump"));
        local.name.clear();
        local.host = "ignored.example".to_string();
        local.port = 22;
        local.username = "ignored".to_string();
        local.is_local = true;
        local.private_key_path = Some("/tmp/key".to_string());

        let local = normalize_profile(local);
        assert_eq!(local.name, "Local terminal");
        assert_eq!(local.host, "localhost");
        assert_eq!(local.port, 0);
        assert_eq!(local.username, "local");
        assert!(local.private_key_path.is_none());
        assert!(local.proxy_jump_id.is_none());
    }

    fn no_password(_id: &str) -> Option<String> {
        None
    }

    #[test]
    fn resolves_direct_and_single_and_multi_hop_chains() {
        let profiles = vec![
            profile("target", Some("mid")),
            profile("mid", Some("edge")),
            profile("edge", None),
        ];
        assert!(
            build_proxy_target("edge", &profiles, &mut Vec::new(), &no_password)
                .unwrap()
                .unwrap()
                .proxy
                .is_none()
        );

        let chain = build_proxy_target("mid", &profiles, &mut Vec::new(), &no_password)
            .unwrap()
            .unwrap();
        assert_eq!(chain.host, "mid.example");
        assert_eq!(chain.proxy.as_ref().unwrap().host, "edge.example");
        assert!(chain.password.is_none());
    }

    #[test]
    fn fills_hop_password_from_provider() {
        let profiles = vec![profile("edge", None)];
        let password_for = |id: &str| (id == "edge").then(|| "s3cret".to_string());
        let chain = build_proxy_target("edge", &profiles, &mut Vec::new(), &password_for)
            .unwrap()
            .unwrap();
        assert_eq!(chain.password.as_deref(), Some("s3cret"));
    }

    #[test]
    fn restores_saved_credentials_for_connection_tests() {
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
            username: "user".to_string(),
            password: None,
            private_key_path: None,
            proxy: Some(Box::new(SshTarget {
                session_id: "jump".to_string(),
                host: "jump.example".to_string(),
                port: 22,
                username: "user".to_string(),
                password: None,
                private_key_path: None,
                proxy: None,
            })),
        };

        restore_stored_credentials(&mut target, &credentials).unwrap();

        assert_eq!(target.password.as_deref(), Some("target-secret"));
        assert_eq!(
            target.proxy.as_ref().unwrap().password.as_deref(),
            Some("jump-secret")
        );
    }

    #[test]
    fn rejects_missing_cycle_and_too_deep_chains() {
        let missing = vec![profile("a", Some("ghost"))];
        assert!(
            build_proxy_target("a", &missing, &mut Vec::new(), &no_password)
                .unwrap_err()
                .contains("not found")
        );

        let self_cycle = vec![profile("a", Some("a"))];
        assert!(
            build_proxy_target("a", &self_cycle, &mut Vec::new(), &no_password)
                .unwrap_err()
                .contains("loop")
        );

        let mutual = vec![profile("a", Some("b")), profile("b", Some("a"))];
        assert!(
            build_proxy_target("a", &mutual, &mut Vec::new(), &no_password)
                .unwrap_err()
                .contains("loop")
        );

        let deep = vec![
            profile("a", Some("b")),
            profile("b", Some("c")),
            profile("c", Some("d")),
            profile("d", None),
        ];
        assert!(
            build_proxy_target("a", &deep, &mut Vec::new(), &no_password)
                .unwrap_err()
                .contains("too deep")
        );
    }

    #[test]
    fn round_trips_mixed_legacy_and_typed_known_hosts() {
        let json = r#"{
            "old.example:22": "aabbcc",
            "new.example:22": { "ssh-ed25519": "ddeeff", "ssh-rsa": "112233" }
        }"#;
        let parsed: KnownHosts = serde_json::from_str(json).unwrap();
        assert!(matches!(
            parsed.get("old.example:22"),
            Some(KnownHostEntry::Legacy(fp)) if fp == "aabbcc"
        ));
        assert!(matches!(
            parsed.get("new.example:22"),
            Some(KnownHostEntry::PerType(map)) if map.len() == 2
        ));

        let serialized = serde_json::to_string(&parsed).unwrap();
        let reparsed: KnownHosts = serde_json::from_str(&serialized).unwrap();
        assert_eq!(reparsed.len(), 2);
    }
}
