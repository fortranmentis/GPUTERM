//! SSH tunnels for instances whose runtime is only reachable on the remote
//! host's own network.
//!
//! The poller talks plain HTTP to a loopback port; an SSH `direct-tcpip` channel
//! carries it to the runtime. Nothing in `llm::http` or any adapter knows this
//! is happening.
//!
//! `TunnelManager` is owned solely by the coordinator thread, so it needs no
//! interior mutability and no lock. The one thing it must never do is block: an
//! SSH connect can take ten seconds and the coordinator ticks every 500 ms,
//! which is why connecting is handed to the backend and polled through a
//! `ConnectHandle` instead of being awaited.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use super::adapter::{RuntimeError, RuntimeErrorCode};
use super::instance::{base_url_endpoint, tunnel_base_url, LlmInstance};
use crate::ssh::credentials::SecureCredentialStore;
use crate::ssh::session::{
    host_keys_trusted, open_forwarding_session, open_persistent_forward, target_for_profile,
    target_signature, ActiveConnection, PersistentForward, SshConnection, SshTarget,
    UNKNOWN_HOST_KEY_PREFIX,
};

/// Deliberately parallel to `monitor::BACKOFF_STEPS_SECS`, but per SSH profile:
/// this only suppresses connect attempts, it never schedules a poll.
const SSH_BACKOFF_STEPS_SECS: [u64; 4] = [5, 15, 30, 60];

/// How long a connect may stay in flight before it is called a failure, so an
/// instance is never stuck reporting "not polled yet" forever.
pub const CONNECT_TIMEOUT_SECS: u64 = 20;

fn backoff_secs(failures: u32) -> u64 {
    let index = (failures.max(1) - 1) as usize;
    SSH_BACKOFF_STEPS_SECS[index.min(SSH_BACKOFF_STEPS_SECS.len() - 1)]
}

/// Turns an SSH-layer error string into a runtime error the UI can explain.
///
/// The untrusted-host sentinel is translated rather than passed through: it is
/// an internal protocol between `open_ssh_session` and the interactive prompt,
/// and a background thread has no way to show that prompt.
pub fn classify_ssh_error(profile_label: &str, error: &str) -> RuntimeError {
    if error.starts_with(UNKNOWN_HOST_KEY_PREFIX) {
        return untrusted_error(profile_label);
    }
    RuntimeError::new(
        RuntimeErrorCode::SshTunnelError,
        format!("SSH tunnel through {} failed: {}", profile_label, error),
    )
}

fn untrusted_error(profile_label: &str) -> RuntimeError {
    RuntimeError::new(
        RuntimeErrorCode::SshHostUntrusted,
        format!(
            "Cannot verify the SSH host key for {}. Open that SSH session in a terminal once, or \
             use Test connection on this instance, to review and trust its fingerprint. \
             Monitoring resumes automatically after that.",
            profile_label
        ),
    )
}

/// A connect in progress. Completed exactly once by whoever started it.
pub struct ConnectHandle<H> {
    slot: Arc<Mutex<Option<Result<H, RuntimeError>>>>,
}

impl<H> ConnectHandle<H> {
    /// A handle whose result is supplied later, by a thread the backend owns.
    pub fn deferred() -> Self {
        Self {
            slot: Arc::new(Mutex::new(None)),
        }
    }

    /// A handle that is already finished.
    ///
    /// Test-only: the real backend always spawns a thread, so nothing in
    /// production has a result to hand over synchronously.
    #[cfg(test)]
    pub fn settled(result: Result<H, RuntimeError>) -> Self {
        Self {
            slot: Arc::new(Mutex::new(Some(result))),
        }
    }

    pub fn completer(&self) -> ConnectCompleter<H> {
        ConnectCompleter {
            slot: Arc::clone(&self.slot),
        }
    }

    fn take(&self) -> Option<Result<H, RuntimeError>> {
        self.slot.lock().ok().and_then(|mut slot| slot.take())
    }
}

pub struct ConnectCompleter<H> {
    slot: Arc<Mutex<Option<Result<H, RuntimeError>>>>,
}

impl<H> ConnectCompleter<H> {
    pub fn complete(self, result: Result<H, RuntimeError>) {
        if let Ok(mut slot) = self.slot.lock() {
            *slot = Some(result);
        }
    }
}

/// Everything the manager needs from the SSH layer.
///
/// The seam exists for the same reason `HttpClient` does: every scheduling and
/// teardown decision above it is then testable without a server.
pub trait TunnelBackend {
    type Host;
    /// Dropping this must tear the forward down.
    type Forward: ForwardEndpoint;

