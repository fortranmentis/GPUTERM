//! SGLang adapter: `/health`, `/v1/models`, and `/metrics` (`sglang:*`, served
//! when the server runs with `--enable-metrics`).

use super::adapter::{
    LlmRuntimeAdapter, RuntimeError, RuntimeMetrics, RuntimeModel, RuntimeStatus, RuntimeType,
};
use super::http::HttpClient;
use super::monitor::CounterState;
use super::prometheus::{self, Scrape};
use super::serving::{
    counter_rate, get_body, has_histogram, health_from_get, mean_ratio, parse_openai_models,
    sum_first, worst_quantile, OPENAI_MODELS_PATH,
};

pub const HEALTH_PATH: &str = "/health";
pub const METRICS_PATH: &str = "/metrics";

const REQUESTS_RUNNING: &str = "sglang:num_running_reqs";
const REQUESTS_WAITING: &str = "sglang:num_queue_reqs";
/// Fraction of the KV token pool in use.
const TOKEN_USAGE: &str = "sglang:token_usage";
const CACHE_HIT_RATE: &str = "sglang:cache_hit_rate";
const PROMPT_TOKENS: [&str; 2] = ["sglang:prompt_tokens_total", "sglang:prompt_tokens"];
const GENERATION_TOKENS: [&str; 2] = ["sglang:generation_tokens_total", "sglang:generation_tokens"];
const REQUESTS_TOTAL: [&str; 2] = ["sglang:num_requests_total", "sglang:num_requests"];
const TTFT_HISTOGRAM: &str = "sglang:time_to_first_token_seconds";
const E2E_HISTOGRAM: &str = "sglang:e2e_request_latency_seconds";
const QUEUE_HISTOGRAM: &str = "sglang:queue_time_seconds";

pub struct SglangAdapter {
    instance_id: String,
    now: u64,
}

impl SglangAdapter {
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

    metrics.requests_running = scrape.sum(REQUESTS_RUNNING);
    metrics.requests_waiting = scrape.sum(REQUESTS_WAITING);
    if metrics.requests_running.is_none() {
        unsupported.push(REQUESTS_RUNNING.to_string());
    }

    metrics.kv_cache_usage_ratio = mean_ratio(scrape, TOKEN_USAGE);
    metrics.kv_cache_remaining_ratio = metrics
        .kv_cache_usage_ratio
        .map(|usage| (1.0 - usage).max(0.0));
    if metrics.kv_cache_usage_ratio.is_none() {
        unsupported.push(TOKEN_USAGE.to_string());
    }

    metrics.prefix_cache_hit_ratio = mean_ratio(scrape, CACHE_HIT_RATE);
    if metrics.prefix_cache_hit_ratio.is_none() {
        unsupported.push(CACHE_HIT_RATE.to_string());
    }

    metrics.prompt_tokens_per_second = counter_rate(
        state,
        PROMPT_TOKENS[0],
        sum_first(scrape, &PROMPT_TOKENS),
        now,
    );
    metrics.generation_tokens_per_second = counter_rate(
        state,
        GENERATION_TOKENS[0],
        sum_first(scrape, &GENERATION_TOKENS),
        now,
    );
    metrics.requests_per_second = counter_rate(
        state,
        REQUESTS_TOTAL[0],
        sum_first(scrape, &REQUESTS_TOTAL),
        now,
    );

    metrics.ttft_p50_seconds = worst_quantile(scrape, TTFT_HISTOGRAM, 0.50);
    metrics.ttft_p95_seconds = worst_quantile(scrape, TTFT_HISTOGRAM, 0.95);
    metrics.e2e_latency_p95_seconds = worst_quantile(scrape, E2E_HISTOGRAM, 0.95);
    metrics.queue_time_p95_seconds = worst_quantile(scrape, QUEUE_HISTOGRAM, 0.95);
    if !has_histogram(scrape, TTFT_HISTOGRAM) {
        unsupported.push(TTFT_HISTOGRAM.to_string());
    }

    metrics.unsupported = unsupported;
    metrics
}

impl LlmRuntimeAdapter for SglangAdapter {
    fn check_health(&self, client: &dyn HttpClient) -> RuntimeStatus {
        health_from_get(
            client,
            HEALTH_PATH,
            &self.instance_id,
            RuntimeType::Sglang,
            self.now,
        )
    }

