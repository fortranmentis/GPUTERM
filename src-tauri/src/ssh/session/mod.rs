mod known_hosts;
#[cfg(test)]
mod tests;
mod tunnel;
mod types;

#[allow(unused_imports)]
pub use known_hosts::{
    host_keys_trusted, is_host_key_sentinel, readable_connect_error, HOST_KEY_CHANGED_PREFIX,
    UNKNOWN_HOST_KEY_PREFIX,
};
#[allow(unused_imports)]
pub use tunnel::{open_forwarding_session, open_persistent_forward, PersistentForward};
#[allow(unused_imports)]
pub(crate) use tunnel::{
    classify_probe, connection_answers, is_would_block, open_direct_channel, probe_reply,
    pump_channel_socket, try_reserve_slot, CountGuard, IdleBackoff, ProbeFailure, ProbeOutcome,
    HOP_PROBE_TIMEOUT,
};
#[allow(unused_imports)]
pub(crate) use types::AuthMethod;
#[allow(unused_imports)]
pub use types::{
    ActiveConnection, AgentQuotaRefreshes, AppState, OpsSessions, SessionConnectRequest,
    SessionProfile, SshConnection, SshTarget, TerminalSessionInfo,
};

use crate::llm::instance::LlmInstance;
use crate::ssh::credentials::{CredentialStore, CredentialVaultStatus, SecureCredentialStore};
use crate::ssh::kbdint::{
    asks_for_codes, AuthAbort, AuthResponder, HopInfo, NonInteractive, PromptAdapter,
    INTERACTIVE_AUTH_PREFIX,
};
use known_hosts::{trust_host_key_entry, verify_known_host};
use ssh2::Session;
use std::collections::HashMap;
use std::env;
use std::fmt::Write as FmtWrite;
use std::fs;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::State;
use tunnel::{enable_tcp_keepalive, open_tunneled_stream};
use uuid::Uuid;

/// Serializes read-modify-write of `sessions.json` so a delete cannot
/// interleave with a concurrent save and lose one of the two edits.
static PROFILE_WRITE_LOCK: Mutex<()> = Mutex::new(());

fn lock_profiles() -> Result<std::sync::MutexGuard<'static, ()>, String> {
    PROFILE_WRITE_LOCK
        .lock()
        .map_err(|_| "Session storage is unavailable".to_string())
}

#[tauri::command(async)]
pub fn load_sessions() -> Result<Vec<SessionProfile>, String> {
    read_profiles()
}

/// Saves a new or edited profile. A non-empty `password` (and
/// `proxy_jump_password`, for the jump host) goes to the vault; `None` or an
/// empty string leaves any stored password untouched, so an edit does not
/// have to re-enter it.
#[tauri::command]
pub async fn save_session(
    state: State<'_, AppState>,
    profile: SessionProfile,
    password: Option<String>,
    proxy_jump_password: Option<String>,
) -> Result<Vec<SessionProfile>, String> {
    let credentials = state.credentials.clone();
    tauri::async_runtime::spawn_blocking(move || {
        save_profile_and_credentials(
            normalize_profile(profile),
            password,
            proxy_jump_password,
            &credentials,
            upsert_profile,
        )
    })
    .await
    .map_err(|error| format!("Saving the session failed: {}", error))?
}

/// Stores the passwords first, so a locked vault fails the save before the
/// profile is written rather than leaving a profile whose typed password was
/// silently dropped.
fn save_profile_and_credentials(
    profile: SessionProfile,
    password: Option<String>,
    proxy_jump_password: Option<String>,
    credentials: &impl CredentialStore,
    upsert: impl FnOnce(SessionProfile) -> Result<Vec<SessionProfile>, String>,
) -> Result<Vec<SessionProfile>, String> {
    let non_empty = |value: Option<String>| value.filter(|value| !value.is_empty());
    if !profile.is_local {
        if let Some(password) = non_empty(password) {
            credentials.set_password(&profile.id, password)?;
        }
        if let (Some(jump_id), Some(password)) =
            (profile.proxy_jump_id.as_deref(), non_empty(proxy_jump_password))
        {
            credentials.set_password(jump_id, password)?;
        }
    }
    upsert(profile)
}

