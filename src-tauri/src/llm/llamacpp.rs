//! llama.cpp `llama-server` adapter: `/health`, `/v1/models`, and `/metrics`
//! (`llamacpp:*`, only when the server runs with `--metrics`).

use super::adapter::{
    LlmRuntimeAdapter, RuntimeError, RuntimeErrorCode, RuntimeMetrics, RuntimeModel, RuntimeStatus,
    RuntimeType,
};
use super::http::{error_for_status, HttpClient};
use super::monitor::CounterState;
use super::prometheus::{self, Scrape};
use super::serving::{counter_rate, get_body, mean_ratio, parse_openai_models, OPENAI_MODELS_PATH};

pub const HEALTH_PATH: &str = "/health";
pub const METRICS_PATH: &str = "/metrics";

const REQUESTS_PROCESSING: &str = "llamacpp:requests_processing";
const REQUESTS_DEFERRED: &str = "llamacpp:requests_deferred";
const KV_CACHE_USAGE: &str = "llamacpp:kv_cache_usage_ratio";
const PROMPT_TOKENS: &str = "llamacpp:prompt_tokens_total";
const PREDICTED_TOKENS: &str = "llamacpp:tokens_predicted_total";
/// Shown when `/metrics` is switched off, so the user knows how to enable it.
pub const METRICS_DISABLED: &str = "/metrics (start llama-server with --metrics)";

pub struct LlamaCppAdapter {
    instance_id: String,
    now: u64,
}

impl LlamaCppAdapter {
    pub fn new(instance_id: &str, now: u64) -> Self {
        Self {
            instance_id: instance_id.to_string(),
            now,
        }
    }
}

pub fn metrics_from_scrape(scrape: &Scrape, state: &mut CounterState, now: u64) -> RuntimeMetrics {
    let mut metrics = RuntimeMetrics {
        collected_at: now,
        ..Default::default()
    };
    let mut unsupported = Vec::new();

    metrics.requests_running = scrape.sum(REQUESTS_PROCESSING);
    metrics.requests_waiting = scrape.sum(REQUESTS_DEFERRED);
    if metrics.requests_running.is_none() {
        unsupported.push(REQUESTS_PROCESSING.to_string());
    }

    // Newer builds dropped this gauge.
    metrics.kv_cache_usage_ratio = mean_ratio(scrape, KV_CACHE_USAGE);
    metrics.kv_cache_remaining_ratio = metrics
        .kv_cache_usage_ratio
        .map(|usage| (1.0 - usage).max(0.0));
    if metrics.kv_cache_usage_ratio.is_none() {
        unsupported.push(KV_CACHE_USAGE.to_string());
    }

    metrics.prompt_tokens_per_second =
        counter_rate(state, PROMPT_TOKENS, scrape.sum(PROMPT_TOKENS), now);
    metrics.generation_tokens_per_second =
        counter_rate(state, PREDICTED_TOKENS, scrape.sum(PREDICTED_TOKENS), now);
    unsupported.push("time_to_first_token".to_string());

    metrics.unsupported = unsupported;
    metrics
}

impl LlmRuntimeAdapter for LlamaCppAdapter {
    fn check_health(&self, client: &dyn HttpClient) -> RuntimeStatus {
        match client.get(HEALTH_PATH) {
            Ok(response) if response.is_success() => RuntimeStatus::online(
                &self.instance_id,
                RuntimeType::LlamaCpp,
                response.elapsed_ms,
                self.now,
            ),
            // 503 while the model is still loading: up, but not serving yet.
            Ok(response) if response.status == 503 && response.body.contains("oading") => {
                RuntimeStatus::degraded(
                    &self.instance_id,
                    RuntimeType::LlamaCpp,
                    response.elapsed_ms,
                    self.now,
                    &RuntimeError::new(
                        RuntimeErrorCode::EngineDead,
                        "llama-server is still loading the model.",
                    ),
                )
            }
            Ok(response) => RuntimeStatus::failed(
                &self.instance_id,
                RuntimeType::LlamaCpp,
                Some(response.elapsed_ms),
                self.now,
                &error_for_status(response.status),
            ),
            Err(error) => RuntimeStatus::failed(
                &self.instance_id,
                RuntimeType::LlamaCpp,
                None,
                self.now,
                &error,
            ),
        }
    }

