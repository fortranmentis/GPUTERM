use crate::llm::instance::LlmInstance;
use crate::llm::monitor::MonitorHandle;
use crate::ssh::agent_monitor::AgentQuotaHistories;
use crate::ssh::credentials::SecureCredentialStore;
use crate::ssh::kbdint::PendingAuthPrompts;
use crate::ssh::system_monitor::{RemoteOs, SystemMonitorSettings};
use crate::ssh::terminal::TerminalHandle;
use serde::{Deserialize, Serialize};
use ssh2::Session;
use std::collections::{HashMap, HashSet};
use std::net::TcpStream;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

/// Shared per-session SSH connections reused for short operations
/// (directory listings, stat calls, resource details). `ssh2::Session` is
/// internally synchronized but operations on one session must be serialized,
/// hence the per-entry mutex. Holding `Option<SshConnection>` inside the slot
/// mutex also serializes cold-cache connection creation per session.
pub type OpsSessions = Arc<Mutex<HashMap<String, Arc<Mutex<Option<SshConnection>>>>>>;
pub type AgentQuotaRefreshes = Arc<Mutex<HashMap<String, HashSet<String>>>>;

fn is_false(value: &bool) -> bool {
    !*value
}

fn default_true() -> bool {
    true
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
    /// GPU tools per session for the detail path, so a refresh does not
    /// re-probe (and re-run the slow `amd-smi static`); cleared with
    /// `remote_os_cache`.
    pub gpu_probe_cache: Arc<Mutex<HashMap<String, crate::ssh::gpu_monitor::GpuProbe>>>,
    /// The last hour of usage per session, for the history charts.
    pub telemetry_history: Arc<crate::ssh::telemetry_history::TelemetryHistory>,
    /// The most recent telemetry reading per session, as sent to the UI; the
    /// MCP server reads it, since the backend otherwise keeps only history.
    pub latest_telemetry: Arc<Mutex<HashMap<String, serde_json::Value>>>,
    /// Threshold alerts and their per-rule state.
    pub alerts: Arc<crate::alerts::AlertEngine>,
    /// Sign-in questions waiting for the user's answers.
    pub auth_prompts: PendingAuthPrompts,
    /// Provider names queued by the UI for an immediate account-quota refresh.
    pub agent_quota_refreshes: AgentQuotaRefreshes,
    /// In-memory AGY quota history keyed by monitored host/account identity.
    pub agent_quota_histories: AgentQuotaHistories,
    /// Registered Ollama/vLLM instances. Unlike everything above this is not
    /// keyed by SSH session: an LLM runtime is a standalone HTTP endpoint.
    pub llm_instances: Arc<Mutex<Vec<LlmInstance>>>,
    /// Latest LLM runtime telemetry plus the poller's refresh trigger.
    pub llm_monitor: Arc<MonitorHandle>,
    pub port_forwards: Arc<crate::ssh::forwarding::ForwardManager>,
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
    /// Port forwards started when the profile connects. Only
    /// `save_profile_forwards` changes them; other saves keep what is stored.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forwards: Vec<crate::ssh::forwarding::ForwardSpec>,
    /// The key GpuTerm generated and installed for this profile. Its private
    /// half lives only in the vault. Like `forwards`, only its own commands
    /// change it; other saves keep what is stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_key: Option<crate::ssh::managed_key::ManagedKeyInfo>,
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
    /// Whether sign-in questions (a one-time code) may be shown to the user.
    /// Automatic reconnects send false, so they stop instead of prompting.
    #[serde(default = "default_true")]
    pub allow_interactive_auth: bool,
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
    /// The profile's GpuTerm-managed key, loaded from the vault; tried first.
    pub managed_key: Option<crate::ssh::managed_key::SecretKey>,
    /// The key manager key assigned to the profile, loaded from the vault;
    /// tried right after `managed_key`.
    pub assigned_key: Option<crate::ssh::managed_key::SecretKey>,
    /// Jump host to tunnel through, resolved from the saved profile chain.
    pub proxy: Option<Box<SshTarget>>,
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
    /// Held while a channel is being opened. libssh2 keeps one in-progress
    /// channel-open per session and resumes it on the next call whatever that
    /// call asked for, so two threads opening channels at once on a
    /// nonblocking session could each get the other's destination.
    pub(crate) open_lock: Mutex<()>,
}

impl SshConnection {
    pub(super) fn new(
        session: Session,
        transport: TcpStream,
        tunnel: Option<SshTunnel>,
    ) -> Self {
        Self {
            session,
            _transport: transport,
            _tunnel: tunnel,
            open_lock: Mutex::new(()),
        }
    }

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

pub(super) struct SshTunnel {
    pub(super) stop: Arc<AtomicBool>,
    pub(super) _bastion: Box<SshConnection>,
}

impl Drop for SshTunnel {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthMethod {
    /// The GpuTerm-managed key, from the vault.
    ManagedKey,
    PublicKey,
    Password,
    KeyboardInteractive,
    Agent,
}