/// Replaces a profile's saved port forward rules. Running forwards are not
/// touched; the UI restarts them explicitly.
#[tauri::command(async)]
pub fn save_profile_forwards(
    profile_id: String,
    forwards: Vec<crate::ssh::forwarding::ForwardSpec>,
) -> Result<Vec<SessionProfile>, String> {
    let forwards = forwards
        .into_iter()
        .map(crate::ssh::forwarding::validate_spec)
        .collect::<Result<Vec<_>, _>>()?;
    let _write_guard = lock_profiles()?;
    let mut profiles = read_profiles()?;
    let profile = profiles
        .iter_mut()
        .find(|profile| profile.id == profile_id)
        .ok_or_else(|| "That session profile no longer exists".to_string())?;
    if profile.is_local {
        return Err("A local terminal cannot forward ports".to_string());
    }
    profile.forwards = forwards;
    write_profiles(&profiles)?;
    Ok(profiles)
}

#[tauri::command]
pub async fn delete_session(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<Vec<SessionProfile>, String> {
    let credentials = state.credentials.clone();
    let deleted_id = id.clone();
    let profiles = tauri::async_runtime::spawn_blocking(move || {
        let _write_guard = lock_profiles()?;
        let mut profiles = read_profiles()?;
        ensure_profile_unreferenced(
            &id,
            &profiles,
            &crate::llm::instance::read_instances()?,
            &crate::ssh::db_monitor::list_db_instances()?,
        )?;
        profiles.retain(|profile| profile.id != id);
        write_profiles(&profiles)?;
        crate::mcp::prune_hidden_profile(&app, &id);
        // Vault clean-up is best effort: a locked or not-yet-initialized vault
        // must not make profile deletion impossible. Any leftover entry is
        // unreachable once the profile is gone.
        for key in [id.clone(), crate::ssh::managed_key::managed_key_vault_key(&id)] {
            if let Err(error) = credentials.clear_password(&key) {
                eprintln!(
                    "GpuTerm: could not clear stored credential for a deleted profile: {error}"
                );
            }
        }
        Ok::<_, String>(profiles)
    })
    .await
    .map_err(|error| format!("Session deletion task failed: {}", error))??;
    // Only after the delete succeeded: a refused delete must leave the live
    // session (possibly a bastion other sessions ride on) untouched.
    crate::ssh::terminal::teardown_session(&state, &deleted_id);
    Ok(profiles)
}

/// Refuses to delete a profile that another profile uses as its jump host, an
/// LLM instance tunnels through, or a database monitor relies on; deleting it
/// would leave dangling ids.
fn ensure_profile_unreferenced(
    id: &str,
    profiles: &[SessionProfile],
    instances: &[LlmInstance],
    db_instances: &[crate::ssh::db_monitor::DbInstance],
) -> Result<(), String> {
    let dependents = profiles
        .iter()
        .filter(|profile| profile.id != id && profile.proxy_jump_id.as_deref() == Some(id))
        .map(|profile| profile.name.clone())
        .collect::<Vec<_>>();
    if !dependents.is_empty() {
        return Err(format!(
            "This session is the jump host for {}. Change their jump host before deleting it.",
            dependents.join(", ")
        ));
    }
    let instances = instances
        .iter()
        .filter(|instance| instance.ssh_profile_id.as_deref() == Some(id))
        .map(|instance| instance.name.clone())
        .collect::<Vec<_>>();
    if !instances.is_empty() {
        return Err(format!(
            "The LLM runtime(s) {} tunnel through this session. Change or remove them before deleting it.",
            instances.join(", ")
        ));
    }
    let databases = db_instances
        .iter()
        .filter(|instance| instance.profile_id == id)
        .map(|instance| instance.name.clone())
        .collect::<Vec<_>>();
    if !databases.is_empty() {
        return Err(format!(
            "The database monitor(s) {} use this session. Remove them before deleting it.",
            databases.join(", ")
        ));
    }
    Ok(())
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
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    request: SessionConnectRequest,
) -> Result<String, String> {
    let credentials = state.credentials.clone();
    let mut auth = crate::ssh::terminal::sign_in_responder(
        &app,
        &state,
        request.id.as_deref().unwrap_or_default(),
        request.allow_interactive_auth,
    );
    tauri::async_runtime::spawn_blocking(move || {
        let profile = profile_from_request(&request);
        if profile.is_local {
            return Ok("Local terminal is available".to_string());
        }
        let mut target = target_from_request(&profile, &request)?;
        if request.reuse_stored_credentials {
            restore_stored_credentials(&mut target, &credentials)?;
        }
        crate::ssh::managed_key::attach_managed_keys(&mut target, &credentials);
        let connection = open_ssh_session_with(&target, auth.as_mut())?;
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

#[tauri::command(async)]
pub fn trust_host_key(
    host: String,
    port: u16,
    key_type: String,
    fingerprint: String,
) -> Result<(), String> {
    trust_host_key_entry(host, port, key_type, fingerprint)
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
    let slot = {
        let mut map = ops
            .lock()
            .map_err(|_| "Operations connection state is unavailable".to_string())?;
        Arc::clone(
            map.entry(target.session_id.clone())
                .or_insert_with(|| Arc::new(Mutex::new(None))),
        )
    };

    let mut guard = slot
        .lock()
        .map_err(|_| "Operations connection is unavailable".to_string())?;

    if let Some(connection) = guard.as_ref() {
        connection.session.set_timeout(timeout_ms);
        // Cheap liveness probe before running a possibly non-idempotent
        // operation: it needs a reply from the server, so a dead tunnel
        // fails here and is replaced below.
        if connection_answers(connection, Duration::from_millis(timeout_ms.into())) {
            return f(&connection.session);
        }
        *guard = None;
    }

    let connection = match open_ssh_session(target) {
        Ok(connection) => connection,
        Err(error) => {
            // Only evict the empty slot if the map still points to this exact
            // entry and no other caller is waiting on it.
            if let Ok(mut map) = ops.lock() {
                if map
                    .get(&target.session_id)
                    .is_some_and(|current| Arc::ptr_eq(current, &slot))
                    && Arc::strong_count(&slot) == 2
                {
                    map.remove(&target.session_id);
                }
            }
            return Err(error);
        }
    };
    connection.session.set_keepalive(true, 15);
    connection.session.set_timeout(timeout_ms);
    let connection = guard.insert(connection);
    f(&connection.session)
}

pub fn drop_ops_session(ops: &OpsSessions, session_id: &str) {
    if let Ok(mut map) = ops.lock() {
        map.remove(session_id);
    }
}

pub fn drop_all_ops_sessions(ops: &OpsSessions) {
    if let Ok(mut map) = ops.lock() {
        map.clear();
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
        // Kept from the stored profile by `upsert_profile`.
        forwards: Vec::new(),
        managed_key: None,
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
        managed_key: None,
        assigned_key: None,
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
    let mut target = SshTarget {
        session_id: profile.id.clone(),
        host: profile.host,
        port: profile.port,
        username: profile.username,
        password: state.credentials.get_password(session_id).ok().flatten(),
        private_key_path: profile.private_key_path,
        managed_key: None,
        assigned_key: None,
        proxy,
    };
    crate::ssh::managed_key::attach_managed_keys(&mut target, &state.credentials);
    Ok(target)
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
        managed_key: None,
        assigned_key: None,
        proxy,
    })))
}

pub fn upsert_profile(profile: SessionProfile) -> Result<Vec<SessionProfile>, String> {
    let _write_guard = lock_profiles()?;
    let mut profiles = read_profiles()?;
    let id = profile.id.clone();
    merge_profile(&mut profiles, profile);
    // Editing a profile can close a loop the UI's candidate filter missed
    // (say, another window changed a jump host meanwhile); refuse it here.
    ensure_no_jump_loop(&id, &profiles)?;
    profiles.sort_by_key(|profile| profile.name.to_lowercase());
    write_profiles(&profiles)?;
    Ok(profiles)
}

/// Replaces the profile with the same id, or adds it. Forward rules are kept:
/// they are edited only through `save_profile_forwards`.
fn merge_profile(profiles: &mut Vec<SessionProfile>, mut profile: SessionProfile) {
    if let Some(existing) = profiles.iter_mut().find(|item| item.id == profile.id) {
        profile.forwards = std::mem::take(&mut existing.forwards);
        if !profile.is_local {
            profile.managed_key = existing.managed_key.take();
        }
        *existing = profile;
    } else {
        profiles.push(profile);
    }
}

/// Records (or, with None, forgets) a profile's managed key.
pub(crate) fn set_managed_key_info(
    profile_id: &str,
    info: Option<crate::ssh::managed_key::ManagedKeyInfo>,
) -> Result<Vec<SessionProfile>, String> {
    let _write_guard = lock_profiles()?;
    let mut profiles = read_profiles()?;
    let profile = profiles
        .iter_mut()
        .find(|profile| profile.id == profile_id)
        .ok_or_else(|| "That session profile no longer exists".to_string())?;
    profile.managed_key = info;
    write_profiles(&profiles)?;
    Ok(profiles)
}

/// Fails if following jump hosts from `id` comes back to `id`.
fn ensure_no_jump_loop(id: &str, profiles: &[SessionProfile]) -> Result<(), String> {
    let mut current = profiles
        .iter()
        .find(|profile| profile.id == id)
        .and_then(|profile| profile.proxy_jump_id.clone());
    let mut steps = 0;
    while let Some(next) = current {
        if next == id || steps > profiles.len() {
            return Err(
                "That jump host leads back to this profile, which would make a loop.".to_string(),
            );
        }
        steps += 1;
        current = profiles
            .iter()
            .find(|profile| profile.id == next)
            .and_then(|profile| profile.proxy_jump_id.clone());
    }
    Ok(())
}

/// Saves several profiles in one locked write (used by the ssh_config import).
pub(crate) fn upsert_profiles(
    incoming: Vec<SessionProfile>,
) -> Result<Vec<SessionProfile>, String> {
    let _write_guard = lock_profiles()?;
    let mut profiles = read_profiles()?;
    for profile in incoming.into_iter().map(normalize_profile) {
        merge_profile(&mut profiles, profile);
    }
    profiles.sort_by_key(|profile| profile.name.to_lowercase());
    write_profiles(&profiles)?;
    Ok(profiles)
}

/// Opens and authenticates a connection nobody is watching; a server that
/// needs an answer only the user can give fails with the
/// `INTERACTIVE_AUTH_PREFIX` sentinel.
pub fn open_ssh_session(target: &SshTarget) -> Result<SshConnection, String> {
    open_ssh_session_with(target, &mut NonInteractive::default())
}

/// `open_ssh_session`, with `auth` answering keyboard-interactive questions
/// for the target and every jump host before it.
pub fn open_ssh_session_with(
    target: &SshTarget,
    auth: &mut dyn AuthResponder,
) -> Result<SshConnection, String> {
    open_ssh_session_with_policy(target, auth, false)
}

/// Proves that this target accepts its managed key. Jump hosts use their
/// usual credentials, but the final host cannot fall back to an agent,
/// password, keyboard-interactive, or a successful "none" sign-in.
pub(crate) fn verify_managed_key_session(target: &SshTarget) -> Result<SshConnection, String> {
    open_ssh_session_with_policy(target, &mut NonInteractive::default(), true)
}

fn open_ssh_session_with_policy(
    target: &SshTarget,
    auth: &mut dyn AuthResponder,
    managed_key_only: bool,
) -> Result<SshConnection, String> {
    // Direct connection: TCP straight to the target.
    // Proxied connection: connect the bastion first, open a direct-tcpip
    // channel to the target through it, and expose it as a local socket the
    // normal handshake path connects to.
    let (tcp, tunnel) = match &target.proxy {
        None => {
            let address = format_host_port(&target.host, target.port);
            let socket_addrs = (host_without_brackets(&target.host), target.port)
                .to_socket_addrs()
                .map_err(|error| format!("Network resolution failed for {}: {}", address, error))?
                .collect::<Vec<_>>();
            if socket_addrs.is_empty() {
                return Err(format!("No network address found for {}", address));
            }

            // Try every resolved address (e.g. an AAAA record that is not
            // routable here, then the A record) before giving up.
            let mut connected = None;
            let mut last_error = None;
            for socket_addr in socket_addrs {
                match TcpStream::connect_timeout(&socket_addr, Duration::from_secs(10)) {
                    Ok(tcp) => {
                        connected = Some(tcp);
                        break;
                    }
                    Err(error) => last_error = Some(error),
                }
            }
            let tcp = connected.ok_or_else(|| {
                format!(
                    "Network timeout or connection failure for {}: {}",
                    address,
                    last_error
                        .map(|error| error.to_string())
                        .unwrap_or_else(|| "all resolved addresses failed".to_string())
                )
            })?;
            enable_tcp_keepalive(&tcp);
            (tcp, None)
        }
        Some(proxy) => open_tunneled_stream(target, proxy, auth)?,
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
            "SSH handshake failed for {}: {}",
            format_host_port(&target.host, target.port),
            error
        )
    })?;

    verify_known_host(&session, target)?;
    authenticate(&session, target, auth, managed_key_only)?;

    if !session.authenticated() {
        return Err(
            "SSH authentication failed. Check username, password, private key, or SSH agent."
                .to_string(),
        );
    }

    Ok(SshConnection::new(session, transport, tunnel))
}

