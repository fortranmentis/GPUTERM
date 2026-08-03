//! Runtime-agnostic types shared by the Ollama and vLLM adapters.
//!
//! A value a runtime does not expose is `None`, never `0`. The UI relies on that
//! distinction to show `-`, "unknown", or "not supported" instead of inventing a
//! reading.

use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt;

/// The runtimes this module can monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeType {
    Ollama,
    Vllm,
}

impl RuntimeType {
    pub fn key(self) -> &'static str {
        match self {
            Self::Ollama => "ollama",
            Self::Vllm => "vllm",
        }
    }
}

impl fmt::Display for RuntimeType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.key())
    }
}

/// Every failure mode a probe can report, kept as a closed set so the UI can
/// explain what happened without parsing message text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeErrorCode {
    Timeout,
    ConnectionRefused,
    DnsError,
    AuthenticationError,
    HttpClientError,
    HttpServerError,
    InvalidResponse,
    ParseError,
    EngineDead,
    /// The SSH hop carrying a tunneled poll could not be established or died.
    SshTunnelError,
    /// The SSH host key is not on file, so a background connect cannot proceed
    /// without the interactive prompt it has no way to show.
    SshHostUntrusted,
    UnknownError,
}

impl RuntimeErrorCode {
    pub fn key(&self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::ConnectionRefused => "connection_refused",
            Self::DnsError => "dns_error",
            Self::AuthenticationError => "authentication_error",
            Self::HttpClientError => "http_client_error",
            Self::HttpServerError => "http_server_error",
            Self::InvalidResponse => "invalid_response",
            Self::ParseError => "parse_error",
            Self::EngineDead => "engine_dead",
            Self::SshTunnelError => "ssh_tunnel_error",
            Self::SshHostUntrusted => "ssh_host_untrusted",
            Self::UnknownError => "unknown_error",
        }
    }

    /// Whether the runtime answered at all. Drives `offline` versus `error`.
    ///
    /// `UnknownError` is only produced by the transport layer, where nothing
    /// arrived to interpret, so it belongs on the unreachable side. The two SSH
    /// codes are there for the same reason: the poll never left this machine.
    pub fn is_unreachable(&self) -> bool {
        matches!(
            self,
            Self::Timeout
                | Self::ConnectionRefused
                | Self::DnsError
                | Self::SshTunnelError
                | Self::SshHostUntrusted
                | Self::UnknownError
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeError {
    pub code: RuntimeErrorCode,
    /// Safe to show a user: never a stack trace, never a credential.
    pub message: String,
}

impl RuntimeError {
    pub fn new(code: RuntimeErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

/// Reachability of one instance at one moment.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub instance_id: String,
    pub runtime_type: String,
    /// `online` | `degraded` | `offline` | `error`
    pub status: String,
    pub response_time_ms: Option<u64>,
    pub checked_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

impl RuntimeStatus {
    pub fn online(
        instance_id: &str,
        runtime_type: RuntimeType,
        response_time_ms: u64,
        checked_at: u64,
    ) -> Self {
        Self {
            instance_id: instance_id.to_string(),
            runtime_type: runtime_type.key().to_string(),
            status: "online".to_string(),
            response_time_ms: Some(response_time_ms),
            checked_at,
            error_code: None,
            error_message: None,
        }
    }

    /// The runtime answered, but some of what it said could not be read.
    pub fn degraded(
        instance_id: &str,
        runtime_type: RuntimeType,
        response_time_ms: u64,
        checked_at: u64,
        error: &RuntimeError,
    ) -> Self {
        Self {
            instance_id: instance_id.to_string(),
            runtime_type: runtime_type.key().to_string(),
            status: "degraded".to_string(),
            response_time_ms: Some(response_time_ms),
            checked_at,
            error_code: Some(error.code.key().to_string()),
            error_message: Some(error.message.clone()),
        }
    }

    /// `offline` when the host never answered, `error` when it answered badly.
    pub fn failed(
        instance_id: &str,
        runtime_type: RuntimeType,
        response_time_ms: Option<u64>,
        checked_at: u64,
        error: &RuntimeError,
    ) -> Self {
        Self {
            instance_id: instance_id.to_string(),
            runtime_type: runtime_type.key().to_string(),
            status: if error.code.is_unreachable() {
                "offline"
            } else {
                "error"
            }
            .to_string(),
            response_time_ms,
            checked_at,
            error_code: Some(error.code.key().to_string()),
            error_message: Some(error.message.clone()),
        }
    }
}

/// One model, whether it is loaded or merely installed.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeModel {
    pub id: String,
    pub name: String,
    /// `running` | `installed` | `served`
    pub status: String,
    pub parameter_size: Option<String>,
    pub quantization: Option<String>,
    pub model_size_bytes: Option<u64>,
    pub vram_size_bytes: Option<u64>,
    /// `vram_size_bytes / model_size_bytes`, 0-100. `None` when either is absent
    /// or the size is zero.
    pub vram_resident_percent: Option<f64>,
    /// `model_size_bytes - vram_size_bytes`. An estimate of what is not resident
    /// in VRAM — deliberately not called system RAM usage, because it is not a
    /// measurement of RAM.
    pub non_vram_bytes: Option<u64>,
    /// The configured maximum context, not the context a conversation is using.
    pub context_length: Option<u64>,
    pub expires_at: Option<u64>,
    /// Seconds until the runtime may unload the model. `None` when unknown,
    /// `Some(0)` when the deadline has already passed.
    pub expires_in_seconds: Option<u64>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
}

impl RuntimeModel {
    pub fn new(id: impl Into<String>, name: impl Into<String>, status: &str) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            status: status.to_string(),
            parameter_size: None,
            quantization: None,
            model_size_bytes: None,
            vram_size_bytes: None,
            vram_resident_percent: None,
            non_vram_bytes: None,
            context_length: None,
            expires_at: None,
            expires_in_seconds: None,
            metadata: BTreeMap::new(),
        }
    }
}

/// Serving metrics. Ollama fills almost none of these; the fields exist so both
/// runtimes share one shape and the UI can mark the rest unsupported.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeMetrics {
    pub requests_running: Option<f64>,
    pub requests_waiting: Option<f64>,
    pub requests_swapped: Option<f64>,
    pub kv_cache_usage_ratio: Option<f64>,
    pub kv_cache_remaining_ratio: Option<f64>,
    pub prefix_cache_hit_ratio: Option<f64>,
    pub prompt_tokens_per_second: Option<f64>,
    pub generation_tokens_per_second: Option<f64>,
    pub requests_per_second: Option<f64>,
    pub preemptions_total: Option<f64>,
    pub preemptions_delta: Option<f64>,
    pub ttft_p50_seconds: Option<f64>,
    pub ttft_p95_seconds: Option<f64>,
    pub e2e_latency_p95_seconds: Option<f64>,
    pub queue_time_p95_seconds: Option<f64>,
    pub collected_at: u64,
    /// Metric names this build looks for but this server does not expose, so the
    /// UI can say "not supported" rather than showing a blank.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unsupported: Vec<String>,
}

