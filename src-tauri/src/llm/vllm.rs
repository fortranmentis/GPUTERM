//! vLLM adapter: `/health`, `/v1/models`, and `/metrics`.
//!
//! Metric names move between vLLM versions, so support is probed per metric and
//! a missing one is recorded as unsupported instead of failing the collection or
//! being reported as zero.

use serde::Deserialize;
use std::collections::BTreeMap;

use super::adapter::{
    LlmRuntimeAdapter, RuntimeError, RuntimeErrorCode, RuntimeMetrics, RuntimeModel, RuntimeStatus,
    RuntimeType,
};
use super::http::{error_for_status, HttpClient};
use super::monitor::CounterState;
use super::prometheus::{self, Scrape};

pub const HEALTH_PATH: &str = "/health";
pub const MODELS_PATH: &str = "/v1/models";
pub const METRICS_PATH: &str = "/metrics";

/// Current name first, legacy fallback second.
const KV_CACHE_METRICS: [&str; 2] = ["vllm:kv_cache_usage_perc", "vllm:gpu_cache_usage_perc"];

const REQUESTS_RUNNING: &str = "vllm:num_requests_running";
const REQUESTS_WAITING: &str = "vllm:num_requests_waiting";
const REQUESTS_SWAPPED: &str = "vllm:num_requests_swapped";
const PREFIX_QUERIES: &str = "vllm:prefix_cache_queries";
const PREFIX_HITS: &str = "vllm:prefix_cache_hits";
const PROMPT_TOKENS: &str = "vllm:prompt_tokens_total";
const GENERATION_TOKENS: &str = "vllm:generation_tokens_total";
const REQUEST_SUCCESS: &str = "vllm:request_success_total";
const PREEMPTIONS: &str = "vllm:num_preemptions_total";
const TTFT_HISTOGRAM: &str = "vllm:time_to_first_token_seconds";
const E2E_HISTOGRAM: &str = "vllm:e2e_request_latency_seconds";
const QUEUE_HISTOGRAM: &str = "vllm:request_queue_time_seconds";

pub struct VllmAdapter {
    instance_id: String,
    now: u64,
}

impl VllmAdapter {
    pub fn new(instance_id: &str, now: u64) -> Self {
        Self {
            instance_id: instance_id.to_string(),
            now,
        }
    }
}