    fn get_models(&self, client: &dyn HttpClient) -> Result<Vec<RuntimeModel>, RuntimeError> {
        parse_openai_models(&get_body(client, OPENAI_MODELS_PATH)?, "llama.cpp")
    }

    fn get_runtime_metrics(
        &self,
        client: &dyn HttpClient,
        state: &mut CounterState,
        now: u64,
    ) -> Result<Option<RuntimeMetrics>, RuntimeError> {
        let response = client.get(METRICS_PATH)?;
        // 501 (or 404 on older builds): metrics are switched off. That is a
        // configuration choice, not a fault, so it must not degrade the status.
        if matches!(response.status, 404 | 501) {
            return Ok(Some(RuntimeMetrics {
                collected_at: now,
                unsupported: vec![METRICS_DISABLED.to_string()],
                ..Default::default()
            }));
        }
        if !response.is_success() {
            return Err(error_for_status(response.status));
        }
        Ok(Some(metrics_from_scrape(
            &prometheus::parse(&response.body),
            state,
            now,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::http::testing::FakeHttpClient;

    const NOW: u64 = 1_799_000_000;

    const METRICS_BODY: &str = r#"
# TYPE llamacpp:requests_processing gauge
llamacpp:requests_processing 1
# TYPE llamacpp:requests_deferred gauge
llamacpp:requests_deferred 0
# TYPE llamacpp:kv_cache_usage_ratio gauge
llamacpp:kv_cache_usage_ratio 0.25
# TYPE llamacpp:prompt_tokens_total counter
llamacpp:prompt_tokens_total 300
# TYPE llamacpp:tokens_predicted_total counter
llamacpp:tokens_predicted_total 900
"#;

    fn adapter() -> LlamaCppAdapter {
        LlamaCppAdapter::new("l", NOW)
    }

    #[test]
    fn loading_is_degraded_not_dead() {
        let loading = FakeHttpClient::new().with_body(
            HEALTH_PATH,
            503,
            r#"{"error":{"code":503,"message":"Loading model","type":"unavailable_error"}}"#,
        );
        assert_eq!(adapter().check_health(&loading).status, "degraded");

        let dead = FakeHttpClient::new().with_body(HEALTH_PATH, 503, "");
        assert_eq!(adapter().check_health(&dead).status, "error");

        let ok = FakeHttpClient::new().with_body(HEALTH_PATH, 200, r#"{"status":"ok"}"#);
        assert_eq!(adapter().check_health(&ok).status, "online");
    }

    #[test]
    fn disabled_metrics_are_explained_not_an_error() {
        let client = FakeHttpClient::new().with_body(METRICS_PATH, 501, "");
        let metrics = adapter()
            .get_runtime_metrics(&client, &mut CounterState::default(), NOW)
            .unwrap()
            .unwrap();
        assert_eq!(metrics.unsupported, vec![METRICS_DISABLED.to_string()]);
        assert_eq!(metrics.requests_running, None);
    }

    #[test]
    fn reads_slots_kv_and_token_rates() {
        let mut state = CounterState::default();
        let metrics = metrics_from_scrape(&prometheus::parse(METRICS_BODY), &mut state, NOW);
        assert_eq!(metrics.requests_running, Some(1.0));
        assert_eq!(metrics.requests_waiting, Some(0.0));
        assert_eq!(metrics.kv_cache_usage_ratio, Some(0.25));

        let later =
            METRICS_BODY.replace("tokens_predicted_total 900", "tokens_predicted_total 1100");
        let metrics = metrics_from_scrape(&prometheus::parse(&later), &mut state, NOW + 4);
        assert_eq!(metrics.generation_tokens_per_second, Some(50.0));
    }
}
