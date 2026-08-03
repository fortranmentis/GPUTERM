//! Registered Ollama and vLLM instances: validation, persistence, and secrets.
//!
//! The API key is deliberately absent from `LlmInstance`. It lives in the
//! existing credential vault and never crosses the IPC boundary, so it cannot
//! reach the webview, `localStorage`, or a log line.

use crate::ssh::credentials::write_json_file;
use crate::ssh::session::config_dir;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use super::adapter::RuntimeType;

/// Vault keys are one flat namespace shared with SSH profiles, so LLM secrets
/// are prefixed to make a collision impossible.
pub const VAULT_KEY_PREFIX: &str = "llm:";

pub const DEFAULT_REQUEST_TIMEOUT_MS: u64 = 3_000;
pub const DEFAULT_POLL_INTERVAL_SECS: u64 = 5;
const MIN_POLL_INTERVAL_SECS: u64 = 1;
const MAX_POLL_INTERVAL_SECS: u64 = 300;
const MIN_REQUEST_TIMEOUT_MS: u64 = 500;
const MAX_REQUEST_TIMEOUT_MS: u64 = 60_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LlmInstance {
    pub id: String,
    pub name: String,
    pub runtime_type: RuntimeType,
    pub base_url: String,
    pub enabled: bool,
    pub request_timeout_ms: u64,
    pub poll_interval_secs: u64,
    pub created_at: u64,
    pub updated_at: u64,
    /// Id of a saved SSH profile to tunnel the poll through. When set,
    /// `base_url` is resolved on **that host's** network, so `127.0.0.1` means
    /// the runtime's own loopback rather than this machine's.
    ///
    /// `default` plus `skip_serializing_if` is the whole migration story: an
    /// older file loads as `None` and a file with no tunneled instance
    /// re-serializes byte-identically.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_profile_id: Option<String>,
}

pub fn vault_key(instance_id: &str) -> String {
    format!("{}{}", VAULT_KEY_PREFIX, instance_id)
}

pub fn instances_path() -> PathBuf {
    config_dir().join("llm_instances.json")
}

/// Reads the instance list from an explicit path.
///
/// The path is a parameter because `config_dir()` cannot be injected, and
/// without this seam the file layer would be untestable — the same reason
/// `SecureCredentialStore::with_paths` exists.
pub fn read_instances_at(path: &Path) -> Result<Vec<LlmInstance>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(path).map_err(|error| {
        format!(
            "Failed to read LLM instances file {}: {}",
            path.display(),
            error
        )
    })?;
    if content.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&content).map_err(|error| {
        format!(
            "Failed to parse LLM instances file {}: {}",
            path.display(),
            error
        )
    })
}

pub fn write_instances_at(path: &Path, instances: &[LlmInstance]) -> Result<(), String> {
    write_json_file(path, &instances, "LLM instances file")
}

pub fn read_instances() -> Result<Vec<LlmInstance>, String> {
    read_instances_at(&instances_path())
}

pub fn write_instances(instances: &[LlmInstance]) -> Result<(), String> {
    write_instances_at(&instances_path(), instances)
}

/// Validates and canonicalizes a base URL.
///
/// Only `http` and `https` are accepted, a host is required, and any trailing
/// slash is removed so `http://h:1/` and `http://h:1` are the same instance.
pub fn normalize_base_url(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Address is required".to_string());
    }

    let (scheme, rest) = trimmed
        .split_once("://")
        .ok_or_else(|| "Address must start with http:// or https://".to_string())?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(format!(
            "Unsupported address scheme `{}`. Only http and https are allowed.",
            scheme
        ));
    }

    // Keep only the origin: a path, query, or fragment is not part of an
    // instance's identity, and adapters append their own absolute paths.
    let authority_end = rest
        .find(['/', '?', '#'])
        .unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() {
        return Err("Address is missing a host".to_string());
    }
    if authority.contains('@') {
        return Err(
            "Address must not contain credentials. Use the API key field instead.".to_string(),
        );
    }
    if authority.contains(char::is_whitespace) {
        return Err("Address must not contain spaces".to_string());
    }

    // Reject a bare port or an empty host such as `http://:8000`.
    let host = authority.split(':').next().unwrap_or_default();
    if host.is_empty() {
        return Err("Address is missing a host".to_string());
    }
    if let Some((_, port)) = authority.rsplit_once(':') {
        // An IPv6 literal has colons inside brackets; only validate a real port.
        if !authority.ends_with(']') && !port.is_empty() && port.parse::<u16>().is_err() {
            return Err(format!("Address has an invalid port `{}`", port));
        }
    }

    Ok(format!("{}://{}", scheme, authority.to_ascii_lowercase()))
}

