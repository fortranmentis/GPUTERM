//! Warning levels for one instance.
//!
//! Kept as a single pure function so the bar summary and the detail view can
//! never disagree, and so the thresholds can be tested without a server.
//!
//! Reasons are returned as stable codes rather than sentences: the backend
//! speaks English everywhere else in this codebase, and the UI is Korean, so
//! the wording belongs in the frontend.

use super::adapter::{RuntimeMetrics, RuntimeType};

/// Ollama: a slower answer than this is worth flagging.
pub const OLLAMA_SLOW_RESPONSE_MS: u64 = 1_000;
/// Consecutive failed polls before an Ollama instance is called broken.
pub const CONSECUTIVE_FAILURE_LIMIT: u32 = 3;
pub const KV_CACHE_WARNING_RATIO: f64 = 0.70;
pub const KV_CACHE_CONGESTED_RATIO: f64 = 0.85;
pub const KV_CACHE_CRITICAL_RATIO: f64 = 0.95;
/// Consecutive polls with a non-empty queue before it counts as sustained.
pub const WAITING_SUSTAINED_TICKS: u32 = 3;

/// Ordered worst-last so levels can be combined with `max`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SeverityLevel {
    /// Never polled, or polling is switched off.
    Unknown,
    Normal,
    Warning,
    /// vLLM only: serving, but the queue is not draining.
    Congested,
    Critical,
}

impl SeverityLevel {
    pub fn key(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Normal => "normal",
            Self::Warning => "warning",
            Self::Congested => "congested",
            Self::Critical => "critical",
        }
    }
}

pub struct SeverityInput<'a> {
    pub runtime_type: RuntimeType,
    /// `online` | `degraded` | `offline` | `error`
    pub status: &'a str,
    pub error_code: Option<&'a str>,
    pub response_time_ms: Option<u64>,
    pub consecutive_failures: u32,
    pub metrics: Option<&'a RuntimeMetrics>,
    /// How many consecutive polls have seen at least one waiting request.
    pub waiting_streak: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Severity {
    pub level: SeverityLevel,
    pub reasons: Vec<String>,
}

impl Severity {
    fn new(level: SeverityLevel, reasons: Vec<&str>) -> Self {
        Self {
            level,
            reasons: reasons.into_iter().map(str::to_string).collect(),
        }
    }
}

pub fn evaluate(input: &SeverityInput) -> Severity {
    match input.runtime_type {
        RuntimeType::Ollama => evaluate_ollama(input),
        RuntimeType::Vllm => evaluate_vllm(input),
    }
}

fn evaluate_ollama(input: &SeverityInput) -> Severity {
    let mut level = SeverityLevel::Normal;
    let mut reasons: Vec<&str> = Vec::new();

    if input.status == "offline" || input.status == "error" {
        // The spec grades Ollama on repeated failure rather than a single one,
        // because a laptop-hosted Ollama drops a request now and then.
        if input.consecutive_failures >= CONSECUTIVE_FAILURE_LIMIT {
            level = SeverityLevel::Critical;
            reasons.push("repeated_failures");
        } else {
            level = SeverityLevel::Warning;
            reasons.push("poll_failed");
        }
        return Severity::new(level, reasons);
    }

    if input.status == "degraded" {
        level = SeverityLevel::Warning;
        reasons.push("parse_degraded");
    }

    if input
        .response_time_ms
        .is_some_and(|elapsed| elapsed >= OLLAMA_SLOW_RESPONSE_MS)
    {
        level = level.max(SeverityLevel::Warning);
        reasons.push("slow_response");
    }

    Severity::new(level, reasons)
}

