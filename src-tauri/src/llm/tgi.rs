//! Hugging Face Text Generation Inference adapter: `/health`, `/info` (one
//! model per server), and `/metrics` (`tgi_*`).
//!
//! TGI publishes no KV cache or time-to-first-token metric, so those are
//! reported unsupported rather than zero.

use serde::Deserialize;
use std::collections::BTreeMap;

use super::adapter::{
    LlmRuntimeAdapter, RuntimeError, RuntimeErrorCode, RuntimeMetrics, RuntimeModel, RuntimeStatus,
    RuntimeType,
};
use super::http::HttpClient;
use super::monitor::CounterState;
use super::prometheus::{self, Scrape};
use super::serving::{
    counter_rate, get_body, has_histogram, health_from_get, sum_first, worst_quantile,
};

pub const HEALTH_PATH: &str = "/health";
pub const INFO_PATH: &str = "/info";
pub const METRICS_PATH: &str = "/metrics";

const QUEUE_SIZE: &str = "tgi_queue_size";
const BATCH_SIZE: &str = "tgi_batch_current_size";
const REQUEST_SUCCESS: [&str; 2] = ["tgi_request_success", "tgi_request_success_total"];
/// Histogram sums: total input and generated tokens over all requests.
const INPUT_TOKENS_SUM: &str = "tgi_request_input_length_sum";
const GENERATED_TOKENS_SUM: &str = "tgi_request_generated_tokens_sum";
const DURATION_HISTOGRAM: &str = "tgi_request_duration";
const QUEUE_HISTOGRAM: &str = "tgi_request_queue_duration";

pub struct TgiAdapter {
    instance_id: String,
    now: u64,
}

impl TgiAdapter {
    pub fn new(instance_id: &str, now: u64) -> Self {
        Self {
            instance_id: instance_id.to_string(),
            now,
        }
    }
}

#[derive(Debug, Deserialize)]
struct TgiInfo {
    model_id: String,
    #[serde(default)]
    model_dtype: Option<String>,
    #[serde(default)]
    max_total_tokens: Option<u64>,
    #[serde(default)]
    version: Option<String>,
}

fn parse_info(body: &str) -> Result<Vec<RuntimeModel>, RuntimeError> {
    let info: TgiInfo = serde_json::from_str(body).map_err(|error| {
        RuntimeError::new(
            RuntimeErrorCode::ParseError,
            format!("Could not read the TGI server info: {}", error),
        )
    })?;
    let mut model = RuntimeModel::new(info.model_id.clone(), info.model_id, "served");
    model.context_length = info.max_total_tokens;
    model.quantization = info.model_dtype;
    let mut metadata = BTreeMap::new();
    if let Some(version) = info.version {
        metadata.insert("version".to_string(), version);
    }
    model.metadata = metadata;
    Ok(vec![model])
}

pub fn metrics_from_scrape(scrape: &Scrape, state: &mut CounterState, now: u64) -> RuntimeMetrics {
    let mut metrics = RuntimeMetrics {
        collected_at: now,
        ..Default::default()
    };
    let mut unsupported = vec![
        "kv_cache_usage".to_string(),
        "time_to_first_token".to_string(),
    ];

    metrics.requests_running = scrape.sum(BATCH_SIZE);
    metrics.requests_waiting = scrape.sum(QUEUE_SIZE);
    if metrics.requests_waiting.is_none() {
        unsupported.push(QUEUE_SIZE.to_string());
    }

    metrics.prompt_tokens_per_second =
        counter_rate(state, INPUT_TOKENS_SUM, scrape.sum(INPUT_TOKENS_SUM), now);
    metrics.generation_tokens_per_second = counter_rate(
        state,
        GENERATED_TOKENS_SUM,
        scrape.sum(GENERATED_TOKENS_SUM),
        now,
    );
    metrics.requests_per_second = counter_rate(
        state,
        REQUEST_SUCCESS[0],
        sum_first(scrape, &REQUEST_SUCCESS),
        now,
    );

    metrics.e2e_latency_p95_seconds = worst_quantile(scrape, DURATION_HISTOGRAM, 0.95);
    metrics.queue_time_p95_seconds = worst_quantile(scrape, QUEUE_HISTOGRAM, 0.95);
    if !has_histogram(scrape, DURATION_HISTOGRAM) {
        unsupported.push(DURATION_HISTOGRAM.to_string());
    }

    metrics.unsupported = unsupported;
    metrics
}