    fn resolve(&self, profile_id: &str) -> Result<ResolvedTarget, RuntimeError>;
    /// Starts connecting. **Must not block** — the real implementation spawns a
    /// thread and completes the handle from there.
    fn begin_connect(&self, target: ResolvedTarget) -> ConnectHandle<Self::Host>;
    fn forward(
        &self,
        host: &Self::Host,
        remote_host: &str,
        remote_port: u16,
    ) -> Result<Self::Forward, RuntimeError>;
}

pub trait ForwardEndpoint {
    fn local_port(&self) -> u16;
    /// False once the forwarder has stopped, so the manager rebuilds instead of
    /// polling a port nothing is listening on.
    fn is_alive(&self) -> bool;
    /// True when it stopped because the SSH transport is gone, so the connection
    /// has to be rebuilt and not merely re-forwarded.
    fn hop_failed(&self) -> bool;
}

pub struct ResolvedTarget {
    /// Shown to the user in place of raw host details.
    pub label: String,
    /// Changes only when the connection would land somewhere else. Excludes the
    /// password, so unlocking the vault does not churn a healthy connection.
    pub signature: String,
    pub trusted: bool,
    pub target: SshTarget,
}

/// What the coordinator should do with an instance this tick.
#[derive(Debug, PartialEq)]
pub enum Endpoint {
    /// Poll this base URL now.
    Ready(String),
    /// The SSH hop is still being established off-thread. Skip this tick
    /// **without** recording a failure — nothing has gone wrong yet.
    Pending,
    Failed(RuntimeError),
}

struct HostEntry<H> {
    host: H,
    signature: String,
}

struct PendingConnect<H> {
    started_at: u64,
    signature: String,
    handle: ConnectHandle<H>,
}

pub struct TunnelManager<B: TunnelBackend> {
    backend: B,
    /// One SSH connection per profile, shared by every instance on that host.
    hosts: HashMap<String, HostEntry<B::Host>>,
    /// One forwarded port per instance — sharing a port between two instances
    /// would make them share the HTTP client's connection-pool key.
    forwards: HashMap<String, B::Forward>,
    /// Which profile each instance's forward rides on. Tracked here rather than
    /// re-derived, because `retain` runs after an instance may already be gone.
    owners: HashMap<String, String>,
    pending: HashMap<String, PendingConnect<B::Host>>,
    failures: HashMap<String, u32>,
    retry_at: HashMap<String, u64>,
    last_error: HashMap<String, RuntimeError>,
}