/// Splits a normalized base URL into the host and port to open on the far side
/// of a tunnel.
///
/// The port is required by `direct-tcpip`, so a URL that omits it falls back to
/// the scheme's default. IPv6 brackets are stripped because the channel wants a
/// bare address.
pub fn base_url_endpoint(base_url: &str) -> Result<(String, u16), String> {
    let normalized = normalize_base_url(base_url)?;
    let (scheme, authority) = normalized
        .split_once("://")
        .ok_or_else(|| "Address is missing a scheme".to_string())?;

    if let Some(rest) = authority.strip_prefix('[') {
        let (host, tail) = rest
            .split_once(']')
            .ok_or_else(|| "Address has an unterminated IPv6 literal".to_string())?;
        let port = match tail.strip_prefix(':') {
            Some(port) => port
                .parse::<u16>()
                .map_err(|_| format!("Address has an invalid port `{}`", port))?,
            None => default_port(scheme),
        };
        return Ok((host.to_string(), port));
    }

    match authority.rsplit_once(':') {
        Some((host, port)) => {
            let port = port
                .parse::<u16>()
                .map_err(|_| format!("Address has an invalid port `{}`", port))?;
            Ok((host.to_string(), port))
        }
        None => Ok((authority.to_string(), default_port(scheme))),
    }
}

fn default_port(scheme: &str) -> u16 {
    if scheme == "https" {
        443
    } else {
        80
    }
}

/// The loopback URL a tunneled instance is actually polled at.
pub fn tunnel_base_url(local_port: u16) -> String {
    format!("http://127.0.0.1:{}", local_port)
}

/// Keeps user-supplied numbers inside a sane band instead of rejecting them,
/// matching how telemetry settings are handled elsewhere.
pub fn sanitize_instance(mut instance: LlmInstance) -> Result<LlmInstance, String> {
    instance.base_url = normalize_base_url(&instance.base_url)?;
    instance.ssh_profile_id = instance
        .ssh_profile_id
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty());
    if instance.ssh_profile_id.is_some() && instance.base_url.starts_with("https://") {
        // The request would reach 127.0.0.1:<ephemeral>, so the certificate is
        // validated against that address and a normal one cannot match.
        // Refusing here beats an eventual, unexplained TLS failure.
        return Err(
            "An https address cannot be reached through an SSH tunnel: the certificate would be \
             checked against 127.0.0.1. Use http, or poll this instance directly."
                .to_string(),
        );
    }
    instance.name = instance.name.trim().to_string();
    if instance.name.is_empty() {
        instance.name = instance.base_url.clone();
    }
    // Zero means "not set" — an omitted field should get the documented default
    // rather than the fastest possible poll.
    if instance.poll_interval_secs == 0 {
        instance.poll_interval_secs = DEFAULT_POLL_INTERVAL_SECS;
    }
    if instance.request_timeout_ms == 0 {
        instance.request_timeout_ms = DEFAULT_REQUEST_TIMEOUT_MS;
    }
    instance.poll_interval_secs = instance
        .poll_interval_secs
        .clamp(MIN_POLL_INTERVAL_SECS, MAX_POLL_INTERVAL_SECS);
    instance.request_timeout_ms = instance
        .request_timeout_ms
        .clamp(MIN_REQUEST_TIMEOUT_MS, MAX_REQUEST_TIMEOUT_MS);
    Ok(instance)
}

