//! Pieces shared by the adapters for OpenAI-style serving runtimes (vLLM,
//! SGLang, TGI, llama.cpp, generic OpenAI-compatible servers, Triton).

use serde::Deserialize;
use std::collections::BTreeMap;

use super::adapter::{RuntimeError, RuntimeErrorCode, RuntimeModel, RuntimeStatus, RuntimeType};
use super::http::{error_for_status, HttpClient};
use super::monitor::CounterState;
use super::prometheus::Scrape;

pub const OPENAI_MODELS_PATH: &str = "/v1/models";

/// `online` for a 2xx on `path`, otherwise the status-mapped failure.
pub fn health_from_get(
    client: &dyn HttpClient,
    path: &str,
    instance_id: &str,
    runtime: RuntimeType,
    now: u64,
) -> RuntimeStatus {
    match client.get(path) {
        Ok(response) if response.is_success() => {
            RuntimeStatus::online(instance_id, runtime, response.elapsed_ms, now)
        }
        Ok(response) => RuntimeStatus::failed(
            instance_id,
            runtime,
            Some(response.elapsed_ms),
            now,
            &error_for_status(response.status),
        ),
        Err(error) => RuntimeStatus::failed(instance_id, runtime, None, now, &error),
    }
}

/// GETs `path` and returns its body, mapping a non-2xx status to an error.
pub fn get_body(client: &dyn HttpClient, path: &str) -> Result<String, RuntimeError> {
    let response = client.get(path)?;
    if !response.is_success() {
        return Err(error_for_status(response.status));
    }
    Ok(response.body)
}

#[derive(Debug, Deserialize)]
struct OpenAiModelEntry {
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
struct OpenAiModelList {
    #[serde(default)]
    data: Vec<OpenAiModelEntry>,
}

/// Parses an OpenAI-style `/v1/models` listing. `label` names the runtime in
/// the error message.
pub fn parse_openai_models(body: &str, label: &str) -> Result<Vec<RuntimeModel>, RuntimeError> {
    let parsed: OpenAiModelList = serde_json::from_str(body).map_err(|error| {
        RuntimeError::new(
            RuntimeErrorCode::ParseError,
            format!("Could not read the {} model list: {}", label, error),
        )
    })?;

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

/// Highest percentile across label sets.
///
/// Series for different models are never merged into one histogram — summing
/// unrelated label sets would produce a number that describes nothing. Taking
/// the worst is a defensible summary for a single card.
pub fn worst_quantile(scrape: &Scrape, base_name: &str, quantile: f64) -> Option<f64> {
    let mut worst: Option<f64> = None;
    for histogram in scrape.histograms(base_name) {
        if let Some(value) = histogram.quantile(quantile) {
            worst = Some(worst.map_or(value, |current: f64| current.max(value)));
        }
    }
    worst
}

/// Whether `base_name` is exposed as a histogram.
pub fn has_histogram(scrape: &Scrape, base_name: &str) -> bool {
    scrape.has(&format!("{}_bucket", base_name))
}

/// Mean of a ratio gauge across label sets (one per engine or slot), clamped to
/// 0-1. Values above 1 are read as percentages. `None` when nothing usable is
/// exposed.
pub fn mean_ratio(scrape: &Scrape, name: &str) -> Option<f64> {
    if !scrape.has(name) {
        return None;
    }
    let usable: Vec<f64> = scrape
        .samples(name)
        .iter()
        .map(|sample| sample.value)
        .filter(|value| value.is_finite())
        .collect();
    if usable.is_empty() {
        return None;
    }
    let mean = usable.iter().sum::<f64>() / usable.len() as f64;
    let ratio = if mean > 1.0 { mean / 100.0 } else { mean };
    Some(ratio.clamp(0.0, 1.0))
}

/// The first of `names` this server exposes, summed across label sets.
/// Exporters disagree on whether counters carry a `_total` suffix.
pub fn sum_first(scrape: &Scrape, names: &[&str]) -> Option<f64> {
    names
        .iter()
        .find(|name| scrape.has(name))
        .and_then(|name| scrape.sum(name))
}

/// Folds a counter reading into `state` and returns its per-second rate.
/// A counter that went backwards (a restart) is recorded by `state` itself.
pub fn counter_rate(
    state: &mut CounterState,
    key: &str,
    current: Option<f64>,
    now: u64,
) -> Option<f64> {
    state.observe(key, current?, now).per_second
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::prometheus;

    #[test]
    fn mean_ratio_reads_fractions_and_percentages() {
        let scrape = prometheus::parse("a{slot=\"0\"} 0.2\na{slot=\"1\"} 0.4\nb 85\n");
        assert!((mean_ratio(&scrape, "a").unwrap() - 0.3).abs() < 1e-9);
        assert!((mean_ratio(&scrape, "b").unwrap() - 0.85).abs() < 1e-9);
        assert_eq!(mean_ratio(&scrape, "missing"), None);
    }

    #[test]
    fn sum_first_prefers_the_first_exposed_name() {
        let scrape = prometheus::parse("x_total 4\nx 9\n");
        assert_eq!(sum_first(&scrape, &["x_total", "x"]), Some(4.0));
        assert_eq!(sum_first(&scrape, &["nope", "x"]), Some(9.0));
        assert_eq!(sum_first(&scrape, &["nope"]), None);
    }

    #[test]
    fn openai_models_are_served_and_keep_metadata() {
        let models = parse_openai_models(
            r#"{"object":"list","data":[{"id":"qwen","object":"model","owned_by":"me","max_model_len":8192}]}"#,
            "Test",
        )
        .unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].status, "served");
        assert_eq!(models[0].context_length, Some(8192));
        assert_eq!(
            models[0].metadata.get("ownedBy").map(String::as_str),
            Some("me")
        );

        let error = parse_openai_models("not json", "Test").unwrap_err();
        assert_eq!(error.code, RuntimeErrorCode::ParseError);
        assert!(error.message.contains("Test"));
    }
}