impl<B: TunnelBackend> TunnelManager<B> {
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            hosts: HashMap::new(),
            forwards: HashMap::new(),
            owners: HashMap::new(),
            pending: HashMap::new(),
            failures: HashMap::new(),
            retry_at: HashMap::new(),
            last_error: HashMap::new(),
        }
    }

    /// Resolves where this instance should be polled. Never blocks.
    pub fn endpoint_for(&mut self, instance: &LlmInstance, now: u64) -> Endpoint {
        let Some(profile_id) = instance.ssh_profile_id.clone() else {
            return Endpoint::Ready(instance.base_url.clone());
        };
        let (remote_host, remote_port) = match base_url_endpoint(&instance.base_url) {
            Ok(endpoint) => endpoint,
            Err(error) => {
                return Endpoint::Failed(RuntimeError::new(RuntimeErrorCode::SshTunnelError, error))
            }
        };

        let resolved = match self.backend.resolve(&profile_id) {
            Ok(resolved) => resolved,
            Err(error) => return Endpoint::Failed(error),
        };

        if !resolved.trusted {
            // Decided before any socket is opened, so the interactive host-key
            // prompt can never be reached from this thread.
            return Endpoint::Failed(untrusted_error(&resolved.label));
        }

        // A profile edited to point somewhere else invalidates the connection,
        // every forward riding on it, and any connect still in flight.
        if self.signature_changed(&profile_id, &resolved.signature) {
            self.drop_profile(&profile_id);
        }

        // A forwarder that stopped because its transport died condemns the whole
        // connection. Rebuilding only the forward would bind a fresh port on a
        // dead session and then fail every request forever.
        if self
            .forwards
            .get(&instance.id)
            .is_some_and(|forward| !forward.is_alive() && forward.hop_failed())
        {
            self.drop_profile(&profile_id);
        }

        self.drain_pending(&profile_id, &resolved.signature, now);

        if self.hosts.contains_key(&profile_id) {
            return self.forward_for(instance, &profile_id, &remote_host, remote_port);
        }

        if self.pending.contains_key(&profile_id) {
            return Endpoint::Pending;
        }
        if now < self.retry_at.get(&profile_id).copied().unwrap_or(0) {
            return Endpoint::Failed(
                self.last_error
                    .get(&profile_id)
                    .cloned()
                    .unwrap_or_else(|| classify_ssh_error(&resolved.label, "not connected")),
            );
        }

        let signature = resolved.signature.clone();
        let handle = self.backend.begin_connect(resolved);
        self.pending.insert(
            profile_id,
            PendingConnect {
                started_at: now,
                signature,
                handle,
            },
        );
        Endpoint::Pending
    }

    fn signature_changed(&self, profile_id: &str, signature: &str) -> bool {
        self.hosts
            .get(profile_id)
            .is_some_and(|entry| entry.signature != signature)
            || self
                .pending
                .get(profile_id)
                .is_some_and(|entry| entry.signature != signature)
    }

    /// Reuses this instance's forward, or builds one on the existing connection.
    fn forward_for(
        &mut self,
        instance: &LlmInstance,
        profile_id: &str,
        remote_host: &str,
        remote_port: u16,
    ) -> Endpoint {
        if let Some(forward) = self.forwards.get(&instance.id) {
            if forward.is_alive() {
                return Endpoint::Ready(tunnel_base_url(forward.local_port()));
            }
            self.forwards.remove(&instance.id);
            self.owners.remove(&instance.id);
        }

        let host = &self.hosts[profile_id].host;
        match self.backend.forward(host, remote_host, remote_port) {
            Ok(forward) => {
                let url = tunnel_base_url(forward.local_port());
                self.forwards.insert(instance.id.clone(), forward);
                self.owners
                    .insert(instance.id.clone(), profile_id.to_string());
                Endpoint::Ready(url)
            }
            // Binding a local port failed, which says nothing about the hop, so
            // the connection is kept and this is retried next tick.
            Err(error) => Endpoint::Failed(error),
        }
    }

    /// Folds in a finished connect, and gives up on one that never reported.
    fn drain_pending(&mut self, profile_id: &str, signature: &str, now: u64) {
        let Some(pending) = self.pending.get(profile_id) else {
            return;
        };

        match pending.handle.take() {
            Some(Ok(host)) => {
                let signature = pending.signature.clone();
                self.pending.remove(profile_id);
                self.hosts
                    .insert(profile_id.to_string(), HostEntry { host, signature });
                self.failures.remove(profile_id);
                self.retry_at.remove(profile_id);
                self.last_error.remove(profile_id);
            }
            Some(Err(error)) => {
                self.pending.remove(profile_id);
                self.record_failure(profile_id, error, now);
            }
            None => {
                if now.saturating_sub(pending.started_at) > CONNECT_TIMEOUT_SECS {
                    self.pending.remove(profile_id);
                    self.record_failure(
                        profile_id,
                        RuntimeError::new(
                            RuntimeErrorCode::SshTunnelError,
                            "The SSH connection did not complete in time.",
                        ),
                        now,
                    );
                }
            }
        }
        let _ = signature;
    }

    fn record_failure(&mut self, profile_id: &str, error: RuntimeError, now: u64) {
        let failures = self.failures.entry(profile_id.to_string()).or_insert(0);
        *failures += 1;
        self.retry_at
            .insert(profile_id.to_string(), now + backoff_secs(*failures));
        self.last_error.insert(profile_id.to_string(), error);
    }

    /// Drops a profile's forwards first, then its connection.
    ///
    /// Order matters: a forward holds the connection alive, so releasing them in
    /// the other order would leave the SSH session up until the last pump exits.
    fn drop_profile(&mut self, profile_id: &str) {
        let owned: Vec<String> = self
            .owners
            .iter()
            .filter(|(_, owner)| owner.as_str() == profile_id)
            .map(|(instance_id, _)| instance_id.clone())
            .collect();
        for instance_id in owned {
            self.forwards.remove(&instance_id);
            self.owners.remove(&instance_id);
        }
        self.hosts.remove(profile_id);
        self.pending.remove(profile_id);
    }

    /// Releases forwards for instances that are no longer live, then any
    /// connection left with nothing to carry.
    pub fn retain(&mut self, live: &HashSet<String>) {
        let stale: Vec<String> = self
            .forwards
            .keys()
            .filter(|id| !live.contains(id.as_str()))
            .cloned()
            .collect();
        for instance_id in stale {
            self.forwards.remove(&instance_id);
            self.owners.remove(&instance_id);
        }

        let still_used: HashSet<&str> = self.owners.values().map(String::as_str).collect();
        let idle: Vec<String> = self
            .hosts
            .keys()
            .filter(|profile_id| !still_used.contains(profile_id.as_str()))
            .cloned()
            .collect();
        for profile_id in idle {
            self.hosts.remove(&profile_id);
            self.pending.remove(&profile_id);
        }
    }

    /// Tears everything down. Called explicitly at shutdown rather than relying
    /// on drop, so the stop flags are set before the coordinator returns.
    pub fn shutdown(&mut self) {
        self.forwards.clear();
        self.owners.clear();
        self.hosts.clear();
        self.pending.clear();
    }
}

