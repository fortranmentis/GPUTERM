use super::parsers::now_epoch_seconds;
use super::quota::{sync_agy_history, QuotaProbes};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRateLimitMetric {
    pub(super) label: String,
    pub(super) group: Option<String>,
    pub(super) model_names: Vec<String>,
    pub(super) remaining_percent: Option<f64>,
    pub(super) used_percent: Option<f64>,
    pub(super) window_minutes: Option<u64>,
    pub(super) resets_at: Option<u64>,
    /// The window already rolled over, so `used_percent` describes a window
    /// that no longer exists. The UI reports a reset instead of a balance.
    pub(super) stale: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentQuotaHistoryLimit {
    pub(super) group: Option<String>,
    pub(super) window_minutes: u64,
    pub(super) remaining_percent: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentQuotaHistoryPoint {
    pub(super) captured_at: u64,
    pub(super) status: String,
    pub(super) limits: Vec<AgentQuotaHistoryLimit>,
}

pub type AgentQuotaHistories = Arc<Mutex<HashMap<String, Vec<AgentQuotaHistoryPoint>>>>;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentQuotaSnapshot {
    pub(super) status: String,
    pub(super) source: String,
    pub(super) captured_at: Option<u64>,
    pub(super) snapshot_age_seconds: Option<u64>,
    pub(super) message: Option<String>,
    pub(super) limits: Vec<AgentRateLimitMetric>,
    pub(super) history: Vec<AgentQuotaHistoryPoint>,
}

impl Default for AgentQuotaSnapshot {
    fn default() -> Self {
        Self {
            status: "error".to_string(),
            source: "none".to_string(),
            captured_at: None,
            snapshot_age_seconds: None,
            message: None,
            limits: Vec::new(),
            history: Vec::new(),
        }
    }
}

impl AgentQuotaSnapshot {
    pub(super) fn unavailable(provider: Provider) -> Self {
        let (status, message) = match provider {
            Provider::Claude => (
                "setup-required",
                "Set up the GpuTerm Claude status line to monitor 5-hour and weekly limits.",
            ),
            Provider::Agy => (
                "unsupported",
                "AGY /usage could not be read automatically. Open /usage in AGY to view the current quota.",
            ),
            Provider::Codex => (
                "error",
                "Codex account limits are unavailable. A recent session-log snapshot will be used when possible.",
            ),
        };
        Self {
            status: status.to_string(),
            source: "none".to_string(),
            captured_at: None,
            snapshot_age_seconds: None,
            message: Some(message.to_string()),
            limits: Vec::new(),
            history: Vec::new(),
        }
    }

    pub(super) fn available(
        source: &str,
        captured_at: Option<u64>,
        limits: Vec<AgentRateLimitMetric>,
        now: u64,
    ) -> Self {
        let stale = !limits.is_empty() && limits.iter().all(|limit| limit.stale);
        Self {
            status: if stale { "stale" } else { "available" }.to_string(),
            source: source.to_string(),
            captured_at,
            snapshot_age_seconds: captured_at.map(|value| now.saturating_sub(value)),
            message: stale.then(|| {
                "The reported quota window has reset; waiting for a fresh provider snapshot."
                    .to_string()
            }),
            limits,
            history: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentWorkMetric {
    pub(super) name: String,
    pub(super) role: Option<String>,
    pub(super) status: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMetric {
    pub(super) provider: String,
    pub(super) display_name: String,
    pub(super) status: String,
    pub(super) root_pid: u32,
    pub(super) process_count: u32,
    pub(super) user: Option<String>,
    pub(super) cpu_percent: Option<f64>,
    pub(super) memory_bytes: Option<u64>,
    pub(super) elapsed_seconds: Option<u64>,
    pub(super) session_id: Option<String>,
    pub(super) cwd: Option<String>,
    pub(super) model: Option<String>,
    pub(super) input_tokens: Option<u64>,
    pub(super) output_tokens: Option<u64>,
    pub(super) total_tokens: Option<u64>,
    pub(super) context_used_tokens: Option<u64>,
    pub(super) context_window_tokens: Option<u64>,
    pub(super) context_used_percent: Option<f64>,
    pub(super) context_remaining_tokens: Option<u64>,
    pub(super) context_remaining_percent: Option<f64>,
    pub(super) last_request_input_tokens: Option<u64>,
    pub(super) last_request_output_tokens: Option<u64>,
    pub(super) last_request_cache_creation_tokens: Option<u64>,
    pub(super) last_request_cache_read_tokens: Option<u64>,
    pub(super) cost_usd: Option<f64>,
    pub(super) session_duration_seconds: Option<f64>,
    /// Age of the status-line snapshot these numbers came from. Absent when the
    /// provider's own session records supplied them.
    pub(super) snapshot_age_seconds: Option<u64>,
    pub(super) quota: AgentQuotaSnapshot,
    pub(super) subagents: Vec<AgentWorkMetric>,
    pub(super) background_tasks: Vec<AgentWorkMetric>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Provider {
    Agy,
    Codex,
    Claude,
}

impl Provider {
    pub(super) fn key(self) -> &'static str {
        match self {
            Self::Agy => "agy",
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }

    pub(super) fn display_name(self) -> &'static str {
        match self {
            Self::Agy => "Antigravity",
            Self::Codex => "Codex",
            Self::Claude => "Claude Code",
        }
    }

    pub(super) fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "agy" | "antigravity" => Some(Self::Agy),
            "codex" => Some(Self::Codex),
            "claude" | "claude-code" => Some(Self::Claude),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct ProcessSample {
    pub(super) pid: u32,
    pub(super) ppid: u32,
    pub(super) user: Option<String>,
    pub(super) cpu_percent: Option<f64>,
    pub(super) rss_bytes: Option<u64>,
    pub(super) elapsed_seconds: Option<u64>,
    pub(super) name: String,
    pub(super) command: String,
    pub(super) executable_path: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct AgentSessionMetadata {
    pub(super) session_id: Option<String>,
    pub(super) cwd: Option<String>,
    pub(super) model: Option<String>,
    pub(super) status: Option<String>,
    pub(super) input_tokens: Option<u64>,
    pub(super) output_tokens: Option<u64>,
    pub(super) total_tokens: Option<u64>,
    pub(super) context_used_tokens: Option<u64>,
    pub(super) context_window_tokens: Option<u64>,
    pub(super) context_used_percent: Option<f64>,
    pub(super) context_remaining_tokens: Option<u64>,
    pub(super) context_remaining_percent: Option<f64>,
    pub(super) last_request_input_tokens: Option<u64>,
    pub(super) last_request_output_tokens: Option<u64>,
    pub(super) last_request_cache_creation_tokens: Option<u64>,
    pub(super) last_request_cache_read_tokens: Option<u64>,
    pub(super) cost_usd: Option<f64>,
    pub(super) session_duration_seconds: Option<f64>,
    pub(super) snapshot_age_seconds: Option<u64>,
    pub(super) captured_at: Option<u64>,
    /// Process id the status-line snapshot belongs to, used to attribute usage
    /// to the right session when several run at once.
    pub(super) pid_hint: Option<u32>,
    /// Set for the account-wide quota record rather than a real session.
    pub(super) account_scope: bool,
    /// Set for the setup-state record, which reports which install step is
    /// incomplete rather than describing a session.
    pub(super) setup_scope: bool,
    /// `ok`, `empty`, or `missing`.
    pub(super) setup_helper: Option<String>,
    /// `ours`, `other`, or `none`.
    pub(super) setup_status_line: Option<String>,
    pub(super) rate_limits: Vec<AgentRateLimitMetric>,
    pub(super) subagents: Vec<AgentWorkMetric>,
    pub(super) background_tasks: Vec<AgentWorkMetric>,
}

#[derive(Default)]
pub struct AgentMonitorState {
    pub(super) last_metadata_scan: Option<Instant>,
    /// Why the last metadata scrape failed. Swallowing this is what made a
    /// too-long Windows command look like "no data" for so long.
    pub(super) last_metadata_error: Option<String>,
    pub(super) last_metadata_providers: HashSet<Provider>,
    pub(super) metadata: HashMap<Provider, Vec<AgentSessionMetadata>>,
    pub(super) quotas: HashMap<Provider, AgentQuotaSnapshot>,
    pub(super) last_quota_refresh: HashMap<Provider, Instant>,
    pub(super) forced_quota_refresh: HashSet<Provider>,
    pub(super) agy_history_key: Option<String>,
    pub(super) agy_histories: Option<AgentQuotaHistories>,
    pub(super) agy_history: Vec<AgentQuotaHistoryPoint>,
    pub(super) quota_probes: Arc<QuotaProbes>,
    pub(super) windows_cpu: HashMap<u32, f64>,
    pub(super) windows_cpu_sampled_at: Option<Instant>,
}

impl AgentMonitorState {
    pub fn configure_agy_history(&mut self, key: String, histories: AgentQuotaHistories) {
        self.agy_history_key = Some(key);
        self.agy_histories = Some(histories);
        sync_agy_history(self, now_epoch_seconds());
    }

    pub fn force_quota_refresh(&mut self, provider: &str) {
        if let Some(provider) = Provider::parse(provider) {
            self.forced_quota_refresh.insert(provider);
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeQuotaSetupResult {
    pub(super) status: String,
    pub(super) message: String,
}
