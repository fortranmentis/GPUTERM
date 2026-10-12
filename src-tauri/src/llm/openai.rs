//! Generic OpenAI-compatible adapter (LM Studio, LocalAI, llamafile, …): the
//! only endpoint every such server shares is `/v1/models`, so it doubles as
//! the health check. No serving metrics are read.

use super::adapter::{
    LlmRuntimeAdapter, RuntimeError, RuntimeMetrics, RuntimeModel, RuntimeStatus, RuntimeType,
};
use super::http::HttpClient;
use super::monitor::CounterState;
use super::serving::{get_body, health_from_get, parse_openai_models, OPENAI_MODELS_PATH};

pub struct OpenAiAdapter {
    instance_id: String,
    now: u64,
}

impl OpenAiAdapter {
    pub fn new(instance_id: &str, now: u64) -> Self {
        Self {
            instance_id: instance_id.to_string(),
            now,
        }
    }
}

impl LlmRuntimeAdapter for OpenAiAdapter {
    fn check_health(&self, client: &dyn HttpClient) -> RuntimeStatus {
        health_from_get(
            client,
            OPENAI_MODELS_PATH,
            &self.instance_id,
            RuntimeType::OpenAi,
            self.now,
        )
    }

    fn get_models(&self, client: &dyn HttpClient) -> Result<Vec<RuntimeModel>, RuntimeError> {
        parse_openai_models(&get_body(client, OPENAI_MODELS_PATH)?, "OpenAI-compatible")
    }

    fn get_runtime_metrics(
        &self,
        _client: &dyn HttpClient,
        _state: &mut CounterState,
        _now: u64,
    ) -> Result<Option<RuntimeMetrics>, RuntimeError> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::http::testing::FakeHttpClient;

    #[test]
    fn the_model_listing_is_the_health_check() {
        let adapter = OpenAiAdapter::new("o", 1);
        let client = FakeHttpClient::new().with_body(
            OPENAI_MODELS_PATH,
            200,
            r#"{"data":[{"id":"lmstudio-community/qwen2.5-7b"}]}"#,
        );
        assert_eq!(adapter.check_health(&client).status, "online");
        assert_eq!(adapter.get_models(&client).unwrap().len(), 1);
        assert!(adapter
            .get_runtime_metrics(&client, &mut CounterState::default(), 1)
            .unwrap()
            .is_none());

        let unauthorized = FakeHttpClient::new().with_body(OPENAI_MODELS_PATH, 401, "");
        assert_eq!(
            adapter.check_health(&unauthorized).error_code.as_deref(),
            Some("authentication_error")
        );
    }
}