/// Inserts or replaces an instance, rejecting a duplicate runtime + address.
pub fn upsert(
    instances: &mut Vec<LlmInstance>,
    instance: LlmInstance,
    now: u64,
) -> Result<(), String> {
    let mut instance = sanitize_instance(instance)?;

    // The hop is part of the identity: two hosts can each legitimately be
    // `http://127.0.0.1:11434`. Two `None`s compare equal, so the rule for
    // directly polled instances is unchanged.
    let duplicate = instances.iter().any(|existing| {
        existing.id != instance.id
            && existing.runtime_type == instance.runtime_type
            && existing.base_url == instance.base_url
            && existing.ssh_profile_id == instance.ssh_profile_id
    });
    if duplicate {
        return Err(format!(
            "{} is already registered at {}{}",
            instance.runtime_type,
            instance.base_url,
            // Without this the message reads "already registered at
            // http://127.0.0.1:11434" and the user cannot tell which host.
            if instance.ssh_profile_id.is_some() {
                " through the same SSH session"
            } else {
                ""
            }
        ));
    }

    match instances
        .iter_mut()
        .find(|existing| existing.id == instance.id)
    {
        Some(existing) => {
            instance.created_at = existing.created_at;
            instance.updated_at = now;
            *existing = instance;
        }
        None => {
            if instance.created_at == 0 {
                instance.created_at = now;
            }
            instance.updated_at = now;
            instances.push(instance);
        }
    }
    instances.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_NOW: u64 = 1_799_000_000;

    fn instance(id: &str, runtime_type: RuntimeType, base_url: &str) -> LlmInstance {
        LlmInstance {
            id: id.to_string(),
            name: format!("instance {id}"),
            runtime_type,
            base_url: base_url.to_string(),
            enabled: true,
            request_timeout_ms: DEFAULT_REQUEST_TIMEOUT_MS,
            poll_interval_secs: DEFAULT_POLL_INTERVAL_SECS,
            created_at: 0,
            updated_at: 0,
            ssh_profile_id: None,
        }
    }

    #[test]
    fn base_urls_are_normalized_regardless_of_a_trailing_slash() {
        assert_eq!(
            normalize_base_url("http://192.168.0.20:11434/").unwrap(),
            "http://192.168.0.20:11434"
        );
        assert_eq!(
            normalize_base_url("  http://192.168.0.20:11434  ").unwrap(),
            "http://192.168.0.20:11434"
        );
        // A path is not part of the instance identity; adapters add their own.
        assert_eq!(
            normalize_base_url("https://vllm.example.com/v1/").unwrap(),
            "https://vllm.example.com"
        );
        assert_eq!(
            normalize_base_url("HTTP://Host:8000").unwrap(),
            "http://host:8000"
        );
    }

    #[test]
    fn only_http_and_https_are_accepted() {
        for bad in [
            "ftp://host:21",
            "file:///etc/passwd",
            "ws://host:8000",
            "host:11434",
            "",
            "   ",
            "http://",
            "http://:8000",
        ] {
            assert!(
                normalize_base_url(bad).is_err(),
                "{bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn credentials_in_the_address_are_refused() {
        // Otherwise a secret would be persisted in plaintext in the config file.
        let error = normalize_base_url("http://user:pw@host:8000").unwrap_err();
        assert!(error.contains("must not contain credentials"), "{error}");
    }

    #[test]
    fn an_invalid_port_is_rejected_but_ipv6_is_not() {
        assert!(normalize_base_url("http://host:not-a-port").is_err());
        assert_eq!(
            normalize_base_url("http://[::1]:11434").unwrap(),
            "http://[::1]:11434"
        );
    }

    #[test]
    fn the_same_runtime_and_address_cannot_be_registered_twice() {
        let mut instances = Vec::new();
        upsert(
            &mut instances,
            instance("a", RuntimeType::Ollama, "http://host:11434"),
            TEST_NOW,
        )
        .unwrap();

        // Same address, same runtime, different id: a duplicate.
        let error = upsert(
            &mut instances,
            instance("b", RuntimeType::Ollama, "http://host:11434/"),
            TEST_NOW,
        )
        .unwrap_err();
        assert!(error.contains("already registered"), "{error}");

        // Same address but a different runtime is legitimate — one host can run
        // both on different ports, and even the same port is the user's call.
        upsert(
            &mut instances,
            instance("c", RuntimeType::Vllm, "http://host:11434"),
            TEST_NOW,
        )
        .unwrap();
        assert_eq!(instances.len(), 2);

        // Editing an existing instance in place is not a duplicate of itself.
        upsert(
            &mut instances,
            instance("a", RuntimeType::Ollama, "http://host:11434"),
            TEST_NOW,
        )
        .unwrap();
        assert_eq!(instances.len(), 2);
    }

    #[test]
    fn upsert_preserves_created_at_and_advances_updated_at() {
        let mut instances = Vec::new();
        upsert(
            &mut instances,
            instance("a", RuntimeType::Ollama, "http://host:11434"),
            TEST_NOW,
        )
        .unwrap();
        assert_eq!(instances[0].created_at, TEST_NOW);

        upsert(
            &mut instances,
            instance("a", RuntimeType::Ollama, "http://host:11435"),
            TEST_NOW + 60,
        )
        .unwrap();
        assert_eq!(instances[0].created_at, TEST_NOW, "creation time is kept");
        assert_eq!(instances[0].updated_at, TEST_NOW + 60);
    }

    #[test]
    fn out_of_range_intervals_are_clamped_rather_than_rejected() {
        let mut wild = instance("a", RuntimeType::Vllm, "http://host:8000");
        wild.poll_interval_secs = 0;
        wild.request_timeout_ms = 10;
        let sane = sanitize_instance(wild).unwrap();
        // Zero is an omitted field, so it becomes the default, not the minimum.
        assert_eq!(sane.poll_interval_secs, DEFAULT_POLL_INTERVAL_SECS);
        assert_eq!(sane.request_timeout_ms, MIN_REQUEST_TIMEOUT_MS);

        let mut tiny = instance("c", RuntimeType::Vllm, "http://host:8001");
        tiny.poll_interval_secs = MIN_POLL_INTERVAL_SECS;
        assert_eq!(
            sanitize_instance(tiny).unwrap().poll_interval_secs,
            MIN_POLL_INTERVAL_SECS
        );

        let mut huge = instance("b", RuntimeType::Vllm, "http://host:8000");
        huge.poll_interval_secs = 99_999;
        huge.request_timeout_ms = 99_999_999;
        let sane = sanitize_instance(huge).unwrap();
        assert_eq!(sane.poll_interval_secs, MAX_POLL_INTERVAL_SECS);
        assert_eq!(sane.request_timeout_ms, MAX_REQUEST_TIMEOUT_MS);
    }

    #[test]
    fn instances_round_trip_through_a_file() {
        let root = std::env::temp_dir().join(format!("gputerm-llm-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("llm_instances.json");

        // A missing file is an empty list, not an error.
        assert!(read_instances_at(&path).unwrap().is_empty());

        let mut instances = Vec::new();
        upsert(
            &mut instances,
            instance("a", RuntimeType::Ollama, "http://host:11434"),
            TEST_NOW,
        )
        .unwrap();
        write_instances_at(&path, &instances).unwrap();

        let loaded = read_instances_at(&path).unwrap();
        assert_eq!(loaded, instances);
        // The secret must never be in this file.
        let raw = fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("apiKey"), "{raw}");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_endpoint_is_split_out_for_the_far_side_of_a_tunnel() {
        assert_eq!(
            base_url_endpoint("http://127.0.0.1:11434").unwrap(),
            ("127.0.0.1".to_string(), 11434)
        );
        // direct-tcpip needs a port, so an omitted one follows the scheme.
        assert_eq!(
            base_url_endpoint("http://ollama.internal").unwrap(),
            ("ollama.internal".to_string(), 80)
        );
        assert_eq!(
            base_url_endpoint("https://vllm.internal").unwrap(),
            ("vllm.internal".to_string(), 443)
        );
        // The channel wants a bare address, not brackets.
        assert_eq!(
            base_url_endpoint("http://[::1]:11434").unwrap(),
            ("::1".to_string(), 11434)
        );
        assert_eq!(
            base_url_endpoint("http://[::1]").unwrap(),
            ("::1".to_string(), 80)
        );
        assert!(base_url_endpoint("not-a-url").is_err());
    }

    #[test]
    fn the_same_address_on_two_different_hosts_is_not_a_duplicate() {
        let mut instances = Vec::new();
        let mut first = instance("a", RuntimeType::Ollama, "http://127.0.0.1:11434");
        first.ssh_profile_id = Some("wsl".to_string());
        upsert(&mut instances, first, TEST_NOW).unwrap();

        // A different host that also runs Ollama on its own loopback.
        let mut second = instance("b", RuntimeType::Ollama, "http://127.0.0.1:11434");
        second.ssh_profile_id = Some("gpu-box".to_string());
        upsert(&mut instances, second, TEST_NOW).unwrap();
        assert_eq!(instances.len(), 2);

        // The same host twice still is a duplicate, and the message says so.
        let mut same = instance("c", RuntimeType::Ollama, "http://127.0.0.1:11434");
        same.ssh_profile_id = Some("wsl".to_string());
        let error = upsert(&mut instances, same, TEST_NOW).unwrap_err();
        assert!(error.contains("through the same SSH session"), "{error}");

        // And a directly polled instance at that address is distinct again.
        upsert(
            &mut instances,
            instance("d", RuntimeType::Ollama, "http://127.0.0.1:11434"),
            TEST_NOW,
        )
        .unwrap();
        assert_eq!(instances.len(), 3);

        // Two direct instances still collide, with the plain message.
        let error = upsert(
            &mut instances,
            instance("e", RuntimeType::Ollama, "http://127.0.0.1:11434"),
            TEST_NOW,
        )
        .unwrap_err();
        assert!(!error.contains("SSH session"), "{error}");
    }

    #[test]
    fn https_cannot_be_tunneled_but_is_still_fine_on_its_own() {
        let mut tunneled = instance("a", RuntimeType::Vllm, "https://vllm.internal:8000");
        tunneled.ssh_profile_id = Some("gpu-box".to_string());
        let error = sanitize_instance(tunneled).unwrap_err();
        // The certificate would be checked against 127.0.0.1 and could not match.
        assert!(error.contains("checked against 127.0.0.1"), "{error}");

        let direct = instance("b", RuntimeType::Vllm, "https://vllm.internal:8000");
        assert_eq!(
            sanitize_instance(direct).unwrap().base_url,
            "https://vllm.internal:8000"
        );
    }

    #[test]
    fn a_blank_profile_id_means_no_tunnel() {
        let mut blank = instance("a", RuntimeType::Ollama, "http://host:11434");
        blank.ssh_profile_id = Some("   ".to_string());
        assert_eq!(sanitize_instance(blank).unwrap().ssh_profile_id, None);

        let mut padded = instance("b", RuntimeType::Ollama, "http://host:11434");
        padded.ssh_profile_id = Some("  wsl  ".to_string());
        assert_eq!(
            sanitize_instance(padded).unwrap().ssh_profile_id,
            Some("wsl".to_string())
        );
    }

    #[test]
    fn a_file_written_before_tunnels_existed_still_loads() {
        let root = std::env::temp_dir().join(format!("gputerm-llm-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("llm_instances.json");
        // Exactly what 1.2.3-beta wrote: no sshProfileId anywhere.
        fs::write(
            &path,
            r#"[{"id":"a","name":"old","runtimeType":"ollama","baseUrl":"http://host:11434",
                 "enabled":true,"requestTimeoutMs":3000,"pollIntervalSecs":5,
                 "createdAt":1,"updatedAt":1}]"#,
        )
        .unwrap();

        let loaded = read_instances_at(&path).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].ssh_profile_id, None);

        // Round-tripping it must not introduce the key, so a file with no
        // tunneled instance stays byte-identical to what older builds write.
        write_instances_at(&path, &loaded).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("sshProfileId"), "{raw}");

        // A tunneled instance does carry it.
        let mut tunneled = loaded[0].clone();
        tunneled.ssh_profile_id = Some("wsl".to_string());
        write_instances_at(&path, &[tunneled]).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("sshProfileId"), "{raw}");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn vault_keys_cannot_collide_with_ssh_profile_ids() {
        assert_eq!(vault_key("abc"), "llm:abc");
        assert!(vault_key("abc").starts_with(VAULT_KEY_PREFIX));
    }
}