/// What every runtime adapter must provide.
///
/// Implementations must not send inference requests: monitoring is read-only and
/// must not perturb the server it is watching.
pub trait LlmRuntimeAdapter {
    /// Reachability plus response time. Never fails: an unreachable host is a
    /// status, not an error.
    fn check_health(&self, client: &dyn crate::llm::http::HttpClient) -> RuntimeStatus;

    /// The full listing, including anything installed but not loaded.
    fn get_models(
        &self,
        client: &dyn crate::llm::http::HttpClient,
    ) -> Result<Vec<RuntimeModel>, RuntimeError>;

    /// The subset that changes minute to minute, when the runtime has a cheap
    /// endpoint for it.
    ///
    /// `None` means there is no such endpoint, so the caller should keep the
    /// last full listing instead of re-fetching it on every poll. Returning the
    /// full list here would make the catalog interval meaningless.
    fn get_live_models(
        &self,
        _client: &dyn crate::llm::http::HttpClient,
    ) -> Option<Result<Vec<RuntimeModel>, RuntimeError>> {
        None
    }

    /// `Ok(None)` when the runtime exposes no serving metrics at all.
    fn get_runtime_metrics(
        &self,
        client: &dyn crate::llm::http::HttpClient,
        state: &mut super::monitor::CounterState,
        now: u64,
    ) -> Result<Option<RuntimeMetrics>, RuntimeError>;
}

