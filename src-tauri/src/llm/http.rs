//! The one place the app talks to a user-configured LLM runtime.
//!
//! Everything goes through the `HttpClient` trait so adapters can be tested
//! against fixtures with no server and no network. The concrete implementation
//! is blocking, matching the rest of the backend, and is always called from
//! `spawn_blocking` or a dedicated thread.

use super::adapter::{RuntimeError, RuntimeErrorCode};
use std::sync::OnceLock;
use std::time::Duration;

/// Ceiling on a response body. `/metrics` is tens of kilobytes on a busy server;
/// this stops a misconfigured URL pointing at something huge from being read
/// into memory.
const MAX_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;

pub struct HttpResponse {
    pub status: u16,
    pub body: String,
    pub elapsed_ms: u64,
}

impl HttpResponse {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// A GET against one runtime. Implemented once for real HTTP and once for tests.
pub trait HttpClient {
    /// `path` is absolute and starts with `/`. A non-2xx response is returned as
    /// `Ok` — deciding what a 503 means is the adapter's job.
    fn get(&self, path: &str) -> Result<HttpResponse, RuntimeError>;
}

/// Real client, bound to one instance's base URL, timeout, and optional token.
pub struct UreqClient {
    base_url: String,
    api_key: Option<String>,
    timeout: Duration,
}

impl UreqClient {
    pub fn new(base_url: &str, api_key: Option<String>, timeout: Duration) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            timeout,
        }
    }
}

/// One shared agent so connections are pooled across polls. ureq's agent holds
/// no threads and no runtime, unlike a reqwest blocking client.
fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            // The OS trust store, not ureq's bundled Mozilla roots: a
            // self-hosted runtime behind an internal CA is the common case.
            .tls_config(
                ureq::tls::TlsConfig::builder()
                    .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                    .build(),
            )
            .http_status_as_error(false)
            .build()
            .into()
    })
}

impl HttpClient for UreqClient {
    fn get(&self, path: &str) -> Result<HttpResponse, RuntimeError> {
        let url = format!("{}{}", self.base_url, path);
        let started = std::time::Instant::now();

        let mut request = agent()
            .get(&url)
            .config()
            .timeout_global(Some(self.timeout))
            .build();
        if let Some(key) = &self.api_key {
            request = request.header("Authorization", &format!("Bearer {}", key));
        }

        let response = request.call().map_err(|error| classify_transport(&error))?;
        let status = response.status().as_u16();
        let body = response
            .into_body()
            .with_config()
            .limit(MAX_RESPONSE_BYTES)
            .read_to_string()
            .map_err(|error| {
                RuntimeError::new(
                    RuntimeErrorCode::InvalidResponse,
                    format!("Could not read the response body: {}", redact(&error.to_string())),
                )
            })?;

        Ok(HttpResponse {
            status,
            body,
            elapsed_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        })
    }
}

/// Maps a transport failure onto the closed error set. The message is
/// summarized rather than passed through, so no URL credentials or internal
/// detail reaches the UI.
fn classify_transport(error: &ureq::Error) -> RuntimeError {
    let text = error.to_string().to_ascii_lowercase();
    let (code, message) = if matches!(error, ureq::Error::Timeout(_)) || text.contains("timed out") {
        (RuntimeErrorCode::Timeout, "The request timed out.")
    } else if text.contains("dns") || text.contains("name or service not known") {
        (
            RuntimeErrorCode::DnsError,
            "The host name could not be resolved.",
        )
    } else if text.contains("connection refused") || text.contains("refused") {
        (
            RuntimeErrorCode::ConnectionRefused,
            "The server refused the connection.",
        )
    } else if text.contains("certificate") || text.contains("tls") {
        (
            RuntimeErrorCode::InvalidResponse,
            "The TLS certificate could not be verified.",
        )
    } else {
        // Something went wrong below HTTP that none of the cases above name.
        // Reported honestly rather than guessed at as a refusal.
        (
            RuntimeErrorCode::UnknownError,
            "The server could not be reached.",
        )
    };
    RuntimeError::new(code, message)
}

