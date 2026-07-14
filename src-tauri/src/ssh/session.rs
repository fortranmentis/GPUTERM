use crate::ssh::credentials::{CredentialStore, MemoryCredentialStore};
use crate::ssh::system_monitor::SystemMonitorSettings;
use crate::ssh::terminal::TerminalHandle;
use serde::{Deserialize, Serialize};
use ssh2::{HashType, Session};
use std::collections::HashMap;
use std::env;
use std::fmt::Write as FmtWrite;
use std::fs;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

/// Shared per-session SSH connections reused for short operations
/// (directory listings, stat calls, resource details). `ssh2::Session` is
/// internally synchronized but operations on one session must be serialized,
/// hence the per-entry mutex.
pub type OpsSessions = Arc<Mutex<HashMap<String, Arc<Mutex<Session>>>>>;

#[derive(Default)]
pub struct AppState {
    pub terminals: Mutex<HashMap<String, TerminalHandle>>,
    pub active_connections: Mutex<HashMap<String, ActiveConnection>>,
    pub telemetry_stops: Mutex<HashMap<String, Arc<AtomicBool>>>,
    pub telemetry_settings: Arc<Mutex<SystemMonitorSettings>>,
    pub credentials: MemoryCredentialStore,
    pub ops_sessions: OpsSessions,
    pub transfer_cancels: Mutex<HashMap<String, Arc<AtomicBool>>>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_key_path: Option<String>,
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
    pub password: Option<String>,
    #[serde(default)]
    pub private_key_path: Option<String>,
    #[serde(default)]
    pub cols: Option<u32>,
    #[serde(default)]
    pub rows: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSessionInfo {
    pub session_id: String,
    pub profile: SessionProfile,
}

#[derive(Clone)]
pub struct SshTarget {
    pub session_id: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: Option<String>,
    pub private_key_path: Option<String>,
}

#[tauri::command]
pub fn load_sessions() -> Result<Vec<SessionProfile>, String> {
    read_profiles()
}

#[tauri::command]
pub fn save_session(profile: SessionProfile) -> Result<Vec<SessionProfile>, String> {
    upsert_profile(normalize_profile(profile))
}

#[tauri::command]
pub fn delete_session(id: String) -> Result<Vec<SessionProfile>, String> {
    let mut profiles = read_profiles()?;
    profiles.retain(|profile| profile.id != id);
    write_profiles(&profiles)?;
    Ok(profiles)
}

#[tauri::command]
pub async fn test_ssh_connection(request: SessionConnectRequest) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let profile = profile_from_request(&request);
        let target = target_from_request(&profile, &request);
        let session = open_ssh_session(&target)?;
        let _ = session.disconnect(None, "GpuTerm connection test complete", None);
        Ok(format!("SSH connection to {}:{} succeeded", profile.host, profile.port))
    })
    .await
    .map_err(|error| format!("Connection test task failed: {}", error))?
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
        if let Ok(session) = entry.lock() {
            session.set_timeout(timeout_ms);
            // Cheap liveness probe before running a possibly non-idempotent
            // operation: opening a channel round-trips to the server.
            let alive = match session.channel_session() {
                Ok(mut channel) => {
                    let _ = channel.close();
                    true
                }
                Err(_) => false,
            };
            if alive {
                return f(&session);
            }
        }
        if let Ok(mut map) = ops.lock() {
            map.remove(&target.session_id);
        }
    }

    let session = open_ssh_session(target)?;
    session.set_keepalive(true, 15);
    session.set_timeout(timeout_ms);
    let entry = Arc::new(Mutex::new(session));
    {
        let mut map = ops
            .lock()
            .map_err(|_| "Operations connection state is unavailable".to_string())?;
        map.insert(target.session_id.clone(), Arc::clone(&entry));
    }
    let session = entry
        .lock()
        .map_err(|_| "Operations connection is unavailable".to_string())?;
    f(&session)
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
        private_key_path: normalize_optional_string(request.private_key_path.clone()),
    })
}