// ---------------------------------------------------------------------------
// The real backend
// ---------------------------------------------------------------------------

impl ForwardEndpoint for PersistentForward {
    fn local_port(&self) -> u16 {
        self.local_port
    }

    fn is_alive(&self) -> bool {
        PersistentForward::is_alive(self)
    }

    fn hop_failed(&self) -> bool {
        PersistentForward::hop_dead(self)
    }
}

pub struct SshBackend {
    active: Arc<Mutex<HashMap<String, ActiveConnection>>>,
    credentials: SecureCredentialStore,
}

impl SshBackend {
    pub fn new(
        active: Arc<Mutex<HashMap<String, ActiveConnection>>>,
        credentials: SecureCredentialStore,
    ) -> Self {
        Self {
            active,
            credentials,
        }
    }
}

impl TunnelBackend for SshBackend {
    type Host = Arc<SshConnection>;
    type Forward = PersistentForward;

    fn resolve(&self, profile_id: &str) -> Result<ResolvedTarget, RuntimeError> {
        let target = target_for_profile(&self.active, &self.credentials, profile_id).map_err(
            |error| RuntimeError::new(RuntimeErrorCode::SshTunnelError, error),
        )?;
        Ok(ResolvedTarget {
            label: format!("{}:{}", target.host, target.port),
            signature: target_signature(&target),
            trusted: host_keys_trusted(&target),
            target,
        })
    }

    fn begin_connect(&self, resolved: ResolvedTarget) -> ConnectHandle<Self::Host> {
        let handle = ConnectHandle::deferred();
        let completer = handle.completer();
        thread::spawn(move || {
            let result = open_forwarding_session(&resolved.target)
                .map_err(|error| classify_ssh_error(&resolved.label, &error));
            completer.complete(result);
        });
        handle
    }

    fn forward(
        &self,
        host: &Self::Host,
        remote_host: &str,
        remote_port: u16,
    ) -> Result<Self::Forward, RuntimeError> {
        open_persistent_forward(host, remote_host, remote_port)
            .map_err(|error| RuntimeError::new(RuntimeErrorCode::SshTunnelError, error))
    }
}

/// A throwaway connection and forward for one interactive connection test.
///
/// Deliberately separate from the poller's pool: the coordinator's map lives
/// inside its own thread, and a test must not disturb a healthy connection.
/// The untrusted-host sentinel is propagated verbatim so the form can show the
/// fingerprint prompt — the one place in this feature where that is possible.
pub struct OneShotTunnel {
    _forward: PersistentForward,
    _connection: Arc<SshConnection>,
}