#[derive(Debug, Deserialize)]
struct VllmModelEntry {
    id: String,
    #[serde(default)]
    object: Option<String>,
    #[serde(default)]
    created: Option<i64>,
    #[serde(default)]
    owned_by: Option<String>,
    /// Present on some builds; absent on others. Never guessed when missing.
    #[serde(default)]
    max_model_len: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct VllmModelList {
    #[serde(default)]
    data: Vec<VllmModelEntry>,
}

fn parse_models(body: &str, now: u64) -> Result<Vec<RuntimeModel>, RuntimeError> {
    let parsed: VllmModelList = serde_json::from_str(body).map_err(|error| {
        RuntimeError::new(
            RuntimeErrorCode::ParseError,
            format!("Could not read the vLLM model list: {}", error),
        )
    })?;
    let _ = now;

    Ok(parsed
        .data
        .into_iter()
        .map(|entry| {
            let mut model = RuntimeModel::new(entry.id.clone(), entry.id, "served");
            // Left as None when absent: the UI shows "unknown" rather than a
            // guessed context window.
            model.context_length = entry.max_model_len;
            let mut metadata = BTreeMap::new();
            if let Some(object) = entry.object {
                metadata.insert("object".to_string(), object);
            }
            if let Some(owned_by) = entry.owned_by {
                metadata.insert("ownedBy".to_string(), owned_by);
            }
            if let Some(created) = entry.created {
                metadata.insert("created".to_string(), created.to_string());
            }
            model.metadata = metadata;
            model
        })
        .collect())
}

/// Reads the first KV cache metric this server exposes, preferring the current
/// name over the legacy one.
fn kv_cache_usage(scrape: &Scrape) -> Option<f64> {
    for name in KV_CACHE_METRICS {
        if scrape.has(name) {
            // Several engines each report their own gauge; average rather than
            // sum, since this is a ratio.
            let samples = scrape.samples(name);
            let usable: Vec<f64> = samples
                .iter()
                .map(|sample| sample.value)
                .filter(|value| value.is_finite())
                .collect();
            if usable.is_empty() {
                continue;
            }
            let mean = usable.iter().sum::<f64>() / usable.len() as f64;
            // Some builds report a percentage, others a 0-1 ratio.
            let ratio = if mean > 1.0 { mean / 100.0 } else { mean };
            return Some(ratio.clamp(0.0, 1.0));
        }
    }
    None
}

/// Highest percentile across label sets.
///
/// Series for different models are never merged into one histogram — summing
/// unrelated label sets would produce a number that describes nothing. Taking
/// the worst is a defensible summary for a single card.
fn worst_quantile(scrape: &Scrape, base_name: &str, quantile: f64) -> Option<f64> {
    let mut worst: Option<f64> = None;
    for histogram in scrape.histograms(base_name) {
        if let Some(value) = histogram.quantile(quantile) {
            worst = Some(worst.map_or(value, |current: f64| current.max(value)));
        }
    }
    worst
}

/// Builds metrics from a scrape, folding in the previous counter reading.
pub fn metrics_from_scrape(
    scrape: &Scrape,
    state: &mut CounterState,
    now: u64,
) -> (RuntimeMetrics, bool) {
    let mut metrics = RuntimeMetrics {
        collected_at: now,
        ..Default::default()
    };
    let mut unsupported = Vec::new();

    metrics.requests_running = scrape.sum(REQUESTS_RUNNING);
    metrics.requests_waiting = scrape.sum(REQUESTS_WAITING);
    if scrape.has(REQUESTS_SWAPPED) {
        metrics.requests_swapped = scrape.sum(REQUESTS_SWAPPED);
    } else {
        // Removed in newer vLLM. Showing 0 would claim there is no swapping.
        unsupported.push(REQUESTS_SWAPPED.to_string());
    }

    metrics.kv_cache_usage_ratio = kv_cache_usage(scrape);
    metrics.kv_cache_remaining_ratio = metrics
        .kv_cache_usage_ratio
        .map(|usage| (1.0 - usage).max(0.0));
    if metrics.kv_cache_usage_ratio.is_none() {
        unsupported.push(KV_CACHE_METRICS[0].to_string());
    }

    metrics.prefix_cache_hit_ratio = match (scrape.sum(PREFIX_QUERIES), scrape.sum(PREFIX_HITS)) {
        // A zero denominator is unknown, not zero percent.
        (Some(queries), Some(hits)) if queries > 0.0 => Some((hits / queries).clamp(0.0, 1.0)),
        _ => None,
    };
    if !scrape.has(PREFIX_QUERIES) {
        unsupported.push(PREFIX_QUERIES.to_string());
    }

    let mut counters_reset = false;
    let mut rate = |name: &str, current: Option<f64>, state: &mut CounterState| -> Option<f64> {
        let current = current?;
        let outcome = state.observe(name, current, now);
        if outcome.reset {
            counters_reset = true;
        }
        outcome.per_second
    };

    metrics.prompt_tokens_per_second = rate(PROMPT_TOKENS, scrape.sum(PROMPT_TOKENS), state);
    metrics.generation_tokens_per_second =
        rate(GENERATION_TOKENS, scrape.sum(GENERATION_TOKENS), state);
    metrics.requests_per_second = rate(REQUEST_SUCCESS, scrape.sum(REQUEST_SUCCESS), state);

    if scrape.has(PREEMPTIONS) {
        let total = scrape.sum(PREEMPTIONS);
        metrics.preemptions_total = total;
        if let Some(total) = total {
            let outcome = state.observe(PREEMPTIONS, total, now);
            if outcome.reset {
                counters_reset = true;
            }
            metrics.preemptions_delta = outcome.delta;
        }
    } else {
        unsupported.push(PREEMPTIONS.to_string());
    }

    metrics.ttft_p50_seconds = worst_quantile(scrape, TTFT_HISTOGRAM, 0.50);
    metrics.ttft_p95_seconds = worst_quantile(scrape, TTFT_HISTOGRAM, 0.95);
    metrics.e2e_latency_p95_seconds = worst_quantile(scrape, E2E_HISTOGRAM, 0.95);
    metrics.queue_time_p95_seconds = worst_quantile(scrape, QUEUE_HISTOGRAM, 0.95);
    if !scrape.has(&format!("{}_bucket", TTFT_HISTOGRAM)) {
        unsupported.push(TTFT_HISTOGRAM.to_string());
    }

    metrics.unsupported = unsupported;
    (metrics, counters_reset)
}

impl LlmRuntimeAdapter for VllmAdapter {
    fn check_health(&self, client: &dyn HttpClient) -> RuntimeStatus {
        match client.get(HEALTH_PATH) {
            Ok(response) if response.is_success() => RuntimeStatus::online(
                &self.instance_id,
                RuntimeType::Vllm,
                response.elapsed_ms,
                self.now,
            ),
            Ok(response) => RuntimeStatus::failed(
                &self.instance_id,
                RuntimeType::Vllm,
                Some(response.elapsed_ms),
                self.now,
                &error_for_status(response.status),
            ),
            Err(error) => RuntimeStatus::failed(
                &self.instance_id,
                RuntimeType::Vllm,
                None,
                self.now,
                &error,
            ),
        }
    }