pub(crate) fn format_host_port(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{}]:{}", host, port)
    } else {
        format!("{}:{}", host, port)
    }
}

pub(crate) fn host_without_brackets(host: &str) -> &str {
    host.strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host)
}

/// The saved profiles, for callers outside this module that need names or ids.
pub fn list_profiles() -> Result<Vec<SessionProfile>, String> {
    read_profiles()
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
    let mut target = SshTarget {
        session_id: profile.id.clone(),
        host: profile.host,
        port: profile.port,
        username: profile.username,
        password: credentials.get_password(profile_id).ok().flatten(),
        private_key_path: profile.private_key_path,
        managed_key: None,
        assigned_key: None,
        proxy,
    };
    crate::ssh::managed_key::attach_managed_keys(&mut target, credentials);
    Ok(target)
}

/// A stable description of everything that affects where a connection lands.
///
/// The password is deliberately excluded: unlocking the vault must not tear down
/// a connection that is already working.
pub fn target_signature(target: &SshTarget) -> String {
    let mut signature = format!(
        "{}@{}:{}|{}|{}",
        target.username,
        target.host.to_lowercase(),
        target.port,
        target.private_key_path.as_deref().unwrap_or("-"),
        target
            .managed_key
            .as_ref()
            .map_or("-", |key| key.fingerprint.as_str())
    );
    if let Some(key) = target.assigned_key.as_ref() {
        let _ = write!(signature, "+{}", key.fingerprint);
    }
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

/// The next sign-in method to try, given what the server currently offers
/// (`offered`, comma separated) and what was already tried.
///
/// The configured credential decides the first method, as it always has: a
/// key file, else a password, else the SSH agent. A password goes through
/// `password` when the server offers it, and through keyboard-interactive
/// otherwise (a stock Ubuntu sshd offers only the latter). Keyboard-
/// interactive is also the follow-up when the server wants a second factor
/// after a key or password succeeded partially.
pub(crate) fn next_auth_method(
    offered: &str,
    has_managed_key: bool,
    has_key: bool,
    has_password: bool,
    tried: &[AuthMethod],
    managed_key_only: bool,
) -> Option<AuthMethod> {
    if managed_key_only {
        return (has_managed_key && !tried.contains(&AuthMethod::ManagedKey))
            .then_some(AuthMethod::ManagedKey);
    }
    let offers = |name: &str| offered.split(',').any(|method| method.trim() == name);
    let first = if has_key {
        AuthMethod::PublicKey
    } else if has_password {
        // An unknown list (empty) keeps the old behaviour: try the password.
        if offers("password") || !offers("keyboard-interactive") {
            AuthMethod::Password
        } else {
            AuthMethod::KeyboardInteractive
        }
    } else {
        AuthMethod::Agent
    };
    // The managed key goes first: when the server has dropped it, the
    // profile's other credentials still sign in.
    let managed = has_managed_key.then_some(AuthMethod::ManagedKey);
    managed
        .into_iter()
        .chain([first, AuthMethod::KeyboardInteractive])
        .filter(|method| !tried.contains(method))
        .find(|method| {
            *method == first || Some(*method) == managed || offers("keyboard-interactive")
        })
}

fn interactive_auth_sentinel(hop: &HopInfo) -> String {
    format!("{}{}", INTERACTIVE_AUTH_PREFIX, hop.key())
}

fn authenticate(
    session: &Session,
    target: &SshTarget,
    auth: &mut dyn AuthResponder,
    managed_key_only: bool,
) -> Result<(), String> {
    let private_key_path = normalize_optional_string(target.private_key_path.clone());
    if let Some(path) = &private_key_path {
        if !Path::new(path).exists() {
            return Err(format!("Private key file was not found: {}", path));
        }
    }
    let password = normalize_optional_string(target.password.clone());
    let mut hop = HopInfo {
        session_id: target.session_id.clone(),
        host: target.host.clone(),
        port: target.port,
        username: target.username.clone(),
        // With a key file the stored secret is its passphrase, not the
        // account password, so it must not answer a password question.
        saved_password: if private_key_path.is_some() {
            None
        } else {
            password.clone()
        },
    };

    // Asking for the list is itself a "none" sign-in, which a server may
    // simply accept.
    let mut offered = if managed_key_only {
        // Do not send "none": even a server accepting unauthenticated
        // connections must prove that it accepts this particular key.
        String::new()
    } else {
        match session.auth_methods(&target.username) {
            Ok(list) => list.to_string(),
            Err(_) if session.authenticated() => return Ok(()),
            Err(_) => String::new(),
        }
    };
    let mut tried = Vec::new();
    let mut last_error = None;
    while let Some(method) = next_auth_method(
        &offered,
        target.managed_key.is_some() || target.assigned_key.is_some(),
        private_key_path.is_some(),
        password.is_some(),
        &tried,
        managed_key_only,
    ) {
        tried.push(method);
        match method {
            AuthMethod::ManagedKey => {
                // The profile's own managed key, then the key manager key
                // assigned to it.
                for key in target.managed_key.iter().chain(target.assigned_key.iter()) {
                    if session
                        .userauth_pubkey_memory(&target.username, None, &key.private_openssh, None)
                        .is_ok()
                        || session.authenticated()
                    {
                        break;
                    }
                    last_error = Some(
                        "The server no longer accepts this profile's GpuTerm key. Set the key up again.",
                    );
                }
            }
            AuthMethod::PublicKey => {
                let key = Path::new(private_key_path.as_deref().unwrap_or_default());
                if session
                    .userauth_pubkey_file(&target.username, None, key, password.as_deref())
                    .is_err()
                {
                    last_error = Some(
                        "SSH key authentication failed. Check username, private key, and passphrase.",
                    );
                }
            }
            AuthMethod::Password => {
                let secret = password.as_deref().unwrap_or_default();
                if session.userauth_password(&target.username, secret).is_err() {
                    last_error =
                        Some("SSH password authentication failed. Check username and password.");
                    // Already rejected: offering it again to a password
                    // question would only spend another failed sign-in.
                    hop.saved_password = None;
                }
            }
            AuthMethod::Agent => {
                if session.userauth_agent(&target.username).is_err() {
                    last_error = Some(
                        "SSH agent authentication failed. Enter a password or private key path.",
                    );
                }
            }
            AuthMethod::KeyboardInteractive => {
                if !auth.interactive() && asks_for_codes(&hop.key()) {
                    // Known to want a code: do not spend a failed sign-in.
                    return Err(interactive_auth_sentinel(&hop));
                }
                let mut adapter = PromptAdapter {
                    responder: &mut *auth,
                    hop: &hop,
                    aborted: None,
                };
                let result = session.userauth_keyboard_interactive(&target.username, &mut adapter);
                match adapter.aborted {
                    Some(AuthAbort::NeedsUser) => return Err(interactive_auth_sentinel(&hop)),
                    Some(AuthAbort::Canceled) => {
                        return Err("Sign-in was canceled.".to_string());
                    }
                    None => {}
                }
                if result.is_err() && !session.authenticated() {
                    last_error = Some(
                        "SSH sign-in failed: the server did not accept the answers. Check the password or code.",
                    );
                }
            }
        }
        if session.authenticated() {
            return Ok(());
        }
        // A partial success (say, a key accepted but a code still needed)
        // changes what the server offers next.
        if !managed_key_only {
            if let Ok(list) = session.auth_methods(&target.username) {
                offered = list.to_string();
            }
        }
    }
    Err(last_error
        .unwrap_or("SSH authentication failed. Check username, password, private key, or SSH agent.")
        .to_string())
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
        profile.managed_key = None;
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

fn sessions_path() -> PathBuf {
    config_dir().join("sessions.json")
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