pub fn open_one_shot(
    active: &Arc<Mutex<HashMap<String, ActiveConnection>>>,
    credentials: &SecureCredentialStore,
    profile_id: &str,
    base_url: &str,
) -> Result<(String, OneShotTunnel), String> {
    let (remote_host, remote_port) = base_url_endpoint(base_url)?;
    let target = target_for_profile(active, credentials, profile_id)?;
    let connection = open_forwarding_session(&target)?;
    let forward = open_persistent_forward(&connection, &remote_host, remote_port)?;
    let url = tunnel_base_url(forward.local_port);

    // Give the forwarder a moment to reach its accept loop. Without this the
    // very first connect can race the thread's first poll and be refused.
    thread::sleep(Duration::from_millis(20));

    Ok((
        url,
        OneShotTunnel {
            _forward: forward,
            _connection: connection,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::adapter::RuntimeType;
    use crate::llm::instance::{DEFAULT_POLL_INTERVAL_SECS, DEFAULT_REQUEST_TIMEOUT_MS};
    use std::cell::RefCell;
    use std::rc::Rc;

    const TEST_NOW: u64 = 1_799_000_000;

    fn instance(id: &str, profile: Option<&str>, base_url: &str) -> LlmInstance {
        LlmInstance {
            id: id.to_string(),
            name: id.to_string(),
            runtime_type: RuntimeType::Ollama,
            base_url: base_url.to_string(),
            enabled: true,
            request_timeout_ms: DEFAULT_REQUEST_TIMEOUT_MS,
            poll_interval_secs: DEFAULT_POLL_INTERVAL_SECS,
            created_at: TEST_NOW,
            updated_at: TEST_NOW,
            ssh_profile_id: profile.map(str::to_string),
        }
    }

    /// Counts live forwards so teardown can be asserted, since dropping a real
    /// `PersistentForward` is the only signal a thread stopped.
    #[derive(Clone)]
    struct FakeForward {
        port: u16,
        alive: Rc<RefCell<bool>>,
        hop_failed: Rc<RefCell<bool>>,
        live_count: Rc<RefCell<usize>>,
    }

    impl ForwardEndpoint for FakeForward {
        fn local_port(&self) -> u16 {
            self.port
        }
        fn is_alive(&self) -> bool {
            *self.alive.borrow()
        }
        fn hop_failed(&self) -> bool {
            *self.hop_failed.borrow()
        }
    }

    impl Drop for FakeForward {
        fn drop(&mut self) {
            *self.live_count.borrow_mut() -= 1;
        }
    }

    #[derive(Default)]
    struct FakeState {
        resolve_calls: usize,
        connect_calls: usize,
        forward_calls: usize,
        live_forwards: usize,
        next_port: u16,
        /// Set to fail the next connect.
        connect_error: Option<String>,
        trusted: bool,
        signature: String,
        /// When false, `begin_connect` returns a handle nobody completes.
        settle_immediately: bool,
    }

    struct FakeBackend {
        state: Rc<RefCell<FakeState>>,
        live: Rc<RefCell<usize>>,
    }

    impl FakeBackend {
        fn new() -> Self {
            Self {
                state: Rc::new(RefCell::new(FakeState {
                    next_port: 40000,
                    trusted: true,
                    signature: "user@host:22|-".to_string(),
                    settle_immediately: true,
                    ..Default::default()
                })),
                live: Rc::new(RefCell::new(0)),
            }
        }
    }

    impl TunnelBackend for FakeBackend {
        type Host = String;
        type Forward = FakeForward;

        fn resolve(&self, profile_id: &str) -> Result<ResolvedTarget, RuntimeError> {
            let mut state = self.state.borrow_mut();
            state.resolve_calls += 1;
            Ok(ResolvedTarget {
                label: profile_id.to_string(),
                signature: state.signature.clone(),
                trusted: state.trusted,
                target: SshTarget {
                    session_id: profile_id.to_string(),
                    host: "host".to_string(),
                    port: 22,
                    username: "user".to_string(),
                    password: None,
                    private_key_path: None,
                    proxy: None,
                },
            })
        }

        fn begin_connect(&self, resolved: ResolvedTarget) -> ConnectHandle<Self::Host> {
            let mut state = self.state.borrow_mut();
            state.connect_calls += 1;
            if !state.settle_immediately {
                return ConnectHandle::deferred();
            }
            match state.connect_error.take() {
                Some(error) => ConnectHandle::settled(Err(classify_ssh_error(
                    &resolved.label,
                    &error,
                ))),
                None => ConnectHandle::settled(Ok(resolved.signature)),
            }
        }

        fn forward(
            &self,
            _host: &Self::Host,
            _remote_host: &str,
            _remote_port: u16,
        ) -> Result<Self::Forward, RuntimeError> {
            let mut state = self.state.borrow_mut();
            state.forward_calls += 1;
            state.next_port += 1;
            state.live_forwards += 1;
            *self.live.borrow_mut() += 1;
            Ok(FakeForward {
                port: state.next_port,
                alive: Rc::new(RefCell::new(true)),
                hop_failed: Rc::new(RefCell::new(false)),
                live_count: Rc::clone(&self.live),
            })
        }
    }

    fn manager() -> (TunnelManager<FakeBackend>, Rc<RefCell<FakeState>>, Rc<RefCell<usize>>) {
        let backend = FakeBackend::new();
        let state = Rc::clone(&backend.state);
        let live = Rc::clone(&backend.live);
        (TunnelManager::new(backend), state, live)
    }

    #[test]
    fn an_instance_without_a_profile_is_polled_directly_and_touches_no_ssh() {
        let (mut manager, state, _) = manager();
        let direct = instance("a", None, "http://192.168.0.20:11434");

        assert_eq!(
            manager.endpoint_for(&direct, TEST_NOW),
            Endpoint::Ready("http://192.168.0.20:11434".to_string())
        );
        assert_eq!(state.borrow().resolve_calls, 0);
        assert_eq!(state.borrow().connect_calls, 0);
    }

    #[test]
    fn the_first_tick_is_pending_and_the_next_one_is_ready() {
        let (mut manager, state, _) = manager();
        state.borrow_mut().settle_immediately = false;
        let tunneled = instance("a", Some("p1"), "http://127.0.0.1:11434");

        // Nothing has gone wrong yet, so this must not be a failure.
        assert_eq!(manager.endpoint_for(&tunneled, TEST_NOW), Endpoint::Pending);
        assert_eq!(state.borrow().connect_calls, 1);

        // Still connecting: no second connect is started.
        assert_eq!(
            manager.endpoint_for(&tunneled, TEST_NOW + 1),
            Endpoint::Pending
        );
        assert_eq!(state.borrow().connect_calls, 1);

        // The connect thread reports success.
        manager.pending["p1"]
            .handle
            .completer()
            .complete(Ok("host".to_string()));
        assert_eq!(
            manager.endpoint_for(&tunneled, TEST_NOW + 2),
            Endpoint::Ready("http://127.0.0.1:40001".to_string())
        );
    }

    #[test]
    fn a_second_instance_reuses_the_connection_but_gets_its_own_port() {
        let (mut manager, state, _) = manager();
        let first = instance("a", Some("p1"), "http://127.0.0.1:11434");
        let second = instance("b", Some("p1"), "http://127.0.0.1:8000");

        manager.endpoint_for(&first, TEST_NOW);
        let first_url = manager.endpoint_for(&first, TEST_NOW + 1);
        let second_url = manager.endpoint_for(&second, TEST_NOW + 1);

        // One SSH login for the host, not one per instance.
        assert_eq!(state.borrow().connect_calls, 1);
        assert_eq!(state.borrow().forward_calls, 2);
        assert_ne!(first_url, second_url, "ports must not be shared");
        assert!(matches!(first_url, Endpoint::Ready(_)));
        assert!(matches!(second_url, Endpoint::Ready(_)));
    }

    #[test]
    fn a_repeated_tick_reuses_the_forward_instead_of_rebuilding_it() {
        let (mut manager, state, _) = manager();
        let tunneled = instance("a", Some("p1"), "http://127.0.0.1:11434");

        manager.endpoint_for(&tunneled, TEST_NOW);
        let first = manager.endpoint_for(&tunneled, TEST_NOW + 1);
        let second = manager.endpoint_for(&tunneled, TEST_NOW + 6);

        assert_eq!(first, second);
        assert_eq!(state.borrow().forward_calls, 1);
    }

    #[test]
    fn a_dead_forwarder_is_rebuilt_exactly_once() {
        let (mut manager, state, _) = manager();
        let tunneled = instance("a", Some("p1"), "http://127.0.0.1:11434");
        manager.endpoint_for(&tunneled, TEST_NOW);
        manager.endpoint_for(&tunneled, TEST_NOW + 1);
        assert_eq!(state.borrow().forward_calls, 1);

        // The forwarder thread exited.
        *manager.forwards["a"].alive.borrow_mut() = false;
        let rebuilt = manager.endpoint_for(&tunneled, TEST_NOW + 2);

        assert!(matches!(rebuilt, Endpoint::Ready(_)));
        assert_eq!(state.borrow().forward_calls, 2);
        // The SSH connection itself was not disturbed.
        assert_eq!(state.borrow().connect_calls, 1);
    }

    #[test]
    fn a_forwarder_that_lost_its_transport_forces_a_reconnect() {
        let (mut manager, state, live) = manager();
        let first = instance("a", Some("p1"), "http://127.0.0.1:11434");
        let second = instance("b", Some("p1"), "http://127.0.0.1:8000");
        manager.endpoint_for(&first, TEST_NOW);
        manager.endpoint_for(&first, TEST_NOW + 1);
        manager.endpoint_for(&second, TEST_NOW + 1);
        assert_eq!(state.borrow().connect_calls, 1);
        assert_eq!(*live.borrow(), 2);

        // The SSH transport died, so the forwarder stopped and said why.
        *manager.forwards["a"].alive.borrow_mut() = false;
        *manager.forwards["a"].hop_failed.borrow_mut() = true;

        // Rebuilding only the forward would bind a fresh port on a dead session
        // and fail every request forever, so the connection must go too.
        assert_eq!(
            manager.endpoint_for(&first, TEST_NOW + 2),
            Endpoint::Pending
        );
        assert_eq!(*live.borrow(), 0, "the sibling forward goes as well");
        assert_eq!(state.borrow().connect_calls, 2);

        // And the rebuilt connection serves both instances again.
        assert!(matches!(
            manager.endpoint_for(&first, TEST_NOW + 3),
            Endpoint::Ready(_)
        ));
        assert!(matches!(
            manager.endpoint_for(&second, TEST_NOW + 3),
            Endpoint::Ready(_)
        ));
        assert_eq!(state.borrow().connect_calls, 2);
    }

    #[test]
    fn a_forwarder_that_stopped_without_blaming_the_hop_only_rebuilds_the_forward() {
        let (mut manager, state, _) = manager();
        let tunneled = instance("a", Some("p1"), "http://127.0.0.1:11434");
        manager.endpoint_for(&tunneled, TEST_NOW);
        manager.endpoint_for(&tunneled, TEST_NOW + 1);

        // Stopped, but the transport still answered — the runtime was the problem.
        *manager.forwards["a"].alive.borrow_mut() = false;

        assert!(matches!(
            manager.endpoint_for(&tunneled, TEST_NOW + 2),
            Endpoint::Ready(_)
        ));
        // A working SSH login is not thrown away over a runtime that was down.
        assert_eq!(state.borrow().connect_calls, 1);
        assert_eq!(state.borrow().forward_calls, 2);
    }

    #[test]
    fn an_untrusted_host_fails_without_opening_a_socket() {
        let (mut manager, state, _) = manager();
        state.borrow_mut().trusted = false;
        let tunneled = instance("a", Some("p1"), "http://127.0.0.1:11434");

        let outcome = manager.endpoint_for(&tunneled, TEST_NOW);
        match outcome {
            Endpoint::Failed(error) => {
                assert_eq!(error.code, RuntimeErrorCode::SshHostUntrusted);
                assert!(error.message.contains("Test connection"), "{}", error.message);
            }
            other => panic!("expected a failure, got {other:?}"),
        }
        // The whole point: no connect attempt, so the interactive prompt path is
        // never reached from a background thread.
        assert_eq!(state.borrow().connect_calls, 0);
    }

    #[test]
    fn a_failed_connect_backs_off_before_trying_again() {
        let (mut manager, state, _) = manager();
        state.borrow_mut().connect_error = Some("Connection refused".to_string());
        let tunneled = instance("a", Some("p1"), "http://127.0.0.1:11434");

        assert_eq!(manager.endpoint_for(&tunneled, TEST_NOW), Endpoint::Pending);
        // The settled error is folded in on the next tick.
        let failed = manager.endpoint_for(&tunneled, TEST_NOW + 1);
        assert!(matches!(failed, Endpoint::Failed(_)));
        assert_eq!(state.borrow().connect_calls, 1);

        // Inside the first backoff step: the cached error is reported, no retry.
        let held = manager.endpoint_for(&tunneled, TEST_NOW + 3);
        assert!(matches!(held, Endpoint::Failed(_)));
        assert_eq!(state.borrow().connect_calls, 1);

        // Past it: exactly one new attempt.
        assert_eq!(
            manager.endpoint_for(&tunneled, TEST_NOW + 1 + SSH_BACKOFF_STEPS_SECS[0]),
            Endpoint::Pending
        );
        assert_eq!(state.borrow().connect_calls, 2);
    }

    #[test]
    fn the_backoff_climbs_and_is_capped() {
        assert_eq!(backoff_secs(0), 5, "treated as the first failure");
        assert_eq!(backoff_secs(1), 5);
        assert_eq!(backoff_secs(2), 15);
        assert_eq!(backoff_secs(3), 30);
        assert_eq!(backoff_secs(4), 60);
        assert_eq!(backoff_secs(99), 60);
    }

    #[test]
    fn a_connect_that_never_reports_becomes_a_failure_rather_than_hanging() {
        let (mut manager, state, _) = manager();
        state.borrow_mut().settle_immediately = false;
        let tunneled = instance("a", Some("p1"), "http://127.0.0.1:11434");

        assert_eq!(manager.endpoint_for(&tunneled, TEST_NOW), Endpoint::Pending);
        assert_eq!(
            manager.endpoint_for(&tunneled, TEST_NOW + CONNECT_TIMEOUT_SECS),
            Endpoint::Pending,
            "still inside the window"
        );

        let timed_out = manager.endpoint_for(&tunneled, TEST_NOW + CONNECT_TIMEOUT_SECS + 1);
        match timed_out {
            Endpoint::Failed(error) => assert_eq!(error.code, RuntimeErrorCode::SshTunnelError),
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    #[test]
    fn editing_the_profile_tears_down_both_forwards_and_reconnects_once() {
        let (mut manager, state, live) = manager();
        let first = instance("a", Some("p1"), "http://127.0.0.1:11434");
        let second = instance("b", Some("p1"), "http://127.0.0.1:8000");
        manager.endpoint_for(&first, TEST_NOW);
        manager.endpoint_for(&first, TEST_NOW + 1);
        manager.endpoint_for(&second, TEST_NOW + 1);
        assert_eq!(*live.borrow(), 2);
        assert_eq!(state.borrow().connect_calls, 1);

        // The user changed the profile's port.
        state.borrow_mut().signature = "user@host:2222|-".to_string();

        assert_eq!(
            manager.endpoint_for(&first, TEST_NOW + 2),
            Endpoint::Pending,
            "the old connection is gone, so this reconnects"
        );
        // Both forwards were released, not just the one being polled.
        assert_eq!(*live.borrow(), 0);
        assert_eq!(state.borrow().connect_calls, 2);
    }

    #[test]
    fn retain_releases_a_removed_instance_and_then_its_idle_connection() {
        let (mut manager, state, live) = manager();
        let first = instance("a", Some("p1"), "http://127.0.0.1:11434");
        let second = instance("b", Some("p1"), "http://127.0.0.1:8000");
        manager.endpoint_for(&first, TEST_NOW);
        manager.endpoint_for(&first, TEST_NOW + 1);
        manager.endpoint_for(&second, TEST_NOW + 1);
        assert_eq!(*live.borrow(), 2);

        // One instance was disabled or deleted.
        manager.retain(&HashSet::from(["a".to_string()]));
        assert_eq!(*live.borrow(), 1);
        assert!(manager.hosts.contains_key("p1"), "still carrying a forward");

        // The last one goes: the SSH connection has nothing left to carry.
        manager.retain(&HashSet::new());
        assert_eq!(*live.borrow(), 0);
        assert!(manager.hosts.is_empty());

        // Re-enabling reconnects rather than reusing a released connection.
        manager.endpoint_for(&first, TEST_NOW + 2);
        assert_eq!(state.borrow().connect_calls, 2);
    }

    #[test]
    fn shutdown_releases_everything() {
        let (mut manager, _, live) = manager();
        let tunneled = instance("a", Some("p1"), "http://127.0.0.1:11434");
        manager.endpoint_for(&tunneled, TEST_NOW);
        manager.endpoint_for(&tunneled, TEST_NOW + 1);
        assert_eq!(*live.borrow(), 1);

        manager.shutdown();

        assert_eq!(*live.borrow(), 0);
        assert!(manager.hosts.is_empty());
        assert!(manager.forwards.is_empty());
        assert!(manager.owners.is_empty());
        assert!(manager.pending.is_empty());
    }

    #[test]
    fn an_unparseable_address_fails_without_reaching_the_ssh_layer() {
        let (mut manager, state, _) = manager();
        let broken = instance("a", Some("p1"), "not-a-url");

        match manager.endpoint_for(&broken, TEST_NOW) {
            Endpoint::Failed(error) => assert_eq!(error.code, RuntimeErrorCode::SshTunnelError),
            other => panic!("expected a failure, got {other:?}"),
        }
        assert_eq!(state.borrow().resolve_calls, 0);
    }

    #[test]
    fn the_untrusted_sentinel_is_translated_and_never_leaked() {
        let sentinel = format!("{}abc123|ssh-ed25519|host:22", UNKNOWN_HOST_KEY_PREFIX);
        let error = classify_ssh_error("host:22", &sentinel);

        assert_eq!(error.code, RuntimeErrorCode::SshHostUntrusted);
        // The raw sentinel is an internal protocol; showing it to a user would
        // be meaningless, and it can never reach the prompt from a thread.
        assert!(!error.message.contains(UNKNOWN_HOST_KEY_PREFIX), "{}", error.message);
        assert!(error.message.contains("host key"), "{}", error.message);

        let ordinary = classify_ssh_error("host:22", "Connection refused");
        assert_eq!(ordinary.code, RuntimeErrorCode::SshTunnelError);
        assert!(ordinary.message.contains("Connection refused"));
    }
}