impl LlmRuntimeAdapter for TgiAdapter {
    fn check_health(&self, client: &dyn HttpClient) -> RuntimeStatus {
        health_from_get(
            client,
            HEALTH_PATH,
            &self.instance_id,
            RuntimeType::Tgi,
            self.now,
        )
    }

    fn get_models(&self, client: &dyn HttpClient) -> Result<Vec<RuntimeModel>, RuntimeError> {
        parse_info(&get_body(client, INFO_PATH)?)
    }

    fn get_runtime_metrics(
        &self,
        client: &dyn HttpClient,
        state: &mut CounterState,
        now: u64,
    ) -> Result<Option<RuntimeMetrics>, RuntimeError> {
        let body = get_body(client, METRICS_PATH)?;
        Ok(Some(metrics_from_scrape(
            &prometheus::parse(&body),
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
# TYPE tgi_queue_size gauge
tgi_queue_size 3
# TYPE tgi_batch_current_size gauge
tgi_batch_current_size 8
# TYPE tgi_request_success counter
tgi_request_success 120
# TYPE tgi_request_generated_tokens histogram
tgi_request_generated_tokens_bucket{le="+Inf"} 120
tgi_request_generated_tokens_sum 6000
tgi_request_generated_tokens_count 120
# TYPE tgi_request_duration histogram
tgi_request_duration_bucket{le="1"} 60
tgi_request_duration_bucket{le="5"} 118
tgi_request_duration_bucket{le="+Inf"} 120
tgi_request_duration_sum 200
tgi_request_duration_count 120
"#;

    #[test]
    fn reads_queue_batch_and_latency() {
        let mut state = CounterState::default();
        let metrics = metrics_from_scrape(&prometheus::parse(METRICS_BODY), &mut state, NOW);
        assert_eq!(metrics.requests_waiting, Some(3.0));
        assert_eq!(metrics.requests_running, Some(8.0));
        assert!(metrics.e2e_latency_p95_seconds.is_some());
        assert_eq!(metrics.kv_cache_usage_ratio, None);
        assert!(metrics.unsupported.contains(&"kv_cache_usage".to_string()));
    }

    #[test]
    fn generated_token_rate_comes_from_the_histogram_sum() {
        let mut state = CounterState::default();
        metrics_from_scrape(&prometheus::parse(METRICS_BODY), &mut state, NOW);
        let later = METRICS_BODY
            .replace(
                "tgi_request_generated_tokens_sum 6000",
                "tgi_request_generated_tokens_sum 6500",
            )
            .replace("tgi_request_success 120", "tgi_request_success 130");
        let metrics = metrics_from_scrape(&prometheus::parse(&later), &mut state, NOW + 10);
        assert_eq!(metrics.generation_tokens_per_second, Some(50.0));
        assert_eq!(metrics.requests_per_second, Some(1.0));
    }

    #[test]
    fn info_becomes_the_single_served_model() {
        let adapter = TgiAdapter::new("t", NOW);
        let client = FakeHttpClient::new().with_body(
            INFO_PATH,
            200,
            r#"{"model_id":"meta-llama/Llama-3.1-8B","model_dtype":"torch.float16","max_total_tokens":8192,"version":"2.4.0"}"#,
        );
        let models = adapter.get_models(&client).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].name, "meta-llama/Llama-3.1-8B");
        assert_eq!(models[0].context_length, Some(8192));
        assert_eq!(models[0].quantization.as_deref(), Some("torch.float16"));

        let broken = FakeHttpClient::new().with_body(INFO_PATH, 200, "{}");
        assert_eq!(
            adapter.get_models(&broken).unwrap_err().code,
            RuntimeErrorCode::ParseError
        );
    }
}