fn evaluate_vllm(input: &SeverityInput) -> Severity {
    let mut level = SeverityLevel::Normal;
    let mut reasons: Vec<&str> = Vec::new();

    // Unlike Ollama, the spec treats any vLLM API failure as critical: a serving
    // endpoint that cannot answer /health is not serving.
    if input.status == "offline" || input.status == "error" {
        reasons.push(match input.error_code {
            Some("engine_dead") => "engine_dead",
            Some("authentication_error") => "authentication_error",
            // The poll never left this machine, so blaming the API would send
            // the user looking in the wrong place.
            Some("ssh_tunnel_error") | Some("ssh_host_untrusted") => "ssh_unreachable",
            _ => "api_error",
        });
        return Severity::new(SeverityLevel::Critical, reasons);
    }

    if input.status == "degraded" {
        level = SeverityLevel::Warning;
        reasons.push("parse_degraded");
    }

    let metrics = match input.metrics {
        Some(metrics) => metrics,
        None => return Severity::new(level, reasons),
    };

    // A missing reading is not a healthy reading; it simply cannot raise the
    // level, and the detail view marks it unsupported.
    if let Some(usage) = metrics.kv_cache_usage_ratio {
        if usage >= KV_CACHE_CRITICAL_RATIO {
            level = level.max(SeverityLevel::Critical);
            reasons.push("kv_cache_critical");
        } else if usage >= KV_CACHE_CONGESTED_RATIO {
            level = level.max(SeverityLevel::Congested);
            reasons.push("kv_cache_congested");
        } else if usage >= KV_CACHE_WARNING_RATIO {
            level = level.max(SeverityLevel::Warning);
            reasons.push("kv_cache_high");
        }
    }

    if metrics.requests_waiting.is_some_and(|waiting| waiting >= 1.0) {
        if input.waiting_streak >= WAITING_SUSTAINED_TICKS {
            level = level.max(SeverityLevel::Congested);
            reasons.push("waiting_sustained");
        } else {
            level = level.max(SeverityLevel::Warning);
            reasons.push("requests_waiting");
        }
    }

    if metrics.preemptions_delta.is_some_and(|delta| delta > 0.0) {
        level = level.max(SeverityLevel::Warning);
        reasons.push("preemption_increase");
    }

    Severity::new(level, reasons)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ollama(status: &str) -> SeverityInput<'_> {
        SeverityInput {
            runtime_type: RuntimeType::Ollama,
            status,
            error_code: None,
            response_time_ms: Some(40),
            consecutive_failures: 0,
            metrics: None,
            waiting_streak: 0,
        }
    }

    fn vllm<'a>(status: &'a str, metrics: &'a RuntimeMetrics) -> SeverityInput<'a> {
        SeverityInput {
            runtime_type: RuntimeType::Vllm,
            status,
            error_code: None,
            response_time_ms: Some(20),
            consecutive_failures: 0,
            metrics: Some(metrics),
            waiting_streak: 0,
        }
    }

    #[test]
    fn a_healthy_ollama_is_normal_and_a_slow_one_is_a_warning() {
        assert_eq!(evaluate(&ollama("online")).level, SeverityLevel::Normal);

        let mut slow = ollama("online");
        slow.response_time_ms = Some(OLLAMA_SLOW_RESPONSE_MS);
        let severity = evaluate(&slow);
        assert_eq!(severity.level, SeverityLevel::Warning);
        assert!(severity.reasons.iter().any(|r| r == "slow_response"));

        // Just under the threshold stays normal.
        let mut brisk = ollama("online");
        brisk.response_time_ms = Some(OLLAMA_SLOW_RESPONSE_MS - 1);
        assert_eq!(evaluate(&brisk).level, SeverityLevel::Normal);
    }

    #[test]
    fn ollama_turns_critical_only_after_three_consecutive_failures() {
        let mut failing = ollama("offline");
        for attempt in 0..CONSECUTIVE_FAILURE_LIMIT {
            failing.consecutive_failures = attempt;
            assert_eq!(
                evaluate(&failing).level,
                SeverityLevel::Warning,
                "{attempt} failures"
            );
        }
        failing.consecutive_failures = CONSECUTIVE_FAILURE_LIMIT;
        let severity = evaluate(&failing);
        assert_eq!(severity.level, SeverityLevel::Critical);
        assert!(severity.reasons.iter().any(|r| r == "repeated_failures"));
    }

    #[test]
    fn a_partly_unreadable_ollama_response_is_a_warning() {
        let severity = evaluate(&ollama("degraded"));
        assert_eq!(severity.level, SeverityLevel::Warning);
        assert!(severity.reasons.iter().any(|r| r == "parse_degraded"));
    }

    #[test]
    fn kv_cache_pressure_climbs_through_the_three_thresholds() {
        for (usage, expected) in [
            (0.10, SeverityLevel::Normal),
            (0.699, SeverityLevel::Normal),
            (0.70, SeverityLevel::Warning),
            (0.84, SeverityLevel::Warning),
            (0.85, SeverityLevel::Congested),
            (0.94, SeverityLevel::Congested),
            (0.95, SeverityLevel::Critical),
            (1.00, SeverityLevel::Critical),
        ] {
            let metrics = RuntimeMetrics {
                kv_cache_usage_ratio: Some(usage),
                ..Default::default()
            };
            assert_eq!(
                evaluate(&vllm("online", &metrics)).level,
                expected,
                "usage {usage}"
            );
        }
    }

    #[test]
    fn a_queue_becomes_congestion_only_once_it_persists() {
        let metrics = RuntimeMetrics {
            requests_waiting: Some(2.0),
            ..Default::default()
        };
        let mut input = vllm("online", &metrics);
        assert_eq!(evaluate(&input).level, SeverityLevel::Warning);

        input.waiting_streak = WAITING_SUSTAINED_TICKS;
        let severity = evaluate(&input);
        assert_eq!(severity.level, SeverityLevel::Congested);
        assert!(severity.reasons.iter().any(|r| r == "waiting_sustained"));
    }

    #[test]
    fn an_ssh_failure_is_named_as_such_rather_than_blamed_on_the_api() {
        let metrics = RuntimeMetrics::default();
        let mut input = vllm("offline", &metrics);
        input.error_code = Some("ssh_tunnel_error");
        let severity = evaluate(&input);
        assert_eq!(severity.level, SeverityLevel::Critical);
        assert!(severity.reasons.iter().any(|r| r == "ssh_unreachable"));

        input.error_code = Some("ssh_host_untrusted");
        assert!(evaluate(&input)
            .reasons
            .iter()
            .any(|r| r == "ssh_unreachable"));

        // Ollama grades on status and failure count only, so the same code must
        // not accidentally couple into its path.
        let mut ollama_input = ollama("offline");
        ollama_input.error_code = Some("ssh_tunnel_error");
        let ollama_severity = evaluate(&ollama_input);
        assert!(ollama_severity.reasons.iter().any(|r| r == "poll_failed"));
        assert_eq!(ollama_severity.level, SeverityLevel::Warning);
    }

    #[test]
    fn a_new_preemption_is_a_warning() {
        let metrics = RuntimeMetrics {
            preemptions_delta: Some(3.0),
            ..Default::default()
        };
        assert_eq!(
            evaluate(&vllm("online", &metrics)).level,
            SeverityLevel::Warning
        );

        // No change is not a warning.
        let quiet = RuntimeMetrics {
            preemptions_delta: Some(0.0),
            ..Default::default()
        };
        assert_eq!(
            evaluate(&vllm("online", &quiet)).level,
            SeverityLevel::Normal
        );
    }

    #[test]
    fn an_unreachable_vllm_is_critical_and_names_the_cause() {
        let metrics = RuntimeMetrics::default();
        let mut input = vllm("error", &metrics);
        input.error_code = Some("engine_dead");
        let severity = evaluate(&input);
        assert_eq!(severity.level, SeverityLevel::Critical);
        assert!(severity.reasons.iter().any(|r| r == "engine_dead"));

        input.error_code = Some("authentication_error");
        assert!(evaluate(&input)
            .reasons
            .iter()
            .any(|r| r == "authentication_error"));

        input.status = "offline";
        input.error_code = Some("timeout");
        assert_eq!(evaluate(&input).level, SeverityLevel::Critical);
    }

    #[test]
    fn missing_metrics_never_invent_a_healthy_reading() {
        // Everything unknown: nothing is wrong that we can see, and nothing is
        // reported as fine that we cannot.
        let empty = RuntimeMetrics::default();
        let severity = evaluate(&vllm("online", &empty));
        assert_eq!(severity.level, SeverityLevel::Normal);
        assert!(severity.reasons.is_empty());

        let mut without_metrics = vllm("online", &empty);
        without_metrics.metrics = None;
        assert_eq!(evaluate(&without_metrics).level, SeverityLevel::Normal);
    }

    #[test]
    fn the_worst_signal_wins_when_several_fire() {
        let metrics = RuntimeMetrics {
            kv_cache_usage_ratio: Some(0.96),
            requests_waiting: Some(5.0),
            preemptions_delta: Some(1.0),
            ..Default::default()
        };
        let severity = evaluate(&vllm("online", &metrics));
        assert_eq!(severity.level, SeverityLevel::Critical);
        // All contributing reasons are kept so the detail view can list them.
        assert!(severity.reasons.len() >= 3, "{:?}", severity.reasons);
    }
}
