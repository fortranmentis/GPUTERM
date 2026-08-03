//! Ollama adapter.
//!
//! Reads only `/api/ps` and `/api/tags`. It never sends an inference request:
//! that would both perturb the server and cost the user tokens, so per-request
//! figures (TTFT, tokens/s, queue depth) are simply not available here and are
//! left unset rather than estimated.

use serde::Deserialize;
use std::cell::RefCell;
use std::collections::BTreeMap;

use super::adapter::{
    non_vram_bytes, seconds_until, vram_resident_percent, LlmRuntimeAdapter, RuntimeError,
    RuntimeErrorCode, RuntimeMetrics, RuntimeModel, RuntimeStatus, RuntimeType,
};
use super::http::{error_for_status, HttpClient};
use super::monitor::CounterState;

pub const PS_PATH: &str = "/api/ps";
pub const TAGS_PATH: &str = "/api/tags";

pub struct OllamaAdapter {
    instance_id: String,
    now: u64,
    /// `/api/ps` answers both the health probe and the running-model list, so
    /// the one response is kept for whichever call comes second in a cycle.
    /// Taking it empties the slot, which keeps it from being reused across
    /// cycles — an adapter is built fresh for each one anyway.
    loaded_models: RefCell<Option<Vec<OllamaModelEntry>>>,
}

impl OllamaAdapter {
    pub fn new(instance_id: &str, now: u64) -> Self {
        Self {
            instance_id: instance_id.to_string(),
            now,
            loaded_models: RefCell::new(None),
        }
    }

    /// The running models, from this cycle's health response when it is still
    /// available and from a fresh `/api/ps` otherwise.
    fn running_models(
        &self,
        client: &dyn HttpClient,
    ) -> Result<Vec<OllamaModelEntry>, RuntimeError> {
        if let Some(cached) = self.loaded_models.borrow_mut().take() {
            return Ok(cached);
        }
        let response = client.get(PS_PATH)?;
        if !response.is_success() {
            return Err(error_for_status(response.status));
        }
        parse_model_list(&response.body)
    }
}

#[derive(Debug, Deserialize)]
struct OllamaDetails {
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    family: Option<String>,
    #[serde(default)]
    families: Option<Vec<String>>,
    #[serde(default)]
    parameter_size: Option<String>,
    #[serde(default)]
    quantization_level: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OllamaModelEntry {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    size_vram: Option<u64>,
    #[serde(default)]
    context_length: Option<u64>,
    #[serde(default)]
    expires_at: Option<String>,
    #[serde(default)]
    modified_at: Option<String>,
    #[serde(default)]
    digest: Option<String>,
    #[serde(default)]
    details: Option<OllamaDetails>,
}

#[derive(Debug, Deserialize)]
struct OllamaModelList {
    #[serde(default)]
    models: Vec<OllamaModelEntry>,
}

/// Parses `/api/ps` or `/api/tags`. A missing `models` key yields an empty list
/// rather than an error, because an idle server legitimately returns one.
fn parse_model_list(body: &str) -> Result<Vec<OllamaModelEntry>, RuntimeError> {
    let parsed: OllamaModelList = serde_json::from_str(body).map_err(|error| {
        RuntimeError::new(
            RuntimeErrorCode::ParseError,
            format!("Could not read the Ollama response: {}", error),
        )
    })?;
    Ok(parsed.models)
}

/// Ollama reports `expires_at` as RFC 3339. Parsing failures are ignored rather
/// than failing the whole model, so one odd timestamp cannot hide a server.
fn parse_expires_at(raw: Option<&str>) -> Option<u64> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    let parsed = chrono::DateTime::parse_from_rfc3339(raw).ok()?;
    let seconds = parsed.timestamp();
    if seconds <= 0 {
        return None;
    }
    Some(seconds as u64)
}