/// Turns an HTTP status into the matching error, for callers that treat any
/// non-2xx as a failure.
pub fn error_for_status(status: u16) -> RuntimeError {
    match status {
        401 | 403 => RuntimeError::new(
            RuntimeErrorCode::AuthenticationError,
            "The server rejected the credentials. Check the API key.",
        ),
        503 => RuntimeError::new(
            RuntimeErrorCode::EngineDead,
            "The server reported that its engine is not ready (HTTP 503).",
        ),
        400..=499 => RuntimeError::new(
            RuntimeErrorCode::HttpClientError,
            format!("The server rejected the request (HTTP {}).", status),
        ),
        500..=599 => RuntimeError::new(
            RuntimeErrorCode::HttpServerError,
            format!("The server reported an error (HTTP {}).", status),
        ),
        _ => RuntimeError::new(
            RuntimeErrorCode::InvalidResponse,
            format!("Unexpected HTTP status {}.", status),
        ),
    }
}

/// Removes anything credential-shaped from a string before it is shown or
/// logged. Applied to every message that could have come from a URL or header.
pub fn redact(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for token in text.split_inclusive(char::is_whitespace) {
        let lowered = token.to_ascii_lowercase();
        // Once a credential marker appears, everything after it is dropped: the
        // secret may be the next token (`Bearer <key>`), the one after that
        // (`Authorization: Bearer <key>`), or part of this one (`api_key=…`).
        // Guessing which would eventually guess wrong, so the tail goes.
        if lowered.contains("bearer")
            || lowered.contains("authorization")
            || lowered.contains("api_key")
            || lowered.contains("apikey")
            || lowered.contains("token=")
        {
            output.push_str("[redacted]");
            return output;
        }
        // user:password@host in a URL.
        if let Some(at) = token.find('@') {
            if token[..at].contains("://") && token[..at].contains(':') {
                if let Some(scheme_end) = token.find("://") {
                    output.push_str(&token[..scheme_end + 3]);
                    output.push_str("[redacted]");
                    output.push_str(&token[at..]);
                    continue;
                }
            }
        }
        output.push_str(token);
    }
    output.trim_end().to_string()
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Serves canned responses by path so adapters can be exercised without a
    /// server. Mirrors the `MemoryCredentialStore` seam used elsewhere.
    pub struct FakeHttpClient {
        responses: HashMap<String, Result<HttpResponse, RuntimeError>>,
        pub requested: Mutex<Vec<String>>,
    }

    impl FakeHttpClient {
        pub fn new() -> Self {
            Self {
                responses: HashMap::new(),
                requested: Mutex::new(Vec::new()),
            }
        }

        pub fn with_body(mut self, path: &str, status: u16, body: &str) -> Self {
            self.responses.insert(
                path.to_string(),
                Ok(HttpResponse {
                    status,
                    body: body.to_string(),
                    elapsed_ms: 7,
                }),
            );
            self
        }

        pub fn with_error(mut self, path: &str, error: RuntimeError) -> Self {
            self.responses.insert(path.to_string(), Err(error));
            self
        }
    }

    impl HttpClient for FakeHttpClient {
        fn get(&self, path: &str) -> Result<HttpResponse, RuntimeError> {
            if let Ok(mut requested) = self.requested.lock() {
                requested.push(path.to_string());
            }
            match self.responses.get(path) {
                Some(Ok(response)) => Ok(HttpResponse {
                    status: response.status,
                    body: response.body.clone(),
                    elapsed_ms: response.elapsed_ms,
                }),
                Some(Err(error)) => Err(error.clone()),
                None => Err(RuntimeError::new(
                    RuntimeErrorCode::ConnectionRefused,
                    format!("no fixture for {}", path),
                )),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_codes_map_to_the_documented_errors() {
        assert_eq!(
            error_for_status(401).code,
            RuntimeErrorCode::AuthenticationError
        );
        assert_eq!(
            error_for_status(403).code,
            RuntimeErrorCode::AuthenticationError
        );
        // vLLM answers 503 while its engine is down, which is distinct from a
        // generic server error.
        assert_eq!(error_for_status(503).code, RuntimeErrorCode::EngineDead);
        assert_eq!(error_for_status(404).code, RuntimeErrorCode::HttpClientError);
        assert_eq!(error_for_status(500).code, RuntimeErrorCode::HttpServerError);
    }

    #[test]
    fn error_messages_never_carry_credentials() {
        let cases = [
            "failed on https://user:hunter2@vllm.internal/metrics",
            "rejected header Authorization: Bearer sk-abc123",
            "bad request api_key=sk-secret",
        ];
        for case in cases {
            let redacted = redact(case);
            assert!(!redacted.contains("hunter2"), "{redacted}");
            assert!(!redacted.contains("sk-abc123"), "{redacted}");
            assert!(!redacted.contains("sk-secret"), "{redacted}");
        }
        // Ordinary text is left alone.
        assert_eq!(redact("connection refused"), "connection refused");
    }
}
