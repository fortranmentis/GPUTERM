//! User-defined port forwards: local (`ssh -L`), remote (`ssh -R`) and dynamic
//! SOCKS5 (`ssh -D`).
//!
//! Every forward of one profile shares a single nonblocking SSH connection
//! (`open_forwarding_session`), separate from the terminal and ops sessions.
//! Rules saved on the profile start when it connects; others can be added
//! while connected and last until stopped or disconnected. The UI polls
//! `list_forwards` for status.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tauri::State;
use uuid::Uuid;

use super::session::{
    is_would_block, list_profiles, open_direct_channel, open_forwarding_session,
    connection_answers, pump_channel_socket, target_for_active_session, try_reserve_slot,
    AppState, CountGuard, IdleBackoff, SshConnection, SshTarget, HOP_PROBE_TIMEOUT,
};

/// Simultaneous connections per forward. A browser opens several per page; a
/// cap keeps one forward from exhausting sshd's `MaxSessions` for the host.
const MAX_CONNECTIONS_PER_FORWARD: usize = 32;
const CHANNEL_OPEN_TIMEOUT: Duration = Duration::from_secs(10);
const SOCKS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const LOCAL_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ForwardKind {
    /// A port on this computer reaches `dest` from the server (`-L`).
    Local,
    /// A port on the server reaches `dest` from this computer (`-R`).
    Remote,
    /// A SOCKS5 proxy on this computer that connects from the server (`-D`).
    Dynamic,
}

fn loopback() -> String {
    "127.0.0.1".to_string()
}

fn enabled_by_default() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardSpec {
    #[serde(default)]
    pub id: String,
    pub kind: ForwardKind,
    /// Where the listener binds: on this computer for local and dynamic
    /// forwards, on the server for remote ones.
    #[serde(default = "loopback")]
    pub bind_host: String,
    pub bind_port: u16,
    /// The far end; unused by dynamic forwards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dest_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dest_port: Option<u16>,
    /// Saved rules only: start it when the profile connects.
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
}

/// Checks a rule and fills in defaults. A saved profile only ever holds rules
/// that pass this.
pub fn validate_spec(mut spec: ForwardSpec) -> Result<ForwardSpec, String> {
    if spec.id.trim().is_empty() {
        spec.id = Uuid::new_v4().to_string();
    }
    spec.bind_host = spec.bind_host.trim().to_string();
    if spec.bind_host.is_empty() {
        spec.bind_host = loopback();
    }
    // Ports below 1024 may need privileges; binding reports that clearly.
    if spec.bind_port == 0 {
        return Err("The listening port must be between 1 and 65535".to_string());
    }
    match spec.kind {
        ForwardKind::Local | ForwardKind::Remote => {
            let host = spec
                .dest_host
                .as_deref()
                .map(str::trim)
                .filter(|host| !host.is_empty())
                .ok_or_else(|| "A destination host is required".to_string())?
                .to_string();
            if !spec.dest_port.is_some_and(|port| port > 0) {
                return Err("A destination port between 1 and 65535 is required".to_string());
            }
            spec.dest_host = Some(host);
        }
        ForwardKind::Dynamic => {
            spec.dest_host = None;
            spec.dest_port = None;
        }
    }
    Ok(spec)
}