fn to_model(entry: &OllamaModelEntry, status: &str, now: u64) -> Option<RuntimeModel> {
    // A model with neither name nor id cannot be shown or keyed; skip it rather
    // than inventing an identity.
    let name = entry
        .name
        .clone()
        .or_else(|| entry.model.clone())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())?;
    let id = entry.model.clone().unwrap_or_else(|| name.clone());

    let mut model = RuntimeModel::new(id, name, status);
    model.model_size_bytes = entry.size;
    model.vram_size_bytes = entry.size_vram;
    model.vram_resident_percent = vram_resident_percent(entry.size, entry.size_vram);
    model.non_vram_bytes = non_vram_bytes(entry.size, entry.size_vram);
    model.context_length = entry.context_length;
    model.expires_at = parse_expires_at(entry.expires_at.as_deref());
    model.expires_in_seconds = seconds_until(model.expires_at, now);

    if let Some(details) = &entry.details {
        model.parameter_size = details.parameter_size.clone();
        model.quantization = details.quantization_level.clone();
        let mut metadata = BTreeMap::new();
        if let Some(format) = &details.format {
            metadata.insert("format".to_string(), format.clone());
        }
        if let Some(family) = &details.family {
            metadata.insert("family".to_string(), family.clone());
        }
        if let Some(families) = &details.families {
            if !families.is_empty() {
                metadata.insert("families".to_string(), families.join(", "));
            }
        }
        model.metadata = metadata;
    }
    if let Some(digest) = &entry.digest {
        model
            .metadata
            .insert("digest".to_string(), digest.chars().take(12).collect());
    }
    if let Some(modified) = &entry.modified_at {
        model
            .metadata
            .insert("modifiedAt".to_string(), modified.clone());
    }
    Some(model)
}