    fn get_models(&self, client: &dyn HttpClient) -> Result<Vec<RuntimeModel>, RuntimeError> {
        let response = client.get(MODELS_PATH)?;
        if !response.is_success() {
            return Err(error_for_status(response.status));
        }
        parse_models(&response.body, self.now)
    }

    fn get_runtime_metrics(
        &self,
        client: &dyn HttpClient,
        state: &mut CounterState,
        now: u64,
    ) -> Result<Option<RuntimeMetrics>, RuntimeError> {
        let response = client.get(METRICS_PATH)?;
        if !response.is_success() {
            return Err(error_for_status(response.status));
        }
        let scrape = prometheus::parse(&response.body);
        let (metrics, _) = metrics_from_scrape(&scrape, state, now);
        Ok(Some(metrics))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::http::testing::FakeHttpClient;

    const TEST_NOW: u64 = 1_799_000_000;

    const METRICS_BODY: &str = r#"
# TYPE vllm:num_requests_running gauge
vllm:num_requests_running{engine="0",model_name="m"} 3.0
vllm:num_requests_running{engine="1",model_name="m"} 2.0
# TYPE vllm:num_requests_waiting gauge
vllm:num_requests_waiting{engine="0",model_name="m"} 4.0
# TYPE vllm:kv_cache_usage_perc gauge
vllm:kv_cache_usage_perc{engine="0",model_name="m"} 0.42
# TYPE vllm:prefix_cache_queries counter
vllm:prefix_cache_queries{model_name="m"} 200
# TYPE vllm:prefix_cache_hits counter
vllm:prefix_cache_hits{model_name="m"} 50
# TYPE vllm:prompt_tokens_total counter
vllm:prompt_tokens_total{model_name="m"} 1000
# TYPE vllm:generation_tokens_total counter
vllm:generation_tokens_total{model_name="m"} 500
# TYPE vllm:request_success_total counter
vllm:request_success_total{model_name="m"} 40
# TYPE vllm:num_preemptions_total counter
vllm:num_preemptions_total{model_name="m"} 7
# TYPE vllm:time_to_first_token_seconds histogram
vllm:time_to_first_token_seconds_bucket{le="0.1",model_name="m"} 10
vllm:time_to_first_token_seconds_bucket{le="0.5",model_name="m"} 60
vllm:time_to_first_token_seconds_bucket{le="1.0",model_name="m"} 95
vllm:time_to_first_token_seconds_bucket{le="+Inf",model_name="m"} 100
vllm:time_to_first_token_seconds_count{model_name="m"} 100
"#;

    fn adapter() -> VllmAdapter {
        VllmAdapter::new("inst-1", TEST_NOW)
    }

    #[test]
    fn health_200_is_online_and_503_is_engine_dead() {
        let healthy = FakeHttpClient::new().with_body(HEALTH_PATH, 200, "");
        let status = adapter().check_health(&healthy);
        assert_eq!(status.status, "online");
        assert!(status.response_time_ms.is_some());

        let dead = FakeHttpClient::new().with_body(HEALTH_PATH, 503, "");
        let status = adapter().check_health(&dead);
        assert_eq!(status.status, "error");
        assert_eq!(status.error_code.as_deref(), Some("engine_dead"));

        let unauthorized = FakeHttpClient::new().with_body(HEALTH_PATH, 401, "");
        assert_eq!(
            adapter().check_health(&unauthorized).error_code.as_deref(),
            Some("authentication_error")
        );

        let timed_out = FakeHttpClient::new().with_error(
            HEALTH_PATH,
            RuntimeError::new(RuntimeErrorCode::Timeout, "slow"),
        );
        assert_eq!(adapter().check_health(&timed_out).status, "offline");
    }

    #[test]
    fn served_models_parse_and_an_absent_context_stays_unknown() {
        let body = r#"{"object":"list","data":[
            {"id":"meta-llama/Llama-3-8B","object":"model","created":1700000000,"owned_by":"vllm"}
        ]}"#;
        let client = FakeHttpClient::new().with_body(MODELS_PATH, 200, body);
        let models = adapter().get_models(&client).unwrap();