/// Whether a bind address is reachable only from this machine. Anything else
/// exposes the forward to the network it is on.
pub fn is_loopback_bind(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ForwardStatus {
    pub profile_id: String,
    pub spec: ForwardSpec,
    /// `starting` | `listening` | `error` | `stopped`
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// The port actually listening; the server may report a different one
    /// for a remote forward.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bound_port: Option<u16>,
    pub active_connections: usize,
    pub bytes_transferred: u64,
    /// Not saved on the profile; gone after disconnect.
    pub temporary: bool,
    /// The listener accepts connections from other machines, not just this
    /// one (or, for a remote forward, the server itself).
    pub exposed: bool,
}

#[derive(Debug, Default)]
struct RuleState {
    state: String,
    message: Option<String>,
    bound_port: Option<u16>,
}

struct RunningRule {
    spec: ForwardSpec,
    temporary: bool,
    stop: Arc<AtomicBool>,
    state: Arc<Mutex<RuleState>>,
    active: Arc<AtomicUsize>,
    bytes: Arc<AtomicU64>,
}

impl RunningRule {
    fn status(&self, profile_id: &str) -> ForwardStatus {
        let state = self
            .state
            .lock()
            .map(|state| (state.state.clone(), state.message.clone(), state.bound_port));
        let (state, message, bound_port) =
            state.unwrap_or_else(|_| ("error".to_string(), Some("State unavailable".into()), None));
        ForwardStatus {
            profile_id: profile_id.to_string(),
            spec: self.spec.clone(),
            state,
            message,
            bound_port,
            active_connections: self.active.load(Ordering::SeqCst),
            bytes_transferred: self.bytes.load(Ordering::SeqCst),
            temporary: self.temporary,
            exposed: !is_loopback_bind(&self.spec.bind_host),
        }
    }

    fn is_running(&self) -> bool {
        self.state
            .lock()
            .map(|state| state.state == "starting" || state.state == "listening")
            .unwrap_or(false)
    }
}

impl Drop for RunningRule {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

struct ForwardSet {
    connection: Arc<SshConnection>,
    rules: Vec<RunningRule>,
}

/// Every profile's running forwards. Lives in `AppState`.
#[derive(Default)]
pub struct ForwardManager {
    sets: Mutex<HashMap<String, ForwardSet>>,
}

impl ForwardManager {
    /// Stops every forward of a profile; called when it disconnects.
    pub fn stop_profile(&self, profile_id: &str) {
        if let Ok(mut sets) = self.sets.lock() {
            sets.remove(profile_id);
        }
    }

    fn statuses(&self, profile_id: Option<&str>) -> Vec<ForwardStatus> {
        let Ok(sets) = self.sets.lock() else {
            return Vec::new();
        };
        let mut statuses: Vec<ForwardStatus> = sets
            .iter()
            .filter(|(id, _)| profile_id.is_none_or(|wanted| wanted == id.as_str()))
            .flat_map(|(id, set)| set.rules.iter().map(|rule| rule.status(id)))
            .collect();
        statuses.sort_by(|left, right| {
            (&left.profile_id, left.spec.bind_port).cmp(&(&right.profile_id, right.spec.bind_port))
        });
        statuses
    }
}

fn saved_forwards(profile_id: &str) -> Result<Vec<ForwardSpec>, String> {
    Ok(list_profiles()?
        .into_iter()
        .find(|profile| profile.id == profile_id)
        .map(|profile| profile.forwards)
        .unwrap_or_default())
}

/// The forwarding connection for a profile, reusing a live one.
fn connection_for(
    manager: &ForwardManager,
    target: &SshTarget,
) -> Result<Arc<SshConnection>, String> {
    if let Ok(sets) = manager.sets.lock() {
        if let Some(set) = sets.get(&target.session_id) {
            if connection_answers(&set.connection, HOP_PROBE_TIMEOUT) {
                return Ok(Arc::clone(&set.connection));
            }
        }
    }
    open_forwarding_session(target)
}

/// Brings the profile's forwards up: its saved, enabled rules plus any
/// temporary ones already listed. Rules still running on a live connection
/// are left alone, so calling this after every connect or foreground return
/// only restarts what actually stopped.
pub fn start_profile_forwards(
    manager: &ForwardManager,
    target: &SshTarget,
) -> Result<Vec<ForwardStatus>, String> {
    let profile_id = target.session_id.as_str();
    let saved: Vec<ForwardSpec> = saved_forwards(profile_id)?
        .into_iter()
        .filter(|spec| spec.enabled)
        .collect();
    let has_temporary = manager
        .sets
        .lock()
        .map(|sets| {
            sets.get(profile_id)
                .is_some_and(|set| !set.rules.is_empty())
        })
        .unwrap_or(false);
    if saved.is_empty() && !has_temporary {
        return Ok(manager.statuses(Some(profile_id)));
    }

    let connection = connection_for(manager, target)?;
    {
        let mut sets = manager
            .sets
            .lock()
            .map_err(|_| "Port forwards are unavailable".to_string())?;
        let set = sets
            .entry(profile_id.to_string())
            .or_insert_with(|| ForwardSet {
                connection: Arc::clone(&connection),
                rules: Vec::new(),
            });
        let moved = !Arc::ptr_eq(&set.connection, &connection);
        set.connection = Arc::clone(&connection);

        // Temporary rules that stopped (or lived on the old connection).
        let mut keep = Vec::new();
        let mut relaunch = Vec::new();
        for rule in set.rules.drain(..) {
            let saved_rule = saved.iter().any(|spec| spec.id == rule.spec.id);
            if saved_rule {
                // Saved rules are rebuilt from the profile below.
                if !moved && rule.is_running() {
                    keep.push(rule);
                }
            } else if rule.temporary {
                if !moved && rule.is_running() {
                    keep.push(rule);
                } else {
                    relaunch.push(rule.spec.clone());
                }
            }
        }
        set.rules = keep;
        for spec in saved {
            if !set.rules.iter().any(|rule| rule.spec.id == spec.id) {
                set.rules.push(launch(&connection, spec, false));
            }
        }
        for spec in relaunch {
            set.rules.push(launch(&connection, spec, true));
        }
    }
    Ok(manager.statuses(Some(profile_id)))
}

/// Starts one rule while connected. A rule with the id of a saved one
/// restarts it; any other is temporary.
fn start_one_forward(
    manager: &ForwardManager,
    target: &SshTarget,
    spec: ForwardSpec,
) -> Result<Vec<ForwardStatus>, String> {
    let profile_id = target.session_id.as_str();
    let spec = validate_spec(spec)?;
    let temporary = !saved_forwards(profile_id)?
        .iter()
        .any(|saved| saved.id == spec.id);
    let connection = connection_for(manager, target)?;
    {
        let mut sets = manager
            .sets
            .lock()
            .map_err(|_| "Port forwards are unavailable".to_string())?;
        let set = sets
            .entry(profile_id.to_string())
            .or_insert_with(|| ForwardSet {
                connection: Arc::clone(&connection),
                rules: Vec::new(),
            });
        if !Arc::ptr_eq(&set.connection, &connection) {
            // The old connection died; everything on it has to move.
            set.connection = Arc::clone(&connection);
            let specs: Vec<(ForwardSpec, bool)> = set
                .rules
                .drain(..)
                .filter(|rule| rule.spec.id != spec.id)
                .map(|rule| (rule.spec.clone(), rule.temporary))
                .collect();
            for (old, old_temporary) in specs {
                set.rules.push(launch(&connection, old, old_temporary));
            }
        }
        // Replacing a rule frees its port before the new one binds.
        set.rules.retain(|rule| rule.spec.id != spec.id);
        if let Some(clash) = set.rules.iter().find(|rule| {
            rule.is_running()
                && rule.spec.kind == spec.kind
                && rule.spec.bind_port == spec.bind_port
        }) {
            return Err(format!(
                "Port {} is already forwarded by another rule",
                clash.spec.bind_port
            ));
        }
        set.rules.push(launch(&connection, spec, temporary));
    }
    Ok(manager.statuses(Some(profile_id)))
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Starts one rule on a connected profile. A rule with the id of a saved one
/// restarts it; any other is temporary.
#[tauri::command(async)]
pub fn start_forward(
    state: State<'_, AppState>,
    profile_id: String,
    spec: ForwardSpec,
) -> Result<Vec<ForwardStatus>, String> {
    let target = target_for_active_session(&state, &profile_id)?;
    start_one_forward(&state.port_forwards, &target, spec)
}

/// (Re)starts the saved, enabled rules of a connected profile.
#[tauri::command(async)]
pub fn restart_profile_forwards(
    state: State<'_, AppState>,
    profile_id: String,
) -> Result<Vec<ForwardStatus>, String> {
    let target = target_for_active_session(&state, &profile_id)?;
    start_profile_forwards(&state.port_forwards, &target)
}

#[tauri::command]
pub fn stop_forward(
    state: State<'_, AppState>,
    profile_id: String,
    forward_id: String,
) -> Result<Vec<ForwardStatus>, String> {
    let manager = &state.port_forwards;
    if let Ok(mut sets) = manager.sets.lock() {
        if let Some(set) = sets.get_mut(&profile_id) {
            set.rules.retain(|rule| rule.spec.id != forward_id);
            if set.rules.is_empty() {
                sets.remove(&profile_id);
            }
        }
    }
    Ok(manager.statuses(Some(&profile_id)))
}

#[tauri::command]
pub fn list_forwards(state: State<'_, AppState>, profile_id: Option<String>) -> Vec<ForwardStatus> {
    state.port_forwards.statuses(profile_id.as_deref())
}

/// Checks a rule without starting it, so the editor can show the error.
#[tauri::command]
pub fn validate_forward(spec: ForwardSpec) -> Result<ForwardSpec, String> {
    validate_spec(spec)
}

// ---------------------------------------------------------------------------
// Running a rule
// ---------------------------------------------------------------------------

fn launch(connection: &Arc<SshConnection>, spec: ForwardSpec, temporary: bool) -> RunningRule {
    let rule = RunningRule {
        spec: spec.clone(),
        temporary,
        stop: Arc::new(AtomicBool::new(false)),
        state: Arc::new(Mutex::new(RuleState {
            state: "starting".to_string(),
            ..Default::default()
        })),
        active: Arc::new(AtomicUsize::new(0)),
        bytes: Arc::new(AtomicU64::new(0)),
    };
    let context = RuleContext {
        connection: Arc::clone(connection),
        spec,
        stop: Arc::clone(&rule.stop),
        state: Arc::clone(&rule.state),
        active: Arc::clone(&rule.active),
        bytes: Arc::clone(&rule.bytes),
    };
    thread::spawn(move || {
        let outcome = match context.spec.kind {
            ForwardKind::Local | ForwardKind::Dynamic => run_listener(&context),
            ForwardKind::Remote => run_remote(&context),
        };
        context.finish(outcome);
    });
    rule
}

struct RuleContext {
    connection: Arc<SshConnection>,
    spec: ForwardSpec,
    stop: Arc<AtomicBool>,
    state: Arc<Mutex<RuleState>>,
    active: Arc<AtomicUsize>,
    bytes: Arc<AtomicU64>,
}

impl RuleContext {
    fn set(&self, state: &str, message: Option<String>, bound_port: Option<u16>) {
        if let Ok(mut current) = self.state.lock() {
            current.state = state.to_string();
            current.message = message;
            if bound_port.is_some() {
                current.bound_port = bound_port;
            }
        }
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    fn finish(&self, outcome: Result<(), String>) {
        match outcome {
            Err(message) => self.set("error", Some(message), None),
            Ok(()) => self.set("stopped", None, None),
        }
    }

    /// Pumps one connection on its own thread, counting its bytes. `slot`
    /// is the connection's place under the limit, held until the pump ends.
    fn pump(&self, slot: CountGuard, channel: ssh2::Channel, socket: TcpStream) {
        let stop = Arc::clone(&self.stop);
        let bytes = Arc::clone(&self.bytes);
        thread::spawn(move || {
            let _slot = slot;
            pump_channel_socket(socket, channel, &stop, Some(&bytes));
        });
    }

    /// A place under the per-forward connection limit, or None when full.
    fn try_reserve(&self) -> Option<CountGuard> {
        try_reserve_slot(&self.active, MAX_CONNECTIONS_PER_FORWARD)
    }
}

/// How often an idle forward checks that its SSH transport is still there.
const IDLE_LIVENESS_INTERVAL: Duration = Duration::from_secs(5);

/// A local (`-L`) or dynamic (`-D`) forward: listen on this computer.
fn run_listener(context: &RuleContext) -> Result<(), String> {
    let spec = &context.spec;
    let address = (spec.bind_host.trim_matches(['[', ']']), spec.bind_port);
    let listener = TcpListener::bind(address).map_err(|error| match error.kind() {
        ErrorKind::AddrInUse => {
            format!("Port {} is already in use on this computer", spec.bind_port)
        }
        ErrorKind::PermissionDenied => format!(
            "Binding port {} needs administrator rights on this computer; use a port above 1023",
            spec.bind_port
        ),
        _ => format!(
            "Could not listen on {}:{}: {}",
            spec.bind_host, spec.bind_port, error
        ),
    })?;
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("Could not configure the listener: {}", error))?;
    context.set("listening", None, Some(spec.bind_port));

    let mut idle = IdleBackoff::for_accept_loop();
    let mut last_liveness_check = Instant::now();
    while !context.stopped() {
        let socket = match listener.accept() {
            Ok((socket, _)) => socket,
            Err(ref error) if error.kind() == ErrorKind::WouldBlock => {
                idle.sleep();
                // A dead hop would otherwise go unnoticed until a client
                // connects, leaving the rule looking "listening".
                if last_liveness_check.elapsed() >= IDLE_LIVENESS_INTERVAL {
                    last_liveness_check = Instant::now();
                    if !connection_answers(&context.connection, HOP_PROBE_TIMEOUT) {
                        return Err("The SSH connection was lost".to_string());
                    }
                }
                continue;
            }
            Err(error) => return Err(format!("The listener stopped: {}", error)),
        };
        idle.reset();
        let Some(slot) = context.try_reserve() else {
            drop(socket);
            continue;
        };
        if spec.kind == ForwardKind::Dynamic {
            // The handshake can wait on the client, so it runs off the accept loop.
            let connection = Arc::clone(&context.connection);
            let worker = RuleContext {
                connection: Arc::clone(&connection),
                spec: spec.clone(),
                stop: Arc::clone(&context.stop),
                state: Arc::clone(&context.state),
                active: Arc::clone(&context.active),
                bytes: Arc::clone(&context.bytes),
            };
            // The slot moves with the handshake, so handshakes in progress
            // count against the limit too.
            thread::spawn(move || serve_socks(&worker, slot, socket));
            continue;
        }

        let host = spec.dest_host.as_deref().unwrap_or("127.0.0.1");
        let port = spec.dest_port.unwrap_or(0);
        match open_direct_channel(
            &context.connection,
            host,
            port,
            Instant::now() + CHANNEL_OPEN_TIMEOUT,
        ) {
            Ok(channel) => context.pump(slot, channel, socket),
            Err(error) => {
                drop(socket);
                if !connection_answers(&context.connection, HOP_PROBE_TIMEOUT) {
                    return Err("The SSH connection was lost".to_string());
                }
                // Per connection: the destination refused or is down.
                context.set("listening", Some(error), None);
            }
        }
    }
    Ok(())
}

fn serve_socks(context: &RuleContext, slot: CountGuard, mut socket: TcpStream) {
    if socket.set_nonblocking(false).is_err()
        || socket
            .set_read_timeout(Some(SOCKS_HANDSHAKE_TIMEOUT))
            .is_err()
    {
        return;
    }
    let (host, port) = match socks5_handshake(&mut socket) {
        Ok(destination) => destination,
        // The reply, if any, was already sent; the client sees the refusal.
        Err(_) => return,
    };
    match open_direct_channel(
        &context.connection,
        &host,
        port,
        Instant::now() + CHANNEL_OPEN_TIMEOUT,
    ) {
        Ok(channel) => {
            if socket.write_all(&socks5_reply(SOCKS_SUCCEEDED)).is_err() {
                return;
            }
            let _ = socket.set_read_timeout(None);
            context.pump(slot, channel, socket);
        }
        Err(_) => {
            let _ = socket.write_all(&socks5_reply(SOCKS_HOST_UNREACHABLE));
        }
    }
}

/// A remote (`-R`) forward: the server listens and sends connections back.
fn run_remote(context: &RuleContext) -> Result<(), String> {
    let spec = &context.spec;
    let session = &context.connection.session;
    let deadline = Instant::now() + CHANNEL_OPEN_TIMEOUT;
    let (mut listener, bound_port) = loop {
        match session.channel_forward_listen(spec.bind_port, Some(&spec.bind_host), None) {
            Ok(result) => break result,
            Err(error) if is_would_block(&error) => {
                if Instant::now() >= deadline || context.stopped() {
                    return Err("Timed out asking the server to listen".to_string());
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => {
                return Err(format!(
                    "The server refused to listen on {}:{} ({}). Ports below 1024 need root, \
                     and other hosts need GatewayPorts in sshd_config.",
                    spec.bind_host, spec.bind_port, error
                ));
            }
        }
    };
    context.set("listening", None, Some(bound_port));

    let destination = format!(
        "{}:{}",
        spec.dest_host.as_deref().unwrap_or("127.0.0.1"),
        spec.dest_port.unwrap_or(0)
    );
    let mut idle = IdleBackoff::for_accept_loop();
    let mut last_liveness_check = Instant::now();
    while !context.stopped() {
        let channel = match listener.accept() {
            Ok(channel) => channel,
            Err(error) if is_would_block(&error) => {
                idle.sleep();
                if last_liveness_check.elapsed() >= IDLE_LIVENESS_INTERVAL {
                    last_liveness_check = Instant::now();
                    if !connection_answers(&context.connection, HOP_PROBE_TIMEOUT) {
                        return Err("The SSH connection was lost".to_string());
                    }
                }
                continue;
            }
            Err(error) => return Err(format!("The server stopped forwarding: {}", error)),
        };
        idle.reset();
        let Some(slot) = context.try_reserve() else {
            continue;
        };
        match connect_local(&destination) {
            Ok(socket) => context.pump(slot, channel, socket),
            Err(error) => context.set("listening", Some(error), None),
        }
    }
    // Dropping `listener` cancels the forward on the server.
    Ok(())
}

fn connect_local(destination: &str) -> Result<TcpStream, String> {
    let addresses: Vec<SocketAddr> = destination
        .to_socket_addrs()
        .map_err(|error| format!("Could not resolve {}: {}", destination, error))?
        .collect();
    let mut last_error = format!("Could not resolve {}", destination);
    for address in addresses {
        match TcpStream::connect_timeout(&address, LOCAL_CONNECT_TIMEOUT) {
            Ok(stream) => return Ok(stream),
            Err(error) => last_error = format!("Could not connect to {}: {}", destination, error),
        }
    }
    Err(last_error)
}

// ---------------------------------------------------------------------------
// SOCKS5 (RFC 1928): no authentication, CONNECT only.
// ---------------------------------------------------------------------------

const SOCKS_VERSION: u8 = 5;
const SOCKS_SUCCEEDED: u8 = 0;
const SOCKS_HOST_UNREACHABLE: u8 = 4;
const SOCKS_COMMAND_NOT_SUPPORTED: u8 = 7;
const SOCKS_ADDRESS_NOT_SUPPORTED: u8 = 8;

fn socks5_reply(code: u8) -> [u8; 10] {
    // The bound address is not meaningful for a forward; 0.0.0.0:0.
    [SOCKS_VERSION, code, 0, 1, 0, 0, 0, 0, 0, 0]
}

/// Reads the greeting and CONNECT request, answering everything except the
/// final success reply, which is sent once the SSH channel is open.
fn socks5_handshake<S: Read + Write>(stream: &mut S) -> Result<(String, u16), String> {
    let mut header = [0_u8; 2];
    stream
        .read_exact(&mut header)
        .map_err(|_| "Truncated SOCKS greeting".to_string())?;
    if header[0] != SOCKS_VERSION {
        return Err("Only SOCKS5 is supported".to_string());
    }
    let mut methods = vec![0_u8; header[1] as usize];
    stream
        .read_exact(&mut methods)
        .map_err(|_| "Truncated SOCKS greeting".to_string())?;
    if !methods.contains(&0) {
        let _ = stream.write_all(&[SOCKS_VERSION, 0xFF]);
        return Err("The client requires SOCKS authentication".to_string());
    }
    stream
        .write_all(&[SOCKS_VERSION, 0])
        .map_err(|_| "The SOCKS client went away".to_string())?;

    let mut request = [0_u8; 4];
    stream
        .read_exact(&mut request)
        .map_err(|_| "Truncated SOCKS request".to_string())?;
    if request[0] != SOCKS_VERSION {
        return Err("Only SOCKS5 is supported".to_string());
    }
    if request[1] != 1 {
        let _ = stream.write_all(&socks5_reply(SOCKS_COMMAND_NOT_SUPPORTED));
        return Err("Only SOCKS CONNECT is supported".to_string());
    }
    let host = match request[3] {
        1 => {
            let mut octets = [0_u8; 4];
            stream
                .read_exact(&mut octets)
                .map_err(|_| "Truncated SOCKS address".to_string())?;
            std::net::Ipv4Addr::from(octets).to_string()
        }
        3 => {
            let mut length = [0_u8; 1];
            stream
                .read_exact(&mut length)
                .map_err(|_| "Truncated SOCKS address".to_string())?;
            let mut name = vec![0_u8; length[0] as usize];
            stream
                .read_exact(&mut name)
                .map_err(|_| "Truncated SOCKS address".to_string())?;
            String::from_utf8(name).map_err(|_| "Invalid SOCKS host name".to_string())?
        }
        4 => {
            let mut octets = [0_u8; 16];
            stream
                .read_exact(&mut octets)
                .map_err(|_| "Truncated SOCKS address".to_string())?;
            std::net::Ipv6Addr::from(octets).to_string()
        }
        _ => {
            let _ = stream.write_all(&socks5_reply(SOCKS_ADDRESS_NOT_SUPPORTED));
            return Err("Unsupported SOCKS address type".to_string());
        }
    };
    let mut port = [0_u8; 2];
    stream
        .read_exact(&mut port)
        .map_err(|_| "Truncated SOCKS port".to_string())?;
    Ok((host, u16::from_be_bytes(port)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Reads from `input`, records what is written.
    struct Duplex {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
    }

    impl Duplex {
        fn new(input: &[u8]) -> Self {
            Self {
                input: Cursor::new(input.to_vec()),
                output: Vec::new(),
            }
        }
    }

    impl Read for Duplex {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.input.read(buffer)
        }
    }

    impl Write for Duplex {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.output.extend_from_slice(buffer);
            Ok(buffer.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn socks_connect_by_domain_ipv4_and_ipv6() {
        let mut domain = Duplex::new(&[
            5, 1, 0, // greeting: no auth
            5, 1, 0, 3, 11, b'e', b'x', b'a', b'm', b'p', b'l', b'e', b'.', b'o', b'r', b'g', 0x1F,
            0x90,
        ]);
        assert_eq!(
            socks5_handshake(&mut domain).unwrap(),
            ("example.org".to_string(), 8080)
        );
        assert_eq!(domain.output, vec![5, 0]);

        let mut v4 = Duplex::new(&[5, 2, 2, 0, 5, 1, 0, 1, 10, 0, 0, 7, 0, 22]);
        assert_eq!(
            socks5_handshake(&mut v4).unwrap(),
            ("10.0.0.7".to_string(), 22)
        );

        let mut v6_request = vec![5, 1, 0, 5, 1, 0, 4];
        v6_request.extend_from_slice(&std::net::Ipv6Addr::LOCALHOST.octets());
        v6_request.extend_from_slice(&[0, 80]);
        assert_eq!(
            socks5_handshake(&mut Duplex::new(&v6_request)).unwrap(),
            ("::1".to_string(), 80)
        );
    }

    #[test]
    fn socks_refuses_what_it_does_not_support() {
        let mut auth_only = Duplex::new(&[5, 1, 2]);
        assert!(socks5_handshake(&mut auth_only).is_err());
        assert_eq!(auth_only.output, vec![5, 0xFF]);

        let mut bind = Duplex::new(&[5, 1, 0, 5, 2, 0, 1, 1, 2, 3, 4, 0, 80]);
        assert!(socks5_handshake(&mut bind).is_err());
        assert_eq!(bind.output[2..4], [5, SOCKS_COMMAND_NOT_SUPPORTED]);

        let mut socks4 = Duplex::new(&[4, 1, 0, 80]);
        assert!(socks5_handshake(&mut socks4).is_err());

        let mut truncated = Duplex::new(&[5, 1, 0, 5, 1, 0, 3, 20, b'a']);
        assert!(socks5_handshake(&mut truncated).is_err());
    }

    fn spec(kind: ForwardKind, bind_port: u16) -> ForwardSpec {
        ForwardSpec {
            id: String::new(),
            kind,
            bind_host: String::new(),
            bind_port,
            dest_host: Some(" localhost ".into()),
            dest_port: Some(8888),
            enabled: true,
        }
    }

    #[test]
    fn validation_fills_defaults_and_rejects_bad_rules() {
        let local = validate_spec(spec(ForwardKind::Local, 8888)).unwrap();
        assert!(!local.id.is_empty());
        assert_eq!(local.bind_host, "127.0.0.1");
        assert_eq!(local.dest_host.as_deref(), Some("localhost"));

        // Desktops may bind privileged ports; binding reports a refusal.
        assert!(validate_spec(spec(ForwardKind::Local, 80)).is_ok());
        assert!(validate_spec(spec(ForwardKind::Local, 0)).is_err());
        assert!(validate_spec(spec(ForwardKind::Remote, 80)).is_ok());
        assert!(validate_spec(spec(ForwardKind::Remote, 0)).is_err());

        let mut missing = spec(ForwardKind::Local, 9000);
        missing.dest_port = None;
        assert!(validate_spec(missing).is_err());

        let dynamic = validate_spec(spec(ForwardKind::Dynamic, 1080)).unwrap();
        assert_eq!(dynamic.dest_host, None);
        assert_eq!(dynamic.dest_port, None);
    }

    #[test]
    fn loopback_binds_are_recognised() {
        for host in ["127.0.0.1", "localhost", "::1", "[::1]", "127.0.0.2"] {
            assert!(is_loopback_bind(host), "{host}");
        }
        for host in ["0.0.0.0", "192.168.0.5", "::"] {
            assert!(!is_loopback_bind(host), "{host}");
        }
    }

    #[test]
    fn specs_from_older_files_default_to_loopback_and_enabled() {
        let spec: ForwardSpec =
            serde_json::from_str(r#"{"kind":"dynamic","bindPort":1080}"#).unwrap();
        assert_eq!(spec.bind_host, "127.0.0.1");
        assert!(spec.enabled);
        assert_eq!(spec.id, "");
    }

    #[test]
    fn a_taken_port_is_reported_as_in_use() {
        let taken = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = taken.local_addr().unwrap().port();
        let error = TcpListener::bind(("127.0.0.1", port)).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::AddrInUse);
    }
}