/// Percentage of a model resident in VRAM, guarding division by zero.
pub fn vram_resident_percent(size_bytes: Option<u64>, vram_bytes: Option<u64>) -> Option<f64> {
    match (size_bytes, vram_bytes) {
        (Some(size), Some(vram)) if size > 0 => {
            Some((vram as f64 / size as f64 * 100.0).clamp(0.0, 100.0))
        }
        _ => None,
    }
}

/// Bytes of a model that are not resident in VRAM. An estimate of CPU
/// offloading, not a measurement of system RAM.
pub fn non_vram_bytes(size_bytes: Option<u64>, vram_bytes: Option<u64>) -> Option<u64> {
    match (size_bytes, vram_bytes) {
        (Some(size), Some(vram)) => Some(size.saturating_sub(vram)),
        _ => None,
    }
}

/// Seconds left before a deadline. `Some(0)` once it has passed, `None` when
/// there is no deadline.
pub fn seconds_until(expires_at: Option<u64>, now: u64) -> Option<u64> {
    expires_at.map(|deadline| deadline.saturating_sub(now))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vram_ratio_guards_a_zero_sized_model() {
        assert_eq!(vram_resident_percent(Some(1000), Some(250)), Some(25.0));
        // A zero size would otherwise divide by zero and produce NaN or inf.
        assert_eq!(vram_resident_percent(Some(0), Some(250)), None);
        assert_eq!(vram_resident_percent(None, Some(250)), None);
        assert_eq!(vram_resident_percent(Some(1000), None), None);
        // More VRAM than size is nonsense from the server; clamp rather than
        // report over 100%.
        assert_eq!(vram_resident_percent(Some(100), Some(400)), Some(100.0));
    }

    #[test]
    fn non_vram_estimate_never_goes_negative() {
        assert_eq!(non_vram_bytes(Some(1000), Some(250)), Some(750));
        assert_eq!(non_vram_bytes(Some(100), Some(400)), Some(0));
        assert_eq!(non_vram_bytes(None, Some(250)), None);
    }

    #[test]
    fn expiry_countdown_saturates_at_zero() {
        assert_eq!(seconds_until(Some(1_500), 1_000), Some(500));
        // Already expired: report zero rather than wrapping around.
        assert_eq!(seconds_until(Some(500), 1_000), Some(0));
        assert_eq!(seconds_until(None, 1_000), None);
    }

    #[test]
    fn unreachable_codes_map_to_offline_and_the_rest_to_error() {
        let checked_at = 1_700_000_000;
        for code in [
            RuntimeErrorCode::Timeout,
            RuntimeErrorCode::ConnectionRefused,
            RuntimeErrorCode::DnsError,
        ] {
            let error = RuntimeError::new(code, "nope");
            let status = RuntimeStatus::failed("i", RuntimeType::Vllm, None, checked_at, &error);
            assert_eq!(status.status, "offline");
        }
        // A tunnel that could not be built means nothing arrived to interpret,
        // so these belong on the unreachable side too.
        for code in [
            RuntimeErrorCode::SshTunnelError,
            RuntimeErrorCode::SshHostUntrusted,
        ] {
            let error = RuntimeError::new(code.clone(), "nope");
            let status = RuntimeStatus::failed("i", RuntimeType::Vllm, None, checked_at, &error);
            assert_eq!(status.status, "offline", "{:?}", code);
            assert_eq!(status.error_code.as_deref(), Some(code.key()));
        }
        assert_eq!(RuntimeErrorCode::SshTunnelError.key(), "ssh_tunnel_error");
        assert_eq!(
            RuntimeErrorCode::SshHostUntrusted.key(),
            "ssh_host_untrusted"
        );

        for code in [
            RuntimeErrorCode::EngineDead,
            RuntimeErrorCode::AuthenticationError,
            RuntimeErrorCode::ParseError,
        ] {
            let error = RuntimeError::new(code.clone(), "nope");
            let status = RuntimeStatus::failed("i", RuntimeType::Vllm, None, checked_at, &error);
            assert_eq!(status.status, "error", "{:?} should be an error", code);
            assert_eq!(status.error_code.as_deref(), Some(code.key()));
        }
    }
}