/// Merges loaded models over installed ones. A model present in both is
/// `running` and keeps the residency figures only `/api/ps` reports.
fn merge_models(
    running: &[OllamaModelEntry],
    installed: &[OllamaModelEntry],
    now: u64,
) -> Vec<RuntimeModel> {
    let mut models: Vec<RuntimeModel> = running
        .iter()
        .filter_map(|entry| to_model(entry, "running", now))
        .collect();

    for entry in installed {
        let Some(model) = to_model(entry, "installed", now) else {
            continue;
        };
        let already_running = models
            .iter()
            .any(|existing| existing.id == model.id || existing.name == model.name);
        if !already_running {
            models.push(model);
        }
    }

    models.sort_by(|left, right| {
        // Running models first, then by name.
        let rank = |status: &str| if status == "running" { 0 } else { 1 };
        rank(&left.status)
            .cmp(&rank(&right.status))
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    models
}

impl LlmRuntimeAdapter for OllamaAdapter {
    fn check_health(&self, client: &dyn HttpClient) -> RuntimeStatus {
        // `/api/ps` doubles as the health probe: it is cheap, always present,
        // and needs no inference.
        match client.get(PS_PATH) {
            Ok(response) if response.is_success() => {
                match parse_model_list(&response.body) {
                    Ok(models) => {
                        *self.loaded_models.borrow_mut() = Some(models);
                        RuntimeStatus::online(
                            &self.instance_id,
                            RuntimeType::Ollama,
                            response.elapsed_ms,
                            self.now,
                        )
                    }
                    // It answered, but not with something we understand.
                    Err(error) => RuntimeStatus::degraded(
                        &self.instance_id,
                        RuntimeType::Ollama,
                        response.elapsed_ms,
                        self.now,
                        &error,
                    ),
                }
            }
            Ok(response) => RuntimeStatus::failed(
                &self.instance_id,
                RuntimeType::Ollama,
                Some(response.elapsed_ms),
                self.now,
                &error_for_status(response.status),
            ),
            Err(error) => RuntimeStatus::failed(
                &self.instance_id,
                RuntimeType::Ollama,
                None,
                self.now,
                &error,
            ),
        }
    }

    fn get_models(&self, client: &dyn HttpClient) -> Result<Vec<RuntimeModel>, RuntimeError> {
        let running = self.running_models(client)?;

        // An installed-model listing that fails must not hide the models that
        // are actually loaded, so this failure is swallowed.
        let installed = client
            .get(TAGS_PATH)
            .ok()
            .filter(|response| response.is_success())
            .and_then(|response| parse_model_list(&response.body).ok())
            .unwrap_or_default();

        Ok(merge_models(&running, &installed, self.now))
    }

    /// Loaded models only. `/api/ps` is cheap enough to poll every few seconds,
    /// unlike `/api/tags`, which walks the whole model store.
    fn get_live_models(
        &self,
        client: &dyn HttpClient,
    ) -> Option<Result<Vec<RuntimeModel>, RuntimeError>> {
        Some(
            self.running_models(client)
                .map(|running| merge_models(&running, &[], self.now)),
        )
    }

    fn get_runtime_metrics(
        &self,
        _client: &dyn HttpClient,
        _state: &mut CounterState,
        _now: u64,
    ) -> Result<Option<RuntimeMetrics>, RuntimeError> {
        // Ollama exposes no serving metrics on the endpoints this monitor is
        // allowed to call. Reporting `None` keeps the UI honest instead of
        // showing zeros for requests, queue depth, and throughput.
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::http::testing::FakeHttpClient;

    const TEST_NOW: u64 = 1_799_000_000;

    const PS_BODY: &str = r#"{
      "models": [
        {
          "name": "llama3:70b",
          "model": "llama3:70b",
          "size": 40000000000,
          "size_vram": 30000000000,
          "context_length": 8192,
          "expires_at": "2027-01-03T18:18:20Z",
          "digest": "abcdef0123456789",
          "details": {
            "format": "gguf",
            "family": "llama",
            "families": ["llama"],
            "parameter_size": "70B",
            "quantization_level": "Q4_0"
          }
        }
      ]
    }"#;

    const TAGS_BODY: &str = r#"{
      "models": [
        { "name": "llama3:70b", "model": "llama3:70b", "size": 40000000000 },
        {
          "name": "mistral:7b",
          "model": "mistral:7b",
          "size": 4000000000,
          "modified_at": "2026-05-01T10:00:00Z",
          "details": { "parameter_size": "7B", "quantization_level": "Q4_K_M" }
        }
      ]
    }"#;

    fn adapter() -> OllamaAdapter {
        OllamaAdapter::new("inst-1", TEST_NOW)
    }

    #[test]
    fn parses_a_running_model_with_residency_and_expiry() {
        let client = FakeHttpClient::new()
            .with_body(PS_PATH, 200, PS_BODY)
            .with_body(TAGS_PATH, 200, r#"{"models":[]}"#);
        let models = adapter().get_models(&client).unwrap();

        assert_eq!(models.len(), 1);
        let model = &models[0];
        assert_eq!(model.status, "running");
        assert_eq!(model.parameter_size.as_deref(), Some("70B"));
        assert_eq!(model.quantization.as_deref(), Some("Q4_0"));
        assert_eq!(model.model_size_bytes, Some(40_000_000_000));
        assert_eq!(model.vram_size_bytes, Some(30_000_000_000));
        assert_eq!(model.vram_resident_percent, Some(75.0));
        assert_eq!(model.non_vram_bytes, Some(10_000_000_000));
        assert_eq!(model.context_length, Some(8192));
        // Five minutes past TEST_NOW in the fixture.
        assert_eq!(model.expires_at, Some(TEST_NOW + 300));
        assert_eq!(model.expires_in_seconds, Some(300));
        assert_eq!(model.metadata.get("family").map(String::as_str), Some("llama"));
    }

    #[test]
    fn an_idle_server_reports_no_models_rather_than_failing() {
        let client = FakeHttpClient::new()
            .with_body(PS_PATH, 200, r#"{"models":[]}"#)
            .with_body(TAGS_PATH, 200, r#"{"models":[]}"#);
        assert!(adapter().get_models(&client).unwrap().is_empty());

        // Some builds omit the key entirely.
        let client = FakeHttpClient::new()
            .with_body(PS_PATH, 200, "{}")
            .with_body(TAGS_PATH, 200, "{}");
        assert!(adapter().get_models(&client).unwrap().is_empty());
        assert_eq!(adapter().check_health(&client).status, "online");
    }

    #[test]
    fn missing_fields_leave_values_unset_instead_of_zero() {
        let body = r#"{"models":[{"name":"bare:latest"}]}"#;
        let client = FakeHttpClient::new()
            .with_body(PS_PATH, 200, body)
            .with_body(TAGS_PATH, 200, r#"{"models":[]}"#);
        let models = adapter().get_models(&client).unwrap();

        let model = &models[0];
        assert_eq!(model.name, "bare:latest");
        assert_eq!(model.model_size_bytes, None);
        assert_eq!(model.vram_resident_percent, None, "not 0%");
        assert_eq!(model.non_vram_bytes, None);
        assert_eq!(model.context_length, None);
        assert_eq!(model.expires_in_seconds, None);
    }

    #[test]
    fn a_zero_sized_model_does_not_divide_by_zero() {
        let body = r#"{"models":[{"name":"weird:1b","size":0,"size_vram":0}]}"#;
        let client = FakeHttpClient::new()
            .with_body(PS_PATH, 200, body)
            .with_body(TAGS_PATH, 200, r#"{"models":[]}"#);
        let models = adapter().get_models(&client).unwrap();
        assert_eq!(models[0].vram_resident_percent, None);
        assert_eq!(models[0].non_vram_bytes, Some(0));
    }

    #[test]
    fn running_and_installed_models_merge_without_duplicates() {
        let client = FakeHttpClient::new()
            .with_body(PS_PATH, 200, PS_BODY)
            .with_body(TAGS_PATH, 200, TAGS_BODY);
        let models = adapter().get_models(&client).unwrap();

        assert_eq!(models.len(), 2, "llama3 appears once, not twice");
        assert_eq!(models[0].name, "llama3:70b");
        assert_eq!(models[0].status, "running", "running models sort first");
        assert_eq!(models[1].name, "mistral:7b");
        assert_eq!(models[1].status, "installed");
        // The residency figures come from /api/ps and survive the merge.
        assert_eq!(models[0].vram_size_bytes, Some(30_000_000_000));
    }

    #[test]
    fn an_expiry_in_the_past_reports_zero_not_a_wrapped_number() {
        let body = r#"{"models":[{"name":"old:1b","expires_at":"2020-01-01T00:00:00Z"}]}"#;
        let client = FakeHttpClient::new()
            .with_body(PS_PATH, 200, body)
            .with_body(TAGS_PATH, 200, r#"{"models":[]}"#);
        let models = adapter().get_models(&client).unwrap();
        assert_eq!(models[0].expires_in_seconds, Some(0));

        // An unparseable timestamp is dropped, not treated as epoch zero.
        let body = r#"{"models":[{"name":"odd:1b","expires_at":"not-a-date"}]}"#;
        let client = FakeHttpClient::new()
            .with_body(PS_PATH, 200, body)
            .with_body(TAGS_PATH, 200, r#"{"models":[]}"#);
        let models = adapter().get_models(&client).unwrap();
        assert_eq!(models[0].expires_at, None);
        assert_eq!(models[0].expires_in_seconds, None);
    }

    #[test]
    fn a_failing_tags_call_still_shows_the_running_models() {
        let client = FakeHttpClient::new()
            .with_body(PS_PATH, 200, PS_BODY)
            .with_body(TAGS_PATH, 500, "boom");
        let models = adapter().get_models(&client).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].status, "running");
    }

    #[test]
    fn health_distinguishes_offline_error_and_degraded() {
        let refused = FakeHttpClient::new().with_error(
            PS_PATH,
            RuntimeError::new(RuntimeErrorCode::ConnectionRefused, "refused"),
        );
        assert_eq!(adapter().check_health(&refused).status, "offline");

        let timed_out = FakeHttpClient::new()
            .with_error(PS_PATH, RuntimeError::new(RuntimeErrorCode::Timeout, "slow"));
        let status = adapter().check_health(&timed_out);
        assert_eq!(status.status, "offline");
        assert_eq!(status.error_code.as_deref(), Some("timeout"));

        let server_error = FakeHttpClient::new().with_body(PS_PATH, 500, "boom");
        let status = adapter().check_health(&server_error);
        assert_eq!(status.status, "error");
        assert_eq!(status.error_code.as_deref(), Some("http_server_error"));

        // Answered, but the body is not JSON we can read.
        let malformed = FakeHttpClient::new().with_body(PS_PATH, 200, "{not json");
        let status = adapter().check_health(&malformed);
        assert_eq!(status.status, "degraded");
        assert_eq!(status.error_code.as_deref(), Some("parse_error"));
        assert!(status.response_time_ms.is_some());
    }

    #[test]
    fn ollama_reports_no_serving_metrics_rather_than_zeros() {
        let client = FakeHttpClient::new().with_body(PS_PATH, 200, PS_BODY);
        let mut state = CounterState::default();
        let metrics = adapter()
            .get_runtime_metrics(&client, &mut state, TEST_NOW)
            .unwrap();
        assert!(
            metrics.is_none(),
            "requests and throughput are unknowable from /api/ps"
        );
    }

    #[test]
    fn monitoring_never_calls_a_generation_endpoint() {
        let client = FakeHttpClient::new()
            .with_body(PS_PATH, 200, PS_BODY)
            .with_body(TAGS_PATH, 200, TAGS_BODY);
        let adapter = adapter();
        let _ = adapter.check_health(&client);
        let _ = adapter.get_models(&client);
        let requested = client.requested.lock().unwrap();
        assert!(
            requested.iter().all(|path| path == PS_PATH || path == TAGS_PATH),
            "unexpected endpoint touched: {requested:?}"
        );
    }
}
