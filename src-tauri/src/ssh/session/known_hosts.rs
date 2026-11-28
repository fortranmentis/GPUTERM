use super::config_dir;
use super::types::SshTarget;
use crate::ssh::kbdint::INTERACTIVE_AUTH_PREFIX;
use serde::{Deserialize, Serialize};
use ssh2::{HashType, HostKeyType, Session};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as FmtWrite;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

/// Serializes read-modify-write of `known_hosts.json`: terminal, telemetry
/// and LLM tunnels connect in parallel, and an unserialized trust or legacy
/// migration could overwrite another one's entry.
static KNOWN_HOSTS_WRITE_LOCK: Mutex<()> = Mutex::new(());

fn lock_known_hosts() -> Result<std::sync::MutexGuard<'static, ()>, String> {
    KNOWN_HOSTS_WRITE_LOCK
        .lock()
        .map_err(|_| "Known hosts storage is unavailable".to_string())
}

/// Prefix of the sentinel error raised when connecting to a host whose key has
/// never been seen. Format: `UNKNOWN_HOST_KEY:{fingerprint}|{key_type}|{host}:{port}`
/// — the fingerprint is hex and the key type contains no `|`, so the first two
/// `|` delimiters are unambiguous (IPv6 hosts may contain `:`).
pub const UNKNOWN_HOST_KEY_PREFIX: &str = "UNKNOWN_HOST_KEY:";

/// Prefix of the sentinel error raised when a host presents a different key
/// than the one trusted for its key type. Format:
/// `HOST_KEY_CHANGED:{expected}|{fingerprint}|{key_type}|{host}:{port}`. The
/// user can replace the key with `trust_host_key`, which overwrites the
/// fingerprint for that key type.
pub const HOST_KEY_CHANGED_PREFIX: &str = "HOST_KEY_CHANGED:";

/// Whether `error` is one of the host-key sentinels the interactive prompt
/// consumes, which callers must pass through without rewording.
pub fn is_host_key_sentinel(error: &str) -> bool {
    error.starts_with(UNKNOWN_HOST_KEY_PREFIX) || error.starts_with(HOST_KEY_CHANGED_PREFIX)
}

/// Rewords a host-key sentinel for places that cannot show the prompt
/// (background telemetry, quota probes); other errors pass through unchanged.
pub fn readable_connect_error(error: &str) -> String {
    if let Some(host) = error.strip_prefix(INTERACTIVE_AUTH_PREFIX) {
        return format!(
            "{} asks for a password or one-time code at sign-in, which GpuTerm cannot enter \
             on its own. Reconnect the session to sign in.",
            host
        );
    }
    if let Some(rest) = error.strip_prefix(HOST_KEY_CHANGED_PREFIX) {
        let host = rest.splitn(4, '|').nth(3).unwrap_or("this host");
        return format!(
            "The host key for {} has changed. Reconnect the session to review it.",
            host
        );
    }
    if let Some(rest) = error.strip_prefix(UNKNOWN_HOST_KEY_PREFIX) {
        let host = rest.splitn(3, '|').nth(2).unwrap_or("this host");
        return format!(
            "{} has not been trusted yet. Reconnect the session to review its host key.",
            host
        );
    }
    error.to_string()
}

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
pub(super) enum KnownHostEntry {
    PerType(BTreeMap<String, String>),
    Legacy(String),
}

pub(super) type KnownHosts = HashMap<String, KnownHostEntry>;

#[derive(Debug, PartialEq)]
pub(super) enum HostKeyDecision {
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

pub(super) fn evaluate_host_key(
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

pub(super) fn trust_host_key_entry(
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
    let _write_guard = lock_known_hosts()?;
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

pub(super) fn verify_known_host(session: &Session, target: &SshTarget) -> Result<(), String> {
    let fingerprint = session
        .host_key_hash(HashType::Sha256)
        .map(bytes_to_hex)
        .ok_or_else(|| "Unable to read remote host key fingerprint".to_string())?;
    let key_type = session
        .host_key()
        .map(|(_, key_type)| host_key_type_name(key_type))
        .ok_or_else(|| "Unable to determine remote host key type".to_string())?;
    let host_key = format!("{}:{}", target.host.to_lowercase(), target.port);
    let known_hosts = read_known_hosts()?;

    match evaluate_host_key(known_hosts.get(&host_key), key_type, &fingerprint) {
        HostKeyDecision::Trusted => Ok(()),
        HostKeyDecision::TrustedMigrateLegacy => {
            // The connection is already verified; a failed rewrite must not
            // block it. The migration will be retried on the next connect.
            let _ = migrate_legacy_host_key(&host_key, key_type, &fingerprint);
            Ok(())
        }
        HostKeyDecision::Mismatch { expected } => Err(format!(
            "{}{}|{}|{}|{}",
            HOST_KEY_CHANGED_PREFIX, expected, fingerprint, key_type, host_key
        )),
        HostKeyDecision::Unknown => Err(format!(
            "{}{}|{}|{}",
            UNKNOWN_HOST_KEY_PREFIX, fingerprint, key_type, host_key
        )),
    }
}

/// Rewrites a legacy flat fingerprint under its negotiated key type. Re-reads
/// under the write lock so a trust recorded since the caller's read survives.
fn migrate_legacy_host_key(
    host_key: &str,
    key_type: &str,
    fingerprint: &str,
) -> Result<(), String> {
    let _write_guard = lock_known_hosts()?;
    let mut known_hosts = read_known_hosts()?;
    if evaluate_host_key(known_hosts.get(host_key), key_type, fingerprint)
        != HostKeyDecision::TrustedMigrateLegacy
    {
        return Ok(());
    }
    let mut fingerprints = BTreeMap::new();
    fingerprints.insert(key_type.to_string(), fingerprint.to_string());
    known_hosts.insert(host_key.to_string(), KnownHostEntry::PerType(fingerprints));
    write_known_hosts(&known_hosts)
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

fn known_hosts_path() -> PathBuf {
    config_dir().join("known_hosts.json")
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(&mut output, "{:02x}", byte);
    }
    output
}