pub fn target_from_request(profile: &SessionProfile, request: &SessionConnectRequest) -> SshTarget {
    SshTarget {
        session_id: profile.id.clone(),
        host: profile.host.clone(),
        port: profile.port,
        username: profile.username.clone(),
        password: normalize_optional_string(request.password.clone()),
        private_key_path: profile.private_key_path.clone(),
    }
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

    Ok(SshTarget {
        session_id: profile.id.clone(),
        host: profile.host,
        port: profile.port,
        username: profile.username,
        password: state.credentials.get_password(session_id),
        private_key_path: profile.private_key_path,
    })
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

pub fn open_ssh_session(target: &SshTarget) -> Result<Session, String> {
    let address = format!("{}:{}", target.host, target.port);
    let socket_addr = address
        .to_socket_addrs()
        .map_err(|error| format!("Network resolution failed for {}: {}", address, error))?
        .next()
        .ok_or_else(|| format!("No network address found for {}", address))?;

    let tcp = TcpStream::connect_timeout(&socket_addr, Duration::from_secs(10))
        .map_err(|error| format!("Network timeout or connection failure for {}: {}", address, error))?;
    let _ = tcp.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = tcp.set_write_timeout(Some(Duration::from_secs(10)));

    let mut session = Session::new().map_err(|error| format!("Failed to create SSH session: {}", error))?;
    session.set_tcp_stream(tcp);
    session.set_timeout(10_000);
    session
        .handshake()
        .map_err(|error| format!("SSH handshake failed for {}: {}", address, error))?;

    verify_known_host(&session, target)?;
    authenticate(&session, target)?;

    if !session.authenticated() {
        return Err("SSH authentication failed. Check username, password, private key, or SSH agent.".to_string());
    }

    Ok(session)
}

fn authenticate(session: &Session, target: &SshTarget) -> Result<(), String> {
    if let Some(private_key_path) = normalize_optional_string(target.private_key_path.clone()) {
        let private_key = Path::new(&private_key_path);
        if !private_key.exists() {
            return Err(format!("Private key file was not found: {}", private_key_path));
        }

        session
            .userauth_pubkey_file(
                &target.username,
                None,
                private_key,
                target.password.as_deref(),
            )
            .map_err(|_| "SSH key authentication failed. Check username, private key, and passphrase.".to_string())?;
        return Ok(());
    }

    if let Some(password) = normalize_optional_string(target.password.clone()) {
        session
            .userauth_password(&target.username, &password)
            .map_err(|_| "SSH password authentication failed. Check username and password.".to_string())?;
        return Ok(());
    }

    session
        .userauth_agent(&target.username)
        .map_err(|_| "SSH agent authentication failed. Enter a password or private key path.".to_string())
}

/// Prefix of the sentinel error raised when connecting to a host whose key has
/// never been seen. Format: `UNKNOWN_HOST_KEY:{fingerprint}|{host}:{port}` —
/// the fingerprint is hex so the first `|` separates it from the host key.
pub const UNKNOWN_HOST_KEY_PREFIX: &str = "UNKNOWN_HOST_KEY:";

#[tauri::command]
pub fn trust_host_key(host: String, port: u16, fingerprint: String) -> Result<(), String> {
    let host_key = format!("{}:{}", host.trim().to_lowercase(), port);
    let fingerprint = fingerprint.trim().to_lowercase();
    if fingerprint.is_empty() || !fingerprint.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("Invalid host key fingerprint".to_string());
    }
    let mut known_hosts = read_known_hosts()?;
    known_hosts.insert(host_key, fingerprint);
    write_known_hosts(&known_hosts)
}

fn verify_known_host(session: &Session, target: &SshTarget) -> Result<(), String> {
    let fingerprint = session
        .host_key_hash(HashType::Sha256)
        .map(bytes_to_hex)
        .ok_or_else(|| "Unable to read remote host key fingerprint".to_string())?;
    let host_key = format!("{}:{}", target.host.to_lowercase(), target.port);
    let known_hosts = read_known_hosts()?;

    match known_hosts.get(&host_key) {
        Some(existing) if existing != &fingerprint => Err(format!(
            "Host key mismatch for {}. Expected {}, got {}. Inspect known_hosts.json before reconnecting.",
            host_key, existing, fingerprint
        )),
        Some(_) => Ok(()),
        None => Err(format!(
            "{}{}|{}",
            UNKNOWN_HOST_KEY_PREFIX, fingerprint, host_key
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
    profile.private_key_path = normalize_optional_string(profile.private_key_path);
    if profile.name.trim().is_empty() {
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
    serde_json::from_str(&content)
        .map_err(|error| format!("Failed to parse sessions file {}: {}", path.display(), error))
}

fn write_profiles(profiles: &[SessionProfile]) -> Result<(), String> {
    let path = sessions_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create config directory {}: {}", parent.display(), error))?;
    }
    let content = serde_json::to_string_pretty(profiles)
        .map_err(|error| format!("Failed to serialize sessions: {}", error))?;
    fs::write(&path, content)
        .map_err(|error| format!("Failed to write sessions file {}: {}", path.display(), error))
}

fn read_known_hosts() -> Result<HashMap<String, String>, String> {
    let path = known_hosts_path();
    if !path.exists() {
        return Ok(HashMap::new());
    }

    let content = fs::read_to_string(&path)
        .map_err(|error| format!("Failed to read known_hosts file {}: {}", path.display(), error))?;
    serde_json::from_str(&content)
        .map_err(|error| format!("Failed to parse known_hosts file {}: {}", path.display(), error))
}

fn write_known_hosts(known_hosts: &HashMap<String, String>) -> Result<(), String> {
    let path = known_hosts_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create config directory {}: {}", parent.display(), error))?;
    }
    let content = serde_json::to_string_pretty(known_hosts)
        .map_err(|error| format!("Failed to serialize known_hosts: {}", error))?;
    fs::write(&path, content)
        .map_err(|error| format!("Failed to write known_hosts file {}: {}", path.display(), error))
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
