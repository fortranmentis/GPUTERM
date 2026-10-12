//! NVIDIA Triton Inference Server adapter (KServe v2 HTTP API):
//! `/v2/health/ready` and `/v2/models/stats`.
//!
//! Triton serves Prometheus metrics on a separate port (8002 by default) that a
//! single base URL or tunnel cannot reach, so serving figures come from the
//! per-model statistics on the main port instead: request throughput from the
//! success counters. Queue depth, KV cache and latency percentiles are not in
//! those statistics and are reported unsupported.

use serde::Deserialize;
use std::collections::BTreeMap;

use super::adapter::{
    LlmRuntimeAdapter, RuntimeError, RuntimeErrorCode, RuntimeMetrics, RuntimeModel, RuntimeStatus,
    RuntimeType,
};
use super::http::HttpClient;
use super::monitor::CounterState;
use super::serving::{counter_rate, get_body, health_from_get};

pub const HEALTH_PATH: &str = "/v2/health/ready";
pub const STATS_PATH: &str = "/v2/models/stats";

const SUCCESS_COUNTER: &str = "triton:inference_success";

pub struct TritonAdapter {
    instance_id: String,
    now: u64,
}

impl TritonAdapter {
    pub fn new(instance_id: &str, now: u64) -> Self {
        Self {
            instance_id: instance_id.to_string(),
            now,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct StatCount {
    #[serde(default)]
    count: u64,
}

#[derive(Debug, Default, Deserialize)]
struct InferenceStats {
    #[serde(default)]
    success: StatCount,
}

#[derive(Debug, Deserialize)]
struct ModelStat {
    name: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    inference_count: Option<u64>,
    #[serde(default)]
    inference_stats: InferenceStats,
}

#[derive(Debug, Deserialize)]
struct StatsResponse {
    #[serde(default)]
    model_stats: Vec<ModelStat>,
}

fn parse_stats(body: &str) -> Result<Vec<ModelStat>, RuntimeError> {
    serde_json::from_str::<StatsResponse>(body)
        .map(|response| response.model_stats)
        .map_err(|error| {
            RuntimeError::new(
                RuntimeErrorCode::ParseError,
                format!("Could not read the Triton model statistics: {}", error),
            )
        })
}

fn models_from_stats(stats: &[ModelStat]) -> Vec<RuntimeModel> {
    stats
        .iter()
        .map(|stat| {
            let id = match &stat.version {
                Some(version) if !version.is_empty() => format!("{}:{}", stat.name, version),
                _ => stat.name.clone(),
            };
            let mut model = RuntimeModel::new(id, stat.name.clone(), "served");
            let mut metadata = BTreeMap::new();
            if let Some(version) = &stat.version {
                metadata.insert("version".to_string(), version.clone());
            }
            if let Some(count) = stat.inference_count {
                metadata.insert("inferenceCount".to_string(), count.to_string());
            }
            model.metadata = metadata;
            model
        })
        .collect()
}

impl LlmRuntimeAdapter for TritonAdapter {
    fn check_health(&self, client: &dyn HttpClient) -> RuntimeStatus {
        health_from_get(
            client,
            HEALTH_PATH,
            &self.instance_id,
            RuntimeType::Triton,
            self.now,
        )
    }

    fn get_models(&self, client: &dyn HttpClient) -> Result<Vec<RuntimeModel>, RuntimeError> {
        Ok(models_from_stats(&parse_stats(&get_body(
            client, STATS_PATH,
        )?)?))
    }

    fn get_runtime_metrics(
        &self,
        client: &dyn HttpClient,
        state: &mut CounterState,
        now: u64,
    ) -> Result<Option<RuntimeMetrics>, RuntimeError> {
        let stats = parse_stats(&get_body(client, STATS_PATH)?)?;
        let successes: u64 = stats
            .iter()
            .map(|stat| stat.inference_stats.success.count)
            .sum();
        Ok(Some(RuntimeMetrics {
            requests_per_second: counter_rate(state, SUCCESS_COUNTER, Some(successes as f64), now),
            collected_at: now,
            unsupported: vec![
                "requests_running".to_string(),
                "requests_waiting".to_string(),
                "kv_cache_usage".to_string(),
                "latency_percentiles (Triton metrics port)".to_string(),
            ],
            ..Default::default()
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::http::testing::FakeHttpClient;

    const STATS_BODY: &str = r#"{"model_stats":[
        {"name":"ensemble","version":"1","inference_count":40,"inference_stats":{"success":{"count":40,"ns":1000}}},
        {"name":"tensorrt_llm","version":"1","inference_count":60,"inference_stats":{"success":{"count":60,"ns":5000}}}
    ]}"#;

    #[test]
    fn models_come_from_the_statistics() {
        let adapter = TritonAdapter::new("t", 1);
        let client = FakeHttpClient::new().with_body(STATS_PATH, 200, STATS_BODY);
        let models = adapter.get_models(&client).unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[1].name, "tensorrt_llm");
        assert_eq!(models[1].id, "tensorrt_llm:1");
        assert_eq!(
            models[1].metadata.get("inferenceCount").map(String::as_str),
            Some("60")
        );
    }

    #[test]
    fn throughput_is_the_rate_of_summed_successes() {
        let adapter = TritonAdapter::new("t", 1);
        let mut state = CounterState::default();
        let first = FakeHttpClient::new().with_body(STATS_PATH, 200, STATS_BODY);
        let metrics = adapter
            .get_runtime_metrics(&first, &mut state, 100)
            .unwrap()
            .unwrap();
        assert_eq!(metrics.requests_per_second, None);

        let later = STATS_BODY.replace("\"count\":60", "\"count\":80");
        let second = FakeHttpClient::new().with_body(STATS_PATH, 200, &later);
        let metrics = adapter
            .get_runtime_metrics(&second, &mut state, 110)
            .unwrap()
            .unwrap();
        assert_eq!(metrics.requests_per_second, Some(2.0));
        assert!(metrics
            .unsupported
            .contains(&"requests_waiting".to_string()));
    }

    #[test]
    fn readiness_maps_to_status() {
        let adapter = TritonAdapter::new("t", 1);
        let ready = FakeHttpClient::new().with_body(HEALTH_PATH, 200, "");
        assert_eq!(adapter.check_health(&ready).status, "online");
        let not_ready = FakeHttpClient::new().with_body(HEALTH_PATH, 503, "");
        assert_eq!(adapter.check_health(&not_ready).status, "error");
    }
}