    fn get_models(&self, client: &dyn HttpClient) -> Result<Vec<RuntimeModel>, RuntimeError> {
        parse_openai_models(&get_body(client, OPENAI_MODELS_PATH)?, "SGLang")
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
# TYPE sglang:num_running_reqs gauge
sglang:num_running_reqs{model_name="qwen"} 6.0
# TYPE sglang:num_queue_reqs gauge
sglang:num_queue_reqs{model_name="qwen"} 2.0
# TYPE sglang:token_usage gauge
sglang:token_usage{model_name="qwen"} 0.61
# TYPE sglang:cache_hit_rate gauge
sglang:cache_hit_rate{model_name="qwen"} 0.4
# TYPE sglang:prompt_tokens_total counter
sglang:prompt_tokens_total{model_name="qwen"} 1000
# TYPE sglang:generation_tokens_total counter
sglang:generation_tokens_total{model_name="qwen"} 400
# TYPE sglang:time_to_first_token_seconds histogram
sglang:time_to_first_token_seconds_bucket{le="0.1",model_name="qwen"} 10
sglang:time_to_first_token_seconds_bucket{le="0.5",model_name="qwen"} 90
sglang:time_to_first_token_seconds_bucket{le="+Inf",model_name="qwen"} 100
sglang:time_to_first_token_seconds_count{model_name="qwen"} 100
"#;

    #[test]
    fn reads_queue_kv_and_cache_gauges() {
        let mut state = CounterState::default();
        let metrics = metrics_from_scrape(&prometheus::parse(METRICS_BODY), &mut state, NOW);
        assert_eq!(metrics.requests_running, Some(6.0));
        assert_eq!(metrics.requests_waiting, Some(2.0));
        assert!((metrics.kv_cache_usage_ratio.unwrap() - 0.61).abs() < 1e-9);
        assert!((metrics.prefix_cache_hit_ratio.unwrap() - 0.4).abs() < 1e-9);
        assert!(metrics.ttft_p95_seconds.is_some());
        // Counters need two readings before they have a rate.
        assert_eq!(metrics.prompt_tokens_per_second, None);
        assert!(!metrics.unsupported.contains(&TOKEN_USAGE.to_string()));
    }

    #[test]
    fn token_rates_come_from_successive_scrapes() {
        let mut state = CounterState::default();
        metrics_from_scrape(&prometheus::parse(METRICS_BODY), &mut state, NOW);
        let later = METRICS_BODY
            .replace(
                "prompt_tokens_total{model_name=\"qwen\"} 1000",
                "prompt_tokens_total{model_name=\"qwen\"} 1500",
            )
            .replace(
                "generation_tokens_total{model_name=\"qwen\"} 400",
                "generation_tokens_total{model_name=\"qwen\"} 600",
            );
        let metrics = metrics_from_scrape(&prometheus::parse(&later), &mut state, NOW + 10);
        assert_eq!(metrics.prompt_tokens_per_second, Some(50.0));
        assert_eq!(metrics.generation_tokens_per_second, Some(20.0));
    }

    #[test]
    fn a_server_without_metrics_reports_them_unsupported() {
        let mut state = CounterState::default();
        let metrics = metrics_from_scrape(&prometheus::parse(""), &mut state, NOW);
        assert_eq!(metrics.requests_running, None);
        assert!(metrics.unsupported.contains(&REQUESTS_RUNNING.to_string()));
        assert!(metrics.unsupported.contains(&TOKEN_USAGE.to_string()));
    }

    #[test]
    fn health_and_models_use_the_openai_listing() {
        let adapter = SglangAdapter::new("s", NOW);
        let client = FakeHttpClient::new()
            .with_body(HEALTH_PATH, 200, "")
            .with_body(
                OPENAI_MODELS_PATH,
                200,
                r#"{"data":[{"id":"qwen","max_model_len":32768}]}"#,
            );
        assert_eq!(adapter.check_health(&client).status, "online");
        let models = adapter.get_models(&client).unwrap();
        assert_eq!(models[0].name, "qwen");
        assert_eq!(models[0].context_length, Some(32768));
    }
}