        assert_eq!(models.len(), 1);
        assert_eq!(models[0].name, "meta-llama/Llama-3-8B");
        assert_eq!(models[0].status, "served");
        assert_eq!(
            models[0].context_length, None,
            "never guessed when the server omits it"
        );
        assert_eq!(models[0].metadata.get("ownedBy").map(String::as_str), Some("vllm"));
    }

    #[test]
    fn gauges_sum_across_engines_and_kv_cache_is_a_ratio() {
        let scrape = prometheus::parse(METRICS_BODY);
        let mut state = CounterState::default();
        let (metrics, _) = metrics_from_scrape(&scrape, &mut state, TEST_NOW);

        assert_eq!(metrics.requests_running, Some(5.0));
        assert_eq!(metrics.requests_waiting, Some(4.0));
        assert_eq!(metrics.kv_cache_usage_ratio, Some(0.42));
        let remaining = metrics.kv_cache_remaining_ratio.unwrap();
        assert!((remaining - 0.58).abs() < 1e-9, "{remaining}");
        assert_eq!(metrics.prefix_cache_hit_ratio, Some(0.25));
    }

    #[test]
    fn the_legacy_gpu_cache_metric_is_used_only_as_a_fallback() {
        let legacy = "# TYPE vllm:gpu_cache_usage_perc gauge\nvllm:gpu_cache_usage_perc{e=\"0\"} 0.7\n";
        let scrape = prometheus::parse(legacy);
        let mut state = CounterState::default();
        let (metrics, _) = metrics_from_scrape(&scrape, &mut state, TEST_NOW);
        assert_eq!(metrics.kv_cache_usage_ratio, Some(0.7));

        // When both exist the current name wins.
        let both = "\
# TYPE vllm:kv_cache_usage_perc gauge
vllm:kv_cache_usage_perc{e=\"0\"} 0.3
# TYPE vllm:gpu_cache_usage_perc gauge
vllm:gpu_cache_usage_perc{e=\"0\"} 0.9
";
        let scrape = prometheus::parse(both);
        let mut state = CounterState::default();
        let (metrics, _) = metrics_from_scrape(&scrape, &mut state, TEST_NOW);
        assert_eq!(metrics.kv_cache_usage_ratio, Some(0.3));
    }

    #[test]
    fn metrics_absent_from_this_version_are_marked_unsupported_not_zero() {
        let scrape = prometheus::parse(METRICS_BODY);
        let mut state = CounterState::default();
        let (metrics, _) = metrics_from_scrape(&scrape, &mut state, TEST_NOW);

        assert_eq!(metrics.requests_swapped, None, "not Some(0.0)");
        assert!(metrics
            .unsupported
            .iter()
            .any(|name| name == "vllm:num_requests_swapped"));
    }

    #[test]
    fn a_zero_prefix_denominator_is_unknown_rather_than_zero_percent() {
        let body = "\
# TYPE vllm:prefix_cache_queries counter
vllm:prefix_cache_queries{m=\"a\"} 0
# TYPE vllm:prefix_cache_hits counter
vllm:prefix_cache_hits{m=\"a\"} 0
";
        let scrape = prometheus::parse(body);
        let mut state = CounterState::default();
        let (metrics, _) = metrics_from_scrape(&scrape, &mut state, TEST_NOW);
        assert_eq!(metrics.prefix_cache_hit_ratio, None);
    }

    #[test]
    fn throughput_needs_two_samples_and_then_uses_the_delta() {
        let scrape = prometheus::parse(METRICS_BODY);
        let mut state = CounterState::default();

        // First scrape has no baseline to compare against.
        let (first, reset) = metrics_from_scrape(&scrape, &mut state, TEST_NOW);
        assert_eq!(first.prompt_tokens_per_second, None);
        assert!(!reset);

        // 10 s later: prompt 1000 -> 1600 is 60/s, generation 500 -> 600 is 10/s.
        let later = METRICS_BODY
            .replace("vllm:prompt_tokens_total{model_name=\"m\"} 1000", "vllm:prompt_tokens_total{model_name=\"m\"} 1600")
            .replace("vllm:generation_tokens_total{model_name=\"m\"} 500", "vllm:generation_tokens_total{model_name=\"m\"} 600")
            .replace("vllm:request_success_total{model_name=\"m\"} 40", "vllm:request_success_total{model_name=\"m\"} 60")
            .replace("vllm:num_preemptions_total{model_name=\"m\"} 7", "vllm:num_preemptions_total{model_name=\"m\"} 9");
        let scrape = prometheus::parse(&later);
        let (second, reset) = metrics_from_scrape(&scrape, &mut state, TEST_NOW + 10);

        assert!(!reset);
        assert_eq!(second.prompt_tokens_per_second, Some(60.0));
        assert_eq!(second.generation_tokens_per_second, Some(10.0));
        assert_eq!(second.requests_per_second, Some(2.0));
        assert_eq!(second.preemptions_total, Some(9.0));
        assert_eq!(second.preemptions_delta, Some(2.0));
    }

    #[test]
    fn a_counter_going_backwards_reports_null_rather_than_a_negative_rate() {
        let scrape = prometheus::parse(METRICS_BODY);
        let mut state = CounterState::default();
        metrics_from_scrape(&scrape, &mut state, TEST_NOW);

        // The server restarted and its counters went back to near zero.
        let restarted = METRICS_BODY
            .replace("vllm:prompt_tokens_total{model_name=\"m\"} 1000", "vllm:prompt_tokens_total{model_name=\"m\"} 5")
            .replace("vllm:num_preemptions_total{model_name=\"m\"} 7", "vllm:num_preemptions_total{model_name=\"m\"} 0");
        let scrape = prometheus::parse(&restarted);
        let (metrics, reset) = metrics_from_scrape(&scrape, &mut state, TEST_NOW + 10);

        assert!(reset, "the restart is reported so it can be recorded");
        assert_eq!(metrics.prompt_tokens_per_second, None, "never negative");
        assert_eq!(metrics.preemptions_delta, None);

        // The new value became the baseline, so the next interval works again.
        let after = METRICS_BODY
            .replace("vllm:prompt_tokens_total{model_name=\"m\"} 1000", "vllm:prompt_tokens_total{model_name=\"m\"} 105");
        let scrape = prometheus::parse(&after);
        let (metrics, reset) = metrics_from_scrape(&scrape, &mut state, TEST_NOW + 20);
        assert!(!reset);
        assert_eq!(metrics.prompt_tokens_per_second, Some(10.0));
    }

    #[test]
    fn ttft_percentiles_come_from_the_histogram_buckets() {
        let scrape = prometheus::parse(METRICS_BODY);
        let mut state = CounterState::default();
        let (metrics, _) = metrics_from_scrape(&scrape, &mut state, TEST_NOW);

        // rank 50 sits in (0.1, 0.5], counts 10..60:
        // 0.1 + (0.5 - 0.1) * (50 - 10) / 50 = 0.42
        let p50 = metrics.ttft_p50_seconds.unwrap();
        assert!((p50 - 0.42).abs() < 1e-9, "{p50}");
        // rank 95 lands exactly on the 1.0 bucket boundary.
        let p95 = metrics.ttft_p95_seconds.unwrap();
        assert!((p95 - 1.0).abs() < 1e-9, "{p95}");
        // No e2e histogram in this fixture.
        assert_eq!(metrics.e2e_latency_p95_seconds, None);
    }

    #[test]
    fn a_partly_broken_scrape_still_yields_the_readable_metrics() {
        let body = format!("{}\nthis is not a metric {{{{\n", METRICS_BODY);
        let client = FakeHttpClient::new().with_body(METRICS_PATH, 200, &body);
        let mut state = CounterState::default();
        let metrics = adapter()
            .get_runtime_metrics(&client, &mut state, TEST_NOW)
            .unwrap()
            .unwrap();
        assert_eq!(metrics.requests_running, Some(5.0));
        assert_eq!(metrics.kv_cache_usage_ratio, Some(0.42));
    }

    #[test]
    fn metrics_endpoint_errors_surface_with_their_code() {
        let unauthorized = FakeHttpClient::new().with_body(METRICS_PATH, 401, "");
        let mut state = CounterState::default();
        let error = adapter()
            .get_runtime_metrics(&unauthorized, &mut state, TEST_NOW)
            .unwrap_err();
        assert_eq!(error.code, RuntimeErrorCode::AuthenticationError);

        let timed_out = FakeHttpClient::new().with_error(
            METRICS_PATH,
            RuntimeError::new(RuntimeErrorCode::Timeout, "slow"),
        );
        let error = adapter()
            .get_runtime_metrics(&timed_out, &mut state, TEST_NOW)
            .unwrap_err();
        assert_eq!(error.code, RuntimeErrorCode::Timeout);
    }
}
