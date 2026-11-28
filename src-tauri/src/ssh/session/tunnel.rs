use super::known_hosts::is_host_key_sentinel;
use super::types::{SshConnection, SshTarget, SshTunnel};
use super::{format_host_port, host_without_brackets, open_ssh_session, open_ssh_session_with};
use crate::ssh::kbdint::{AuthResponder, INTERACTIVE_AUTH_PREFIX};
use ssh2::{Channel, Session};
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// Connects the bastion, opens a direct-tcpip channel to `target` through it,
/// and pumps that channel over a local loopback socket so the caller can run a
/// normal SSH handshake against the target.
pub(super) fn open_tunneled_stream(
    target: &SshTarget,
    proxy: &SshTarget,
    auth: &mut dyn AuthResponder,
) -> Result<(TcpStream, Option<SshTunnel>), String> {
    let bastion = open_ssh_session_with(proxy, auth).map_err(|error| {
        if is_host_key_sentinel(&error) || error.starts_with(INTERACTIVE_AUTH_PREFIX) {
            // Keep the sentinel intact so the frontend can prompt for the
            // bastion's own host key.
            error
        } else {
            format!(
                "Jump host {}: {}",
                format_host_port(&proxy.host, proxy.port),
                error
            )
        }
    })?;

    let channel = bastion
        .session
        .channel_direct_tcpip(host_without_brackets(&target.host), target.port, None)
        .map_err(|error| {
            format!(
                "Jump host could not open a channel to {}: {}",
                format_host_port(&target.host, target.port),
                error
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

    pump_channel_socket(socket, channel, &stop, None);
}

pub(super) trait SocketIo {
    fn set_nonblocking_io(&mut self) -> std::io::Result<()>;
    fn read_bytes(&mut self, buf: &mut [u8]) -> std::io::Result<usize>;
    fn write_bytes(&mut self, buf: &[u8]) -> std::io::Result<usize>;
    fn shutdown_write(&mut self);
}

impl SocketIo for TcpStream {
    fn set_nonblocking_io(&mut self) -> std::io::Result<()> {
        self.set_nonblocking(true)
    }

    fn read_bytes(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.read(buf)
    }

    fn write_bytes(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.write(buf)
    }

    fn shutdown_write(&mut self) {
        let _ = self.shutdown(std::net::Shutdown::Write);
    }
}

pub(super) trait ChannelIo {
    fn read_bytes(&mut self, buf: &mut [u8]) -> std::io::Result<usize>;
    fn write_bytes(&mut self, buf: &[u8]) -> std::io::Result<usize>;
    fn is_eof(&self) -> bool;
    fn send_channel_eof(&mut self) -> Result<(), ErrorKind>;
    fn close_channel(&mut self);
}

impl ChannelIo for Channel {
    fn read_bytes(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.read(buf)
    }

    fn write_bytes(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.write(buf)
    }

    fn is_eof(&self) -> bool {
        self.eof()
    }

    fn send_channel_eof(&mut self) -> Result<(), ErrorKind> {
        self.send_eof().map_err(|error| {
            if is_would_block(&error) {
                ErrorKind::WouldBlock
            } else {
                ErrorKind::Other
            }
        })
    }

    fn close_channel(&mut self) {
        let _ = self.close();
    }
}

/// Pumps bytes between one loopback socket and one direct-tcpip channel until
/// the channel closes or the stop flag is set, preserving local half-close
/// (`socket.read() == Ok(0)` → `channel.send_eof()`, then keep draining
/// remote-to-local bytes until the channel hits EOF).
///
/// Both the session and the socket must already be nonblocking: a blocking read
/// on a `Channel` holds the `Session` mutex across the syscall and would stall
/// every other channel on the same connection.
pub(crate) fn pump_channel_socket(
    socket: TcpStream,
    channel: Channel,
    stop: &AtomicBool,
    bytes: Option<&std::sync::atomic::AtomicU64>,
) {
    pump_socket_and_channel(socket, channel, stop, bytes);
}

pub(super) fn pump_socket_and_channel(
    mut socket: impl SocketIo,
    mut channel: impl ChannelIo,
    stop: &AtomicBool,
    bytes: Option<&std::sync::atomic::AtomicU64>,
) {
    use std::sync::atomic::Ordering;

    if socket.set_nonblocking_io().is_err() {
        channel.close_channel();
        return;
    }

    let mut to_channel: Vec<u8> = Vec::new();
    let mut to_socket: Vec<u8> = Vec::new();
    let mut buffer = [0_u8; 32 * 1024];
    let mut idle_sleep = Duration::from_millis(2);
    let max_sleep = Duration::from_millis(15);
    let mut socket_eof = false;
    let mut channel_eof_sent = false;
    let mut channel_eof = false;

    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let mut progressed = false;

        // Socket -> channel (only read more once the pending buffer is flushed).
        if !socket_eof && to_channel.is_empty() {
            match socket.read_bytes(&mut buffer) {
                Ok(0) => {
                    socket_eof = true;
                    progressed = true;
                }
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
            match channel.write_bytes(&to_channel) {
                Ok(0) => {}
                Ok(count) => {
                    to_channel.drain(..count);
                    if let Some(bytes) = bytes {
                        bytes.fetch_add(count as u64, Ordering::Relaxed);
                    }
                    progressed = true;
                }
                Err(ref error)
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
                Err(_) => break,
            }
        }
        if socket_eof && to_channel.is_empty() && !channel_eof_sent {
            match channel.send_channel_eof() {
                Ok(()) => {
                    channel_eof_sent = true;
                    progressed = true;
                }
                Err(ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
                Err(_) => {
                    channel_eof_sent = true;
                }
            }
        }

        // Channel -> socket.
        if !channel_eof && to_socket.is_empty() {
            match channel.read_bytes(&mut buffer) {
                Ok(0) => {
                    if channel.is_eof() {
                        channel_eof = true;
                        progressed = true;
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
            match socket.write_bytes(&to_socket) {
                Ok(0) => {}
                Ok(count) => {
                    to_socket.drain(..count);
                    if let Some(bytes) = bytes {
                        bytes.fetch_add(count as u64, Ordering::Relaxed);
                    }
                    progressed = true;
                }
                Err(ref error)
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
                Err(_) => break,
            }
        }
        if channel_eof && to_socket.is_empty() {
            socket.shutdown_write();
            break;
        }

        if progressed {
            idle_sleep = Duration::from_millis(2);
        } else {
            thread::sleep(idle_sleep);
            idle_sleep = (idle_sleep * 2).min(max_sleep);
        }
    }

    channel.close_channel();
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
pub(super) const LIBSSH2_ERROR_EAGAIN: i32 = -37;

/// Simultaneous forwarded connections per instance.
///
/// One is the steady state; a second appears briefly when the runtime closes an
/// idle keep-alive socket and the HTTP client dials a fresh one. Kept well below
/// sshd's default `MaxSessions 10`, since one SSH connection hosts every
/// forward for that host.
const MAX_FORWARD_CONNECTIONS: usize = 4;

/// How long a forwarder waits for the server to answer a liveness probe.
pub(crate) const HOP_PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Consecutive channel-open failures before a forwarder gives up.
///
/// A backstop for the case where the transport still answers a keepalive but
/// channels cannot be opened (an sshd at its `MaxSessions`, for instance), so a
/// forward can never be stuck alive-but-useless.
const MAX_CONSECUTIVE_OPEN_FAILURES: u32 = 5;

pub(crate) fn is_would_block(error: &ssh2::Error) -> bool {
    matches!(error.code(), ssh2::ErrorCode::Session(code) if code == LIBSSH2_ERROR_EAGAIN)
}

/// Lets the OS notice a vanished peer on its own: an SSH keepalive only
/// queues bytes, which on a half-open connection can sit unacknowledged until
/// TCP's default retry limit (many minutes) runs out. Best effort — a
/// platform that refuses an option keeps its defaults. A jump-host tunnel's
/// loopback socket is not covered; its bastion connection is.
pub(super) fn enable_tcp_keepalive(tcp: &TcpStream) {
    let socket = socket2::SockRef::from(tcp);
    let keepalive = socket2::TcpKeepalive::new()
        .with_time(Duration::from_secs(30))
        .with_interval(Duration::from_secs(10));
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    let keepalive = keepalive.with_retries(3);
    let _ = socket.set_tcp_keepalive(&keepalive);
    // Also bounds how long sent data may stay unacknowledged.
    #[cfg(target_os = "linux")]
    let _ = socket.set_tcp_user_timeout(Some(Duration::from_secs(60)));
}

/// libssh2's `LIBSSH2_ERROR_CHANNEL_FAILURE`: the server refused to open a
/// channel — which, for the probe below, is the expected answer.
pub(super) const LIBSSH2_ERROR_CHANNEL_FAILURE: i32 = -21;
/// `LIBSSH2_ERROR_CHANNEL_REQUEST_DENIED`, also an answer from the server.
pub(super) const LIBSSH2_ERROR_CHANNEL_REQUEST_DENIED: i32 = -22;
/// A channel type no server implements. RFC 4254 requires the server to
/// answer an open of an unknown type with `SSH_MSG_CHANNEL_OPEN_FAILURE`, so
/// asking for one is a request that always gets a reply, costs the server
/// nothing, and does not count against `MaxSessions`.
const PROBE_CHANNEL_TYPE: &str = "ping@gputerm";

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ProbeOutcome {
    Alive,
    /// The request is still in flight (nonblocking session).
    Pending,
    Dead(String),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ProbeFailure {
    /// Nothing came back within the time allowed.
    NoReply,
    /// The transport itself failed.
    Transport(String),
}

impl std::fmt::Display for ProbeFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProbeFailure::NoReply => write!(f, "the server stopped responding"),
            ProbeFailure::Transport(error) => write!(f, "{}", error),
        }
    }
}

/// What one attempt at the probe channel open says about the connection.
pub(crate) fn classify_probe(result: Result<(), &ssh2::Error>) -> ProbeOutcome {
    let Err(error) = result else {
        // A server that actually opened the channel answered all the same.
        return ProbeOutcome::Alive;
    };
    match error.code() {
        ssh2::ErrorCode::Session(code)
            if code == LIBSSH2_ERROR_CHANNEL_FAILURE
                || code == LIBSSH2_ERROR_CHANNEL_REQUEST_DENIED =>
        {
            ProbeOutcome::Alive
        }
        ssh2::ErrorCode::Session(code) if code == LIBSSH2_ERROR_EAGAIN => ProbeOutcome::Pending,
        _ => ProbeOutcome::Dead(error.to_string()),
    }
}

/// Checks that the server still answers, by asking it to open a channel type
/// it does not know and waiting for the refusal. Unlike `session_alive`, which
/// only queues a keepalive, this needs a reply, so a half-open connection
/// fails within `timeout`.
///
/// `lock` serializes the probe with other users of the session: a channel
/// open must not interleave with another (see `SshConnection::open_lock`) or
/// with a partially sent packet. With `release_while_waiting`, the lock is
/// dropped once the request is fully sent, so a terminal can keep sending
/// keystrokes while the reply is awaited; only safe when nothing else opens
/// channels on the session.
pub(crate) fn probe_reply<G>(
    session: &Session,
    timeout: Duration,
    release_while_waiting: bool,
    mut lock: impl FnMut() -> Result<G, String>,
) -> Result<(), ProbeFailure> {
    let blocking = session.is_blocking();
    let previous_timeout = session.timeout();
    if blocking {
        session.set_timeout(timeout.as_millis().clamp(1, u32::MAX as u128) as u32);
    }
    let deadline = Instant::now() + timeout;
    let mut wait = Duration::from_millis(2);
    let mut guard = None;
    let result = loop {
        if guard.is_none() {
            guard = Some(lock().map_err(ProbeFailure::Transport)?);
        }
        let attempt = session.channel_open(PROBE_CHANNEL_TYPE, 0x20_0000, 0x8000, None);
        let outcome = match &attempt {
            Ok(_) => classify_probe(Ok(())),
            Err(error) => classify_probe(Err(error)),
        };
        if let Ok(mut channel) = attempt {
            let _ = channel.close();
        }
        match outcome {
            ProbeOutcome::Alive => break Ok(()),
            ProbeOutcome::Dead(_) if blocking && Instant::now() >= deadline => {
                // A blocking session reports the expired timeout as an error.
                break Err(ProbeFailure::NoReply);
            }
            ProbeOutcome::Dead(error) => break Err(ProbeFailure::Transport(error)),
            ProbeOutcome::Pending => {
                if Instant::now() >= deadline {
                    break Err(ProbeFailure::NoReply);
                }
                let request_sent = !matches!(
                    session.block_directions(),
                    ssh2::BlockDirections::Outbound | ssh2::BlockDirections::Both
                );
                if release_while_waiting && request_sent {
                    guard = None;
                }
                thread::sleep(wait);
                wait = (wait * 2).min(Duration::from_millis(50));
            }
        }
    };
    if blocking {
        session.set_timeout(previous_timeout);
    }
    result
}

/// `probe_reply` for a connection that may host several channel opens at
/// once (forwarding, ops): holds the connection's open lock throughout.
pub(crate) fn connection_answers(connection: &SshConnection, timeout: Duration) -> bool {
    probe_reply(&connection.session, timeout, false, || {
        Ok(connection
            .open_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()))
    })
    .is_ok()
}

/// Opens a direct-tcpip channel on a **nonblocking** session.
///
/// This retry loop is the whole reason the function exists. The ProxyJump path
/// opens its channel while the session is still blocking and so never sees
/// `EAGAIN`; on a nonblocking session the very first call usually returns it.
/// Treating that as a failure would leave a bound port that never serves
/// anything — a silent dead tunnel rather than a visible error.
pub(crate) fn open_direct_channel(
    connection: &SshConnection,
    host: &str,
    port: u16,
    deadline: Instant,
) -> Result<Channel, String> {
    let session = &connection.session;
    // For the whole retry loop: each EAGAIN retry continues the same open.
    let _open = connection
        .open_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
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
pub(crate) struct CountGuard(pub(crate) Arc<std::sync::atomic::AtomicUsize>);

/// Takes one of `limit` slots, counted before any work starts so that work
/// still in progress (a SOCKS handshake, a channel open) counts against the
/// limit. The slot is released when the guard drops.
pub(crate) fn try_reserve_slot(
    counter: &Arc<std::sync::atomic::AtomicUsize>,
    limit: usize,
) -> Option<CountGuard> {
    use std::sync::atomic::Ordering;
    // compare_exchange keeps this usable on the declared MSRV as well as
    // newer toolchains that deprecated fetch_update in favour of try_update.
    let mut count = counter.load(Ordering::SeqCst);
    loop {
        if count >= limit {
            return None;
        }
        match counter.compare_exchange(count, count + 1, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return Some(CountGuard(Arc::clone(counter))),
            Err(current) => count = current,
        }
    }
}

/// Sleep for a polling loop that has nothing to do: short right after
/// activity, so a burst of connections is served promptly, and doubling up to
/// `max` while idle, so an unused forward does not wake hundreds of times a
/// second.
pub(crate) struct IdleBackoff {
    current: Duration,
    min: Duration,
    max: Duration,
}

impl IdleBackoff {
    pub(crate) fn new(min: Duration, max: Duration) -> Self {
        Self {
            current: min,
            min,
            max,
        }
    }

    /// For forward accept loops: 2 ms after activity, at most 100 ms idle.
    pub(crate) fn for_accept_loop() -> Self {
        Self::new(Duration::from_millis(2), Duration::from_millis(100))
    }

    pub(crate) fn reset(&mut self) {
        self.current = self.min;
    }

    /// The delay the next `sleep` would use.
    #[cfg(test)]
    pub(crate) fn next_delay(&self) -> Duration {
        self.current
    }

    pub(crate) fn sleep(&mut self) {
        thread::sleep(self.current);
        self.current = (self.current * 2).min(self.max);
    }
}

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
    let mut idle = IdleBackoff::for_accept_loop();

    while !stop.load(Ordering::SeqCst) {
        let socket = match listener.accept() {
            Ok((socket, _)) => socket,
            Err(ref error) if error.kind() == ErrorKind::WouldBlock => {
                idle.sleep();
                continue;
            }
            // The listener itself failed; nothing can be served any more.
            Err(_) => break,
        };
        idle.reset();

        // Refuse rather than queue: the client retries, and an unbounded
        // channel count would eventually hit sshd's MaxSessions.
        let Some(slot) = try_reserve_slot(&live, MAX_FORWARD_CONNECTIONS) else {
            drop(socket);
            continue;
        };

        let deadline = Instant::now() + Duration::from_secs(10);
        let channel =
            match open_direct_channel(&connection, &remote_host, remote_port, deadline) {
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
                    if !connection_answers(&connection, HOP_PROBE_TIMEOUT) {
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

        let pump_stop = Arc::clone(&stop);
        thread::spawn(move || {
            let _slot = slot;
            pump_channel_socket(socket, channel, &pump_stop, None);
        });
    }
}
