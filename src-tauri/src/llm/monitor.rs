//! Polling coordinator, counter bookkeeping, and the time series.
//!
//! One coordinator thread owns the schedule; each instance is probed on its own
//! short-lived thread so a hung server delays only itself. This mirrors the
//! `QuotaProbes` arrangement in `ssh::agent_monitor`.

use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

use super::adapter::{
    LlmRuntimeAdapter, RuntimeError, RuntimeMetrics, RuntimeModel, RuntimeStatus, RuntimeType,
};
use super::http::{HttpClient, UreqClient};
use super::instance::{vault_key, LlmInstance};
use super::ollama::OllamaAdapter;
use super::severity::{self, Severity, SeverityInput, SeverityLevel};
use super::tunnel::{Endpoint, SshBackend, TunnelManager};
use super::vllm::VllmAdapter;
use crate::ssh::credentials::{CredentialStore, SecureCredentialStore};
use crate::ssh::session::ActiveConnection;

pub const TELEMETRY_EVENT: &str = "llm-runtime-telemetry";

/// Time series retention. Both bounds are applied: the window keeps the chart
/// honest, the count keeps memory bounded if the clock jumps.
pub const HISTORY_WINDOW_SECONDS: u64 = 3_600;
pub const HISTORY_BUCKET_SECONDS: u64 = 10;
pub const HISTORY_MAX_POINTS: usize = 400;
/// Recent state changes shown in the detail view.
pub const MAX_EVENTS: usize = 20;

/// How often the model catalog is refreshed, per runtime.
pub const OLLAMA_CATALOG_INTERVAL_SECS: u64 = 300;
pub const VLLM_CATALOG_INTERVAL_SECS: u64 = 60;

/// Applied after consecutive failures, so a dead host is not hammered.
const BACKOFF_STEPS_SECS: [u64; 4] = [5, 10, 30, 60];

const TICK: Duration = Duration::from_millis(500);

pub fn now_epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Counters
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct CounterReading {
    value: f64,
    at: u64,
}

/// Remembers the previous reading of each cumulative counter so a rate can be
/// derived from two scrapes.
#[derive(Debug, Default)]
pub struct CounterState {
    last: HashMap<String, CounterReading>,
    reset_seen: bool,
}

#[derive(Debug, Default, PartialEq)]
pub struct CounterOutcome {
    pub per_second: Option<f64>,
    pub delta: Option<f64>,
    /// The counter went backwards, i.e. the server restarted.
    pub reset: bool,
}

impl CounterState {
    /// Records a counter reading and reports the change since the last one.
    ///
    /// A counter that decreased yields `None` rather than a negative rate, and
    /// the new value becomes the baseline so the next interval is usable again.
    pub fn observe(&mut self, name: &str, value: f64, now: u64) -> CounterOutcome {
        if !value.is_finite() {
            return CounterOutcome::default();
        }

        let previous = self.last.get(name).copied();
        let Some(previous) = previous else {
            self.last.insert(name.to_string(), CounterReading { value, at: now });
            return CounterOutcome::default();
        };

        if value < previous.value {
            self.last
                .insert(name.to_string(), CounterReading { value, at: now });
            self.reset_seen = true;
            return CounterOutcome {
                reset: true,
                ..Default::default()
            };
        }

        let elapsed = now.saturating_sub(previous.at);
        if elapsed == 0 {
            // Two scrapes inside the same second. Keep the older baseline so the
            // next interval measures a real span instead of dividing by zero.
            return CounterOutcome::default();
        }

        self.last
            .insert(name.to_string(), CounterReading { value, at: now });
        let delta = value - previous.value;
        CounterOutcome {
            per_second: Some(delta / elapsed as f64),
            delta: Some(delta),
            reset: false,
        }
    }

    /// Whether any counter went backwards since this was last called.
    pub fn take_reset(&mut self) -> bool {
        std::mem::take(&mut self.reset_seen)
    }
}

// ---------------------------------------------------------------------------
// Serialized shapes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LlmHistoryPoint {
    pub captured_at: u64,
    pub response_time_ms: Option<u64>,
    pub requests_running: Option<f64>,
    pub requests_waiting: Option<f64>,
    pub kv_cache_usage_ratio: Option<f64>,
    pub prompt_tokens_per_second: Option<f64>,
    pub generation_tokens_per_second: Option<f64>,
    pub requests_per_second: Option<f64>,
    pub preemptions_delta: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmEvent {
    pub at: u64,
    /// `status_changed` | `counters_reset` | `error`
    pub kind: String,
    /// A stable code, not a sentence: the UI supplies the wording.
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmErrorInfo {
    pub at: u64,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmInstanceTelemetry {
    pub instance: LlmInstance,
    /// Whether an API key is stored for this instance. The key itself never
    /// crosses this boundary.
    pub has_api_key: bool,
    pub status: Option<RuntimeStatus>,
    pub severity: String,
    pub severity_reasons: Vec<String>,
    pub models: Vec<RuntimeModel>,
    pub running_model_count: usize,
    pub metrics: Option<RuntimeMetrics>,
    pub history: Vec<LlmHistoryPoint>,
    pub events: Vec<LlmEvent>,
    pub last_success_at: Option<u64>,
    pub last_error: Option<LlmErrorInfo>,
    pub consecutive_failures: u32,
    /// Name of the SSH profile the poll is tunneled through, so a bare
    /// `127.0.0.1` is never ambiguous in the UI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssh_profile_name: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmSummary {
    pub registered: usize,
    pub enabled: usize,
    pub normal: usize,
    pub warning: usize,
    pub error: usize,
    pub unknown: usize,
    pub models: usize,
    /// vLLM only; `None` when no vLLM instance reported a value.
    pub vllm_requests_running: Option<f64>,
    pub vllm_requests_waiting: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmTelemetryPayload {
    pub generated_at: u64,
    pub summary: LlmSummary,
    pub instances: Vec<LlmInstanceTelemetry>,
}

// ---------------------------------------------------------------------------
// Handle shared with the command layer
// ---------------------------------------------------------------------------

/// The coordinator's public face: the latest payload, plus a way to ask for an
/// immediate re-poll after the instance list changes.
#[derive(Default)]
pub struct MonitorHandle {
    latest: Mutex<Option<LlmTelemetryPayload>>,
    refresh_requested: AtomicBool,
    stopped: AtomicBool,
}

impl MonitorHandle {
    pub fn snapshot(&self) -> Option<LlmTelemetryPayload> {
        self.latest.lock().ok().and_then(|latest| latest.clone())
    }

    /// Ends the polling loop. Called when the app exits.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
    }

    fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    /// Asks the coordinator to poll every enabled instance on its next tick.
    pub fn request_refresh(&self) {
        self.refresh_requested.store(true, Ordering::SeqCst);
    }

    fn take_refresh(&self) -> bool {
        self.refresh_requested.swap(false, Ordering::SeqCst)
    }

    fn publish(&self, payload: LlmTelemetryPayload) {
        if let Ok(mut latest) = self.latest.lock() {
            *latest = Some(payload);
        }
    }
}

// ---------------------------------------------------------------------------
// Probing
// ---------------------------------------------------------------------------

pub fn adapter_for(instance: &LlmInstance, now: u64) -> Box<dyn LlmRuntimeAdapter> {
    match instance.runtime_type {
        RuntimeType::Ollama => Box::new(OllamaAdapter::new(&instance.id, now)),
        RuntimeType::Vllm => Box::new(VllmAdapter::new(&instance.id, now)),
    }
}

pub struct ProbeRequest {
    pub instance: LlmInstance,
    /// Where to actually send the GET: the instance's own address, or the
    /// loopback port a tunnel forwards to it.
    pub endpoint: String,
    pub include_catalog: bool,
    pub cached_models: Vec<RuntimeModel>,
    pub now: u64,
}

pub struct ProbeOutcome {
    pub instance_id: String,
    pub status: RuntimeStatus,
    /// `None` means "nothing new, keep what is cached".
    pub models: Option<Vec<RuntimeModel>>,
    pub catalog_refreshed: bool,
    pub metrics: Option<RuntimeMetrics>,
    pub counters_reset: bool,
    /// A sub-request that failed while the health check succeeded.
    pub partial_error: Option<RuntimeError>,
    pub counters: CounterState,
    pub now: u64,
}

impl ProbeOutcome {
    /// An outcome for a probe that could not even be attempted.
    pub fn failed(
        instance: &LlmInstance,
        error: &RuntimeError,
        counters: CounterState,
        now: u64,
    ) -> Self {
        Self {
            instance_id: instance.id.clone(),
            status: RuntimeStatus::failed(
                &instance.id,
                instance.runtime_type,
                None,
                now,
                error,
            ),
            models: None,
            catalog_refreshed: false,
            metrics: None,
            counters_reset: false,
            partial_error: Some(error.clone()),
            counters,
            now,
        }
    }
}

/// Runs one full probe cycle against an already-built client.
///
/// Split out from the threading so it can be tested with `FakeHttpClient`.
pub fn run_probe(
    request: ProbeRequest,
    client: &dyn HttpClient,
    mut counters: CounterState,
) -> ProbeOutcome {
    let adapter = adapter_for(&request.instance, request.now);
    let mut status = adapter.check_health(client);

    let mut outcome = ProbeOutcome {
        instance_id: request.instance.id.clone(),
        status: status.clone(),
        models: None,
        catalog_refreshed: false,
        metrics: None,
        counters_reset: false,
        partial_error: None,
        counters: CounterState::default(),
        now: request.now,
    };

    // An unreachable or erroring host has nothing more to tell us, and asking
    // again would just add two more timeouts to the cycle.
    if status.status == "offline" || status.status == "error" {
        outcome.counters = counters;
        return outcome;
    }

    let mut partial_error: Option<RuntimeError> = None;

    if request.include_catalog {
        match adapter.get_models(client) {
            Ok(models) => {
                outcome.models = Some(models);
                outcome.catalog_refreshed = true;
            }
            Err(error) => partial_error = Some(error),
        }
    } else if let Some(live) = adapter.get_live_models(client) {
        match live {
            Ok(models) => outcome.models = Some(merge_live_with_cache(models, &request.cached_models)),
            Err(error) => partial_error = Some(error),
        }
    }

    match adapter.get_runtime_metrics(client, &mut counters, request.now) {
        Ok(metrics) => outcome.metrics = metrics,
        Err(error) => partial_error = partial_error.or(Some(error)),
    }
    outcome.counters_reset = counters.take_reset();

    // The server answered but part of what it said could not be read: that is
    // exactly what `degraded` means.
    if let Some(error) = &partial_error {
        if status.status == "online" {
            status = RuntimeStatus::degraded(
                &request.instance.id,
                request.instance.runtime_type,
                status.response_time_ms.unwrap_or(0),
                request.now,
                error,
            );
        }
    }

    outcome.status = status;
    outcome.partial_error = partial_error;
    outcome.counters = counters;
    outcome
}

/// Combines a fresh live listing with the last full catalog.
///
/// A cached model missing from the live list is kept, but demoted: it is still
/// installed, it is simply no longer loaded, so its residency figures are
/// dropped rather than shown stale.
pub fn merge_live_with_cache(
    live: Vec<RuntimeModel>,
    cached: &[RuntimeModel],
) -> Vec<RuntimeModel> {
    let mut models = live;
    for entry in cached {
        let already_present = models
            .iter()
            .any(|existing| existing.id == entry.id || existing.name == entry.name);
        if already_present {
            continue;
        }
        let mut demoted = entry.clone();
        if demoted.status == "running" {
            demoted.status = "installed".to_string();
            demoted.vram_size_bytes = None;
            demoted.vram_resident_percent = None;
            demoted.non_vram_bytes = None;
            demoted.expires_at = None;
            demoted.expires_in_seconds = None;
        }
        models.push(demoted);
    }
    models.sort_by(|left, right| {
        let rank = |status: &str| if status == "running" { 0 } else { 1 };
        rank(&left.status)
            .cmp(&rank(&right.status))
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    models
}

// ---------------------------------------------------------------------------
// Time series
// ---------------------------------------------------------------------------

/// Appends a reading, collapsing everything inside one bucket into a single
/// point so a fast poll interval cannot flood the chart.
pub fn push_history(
    history: &mut Vec<LlmHistoryPoint>,
    status: &RuntimeStatus,
    metrics: Option<&RuntimeMetrics>,
    now: u64,
) {
    let bucket = now - (now % HISTORY_BUCKET_SECONDS);
    let point = LlmHistoryPoint {
        captured_at: bucket,
        response_time_ms: status.response_time_ms,
        requests_running: metrics.and_then(|m| m.requests_running),
        requests_waiting: metrics.and_then(|m| m.requests_waiting),
        kv_cache_usage_ratio: metrics.and_then(|m| m.kv_cache_usage_ratio),
        prompt_tokens_per_second: metrics.and_then(|m| m.prompt_tokens_per_second),
        generation_tokens_per_second: metrics.and_then(|m| m.generation_tokens_per_second),
        requests_per_second: metrics.and_then(|m| m.requests_per_second),
        preemptions_delta: metrics.and_then(|m| m.preemptions_delta),
    };

    match history.last_mut() {
        Some(last) if last.captured_at == bucket => *last = point,
        _ => history.push(point),
    }
    trim_history(history, now);
}

fn trim_history(history: &mut Vec<LlmHistoryPoint>, now: u64) {
    let cutoff = now.saturating_sub(HISTORY_WINDOW_SECONDS);
    history.retain(|point| point.captured_at >= cutoff);
    if history.len() > HISTORY_MAX_POINTS {
        history.drain(..history.len() - HISTORY_MAX_POINTS);
    }
}

/// Seconds to wait before the next poll, given how many attempts have failed.
pub fn next_delay_secs(poll_interval_secs: u64, consecutive_failures: u32) -> u64 {
    if consecutive_failures == 0 {
        return poll_interval_secs.max(1);
    }
    let step = BACKOFF_STEPS_SECS
        [((consecutive_failures - 1) as usize).min(BACKOFF_STEPS_SECS.len() - 1)];
    // Never poll faster than the user asked for, even early in the backoff.
    step.max(poll_interval_secs.max(1))
}

/// How long a probe may run before the coordinator assumes its thread is lost.
///
/// A cycle makes at most three requests, so a generous multiple of the timeout
/// plus a fixed margin cannot fire against a merely slow server.
fn stall_limit(instance: &LlmInstance) -> u64 {
    (instance.request_timeout_ms.div_ceil(1_000) * 4) + 30
}

fn catalog_interval_secs(runtime_type: RuntimeType) -> u64 {
    match runtime_type {
        RuntimeType::Ollama => OLLAMA_CATALOG_INTERVAL_SECS,
        RuntimeType::Vllm => VLLM_CATALOG_INTERVAL_SECS,
    }
}

// ---------------------------------------------------------------------------
// Per-instance state held by the coordinator
// ---------------------------------------------------------------------------

struct InstanceState {
    /// Taken by the probe thread while it runs and returned with the outcome,
    /// which makes `None` the single, race-free "probe in flight" marker.
    counters: Option<CounterState>,
    /// When the in-flight probe started, so a thread that never reports back
    /// cannot silence an instance forever.
    probe_started_at: Option<u64>,
    models: Vec<RuntimeModel>,
    status: Option<RuntimeStatus>,
    metrics: Option<RuntimeMetrics>,
    history: Vec<LlmHistoryPoint>,
    events: VecDeque<LlmEvent>,
    last_success_at: Option<u64>,
    last_error: Option<LlmErrorInfo>,
    consecutive_failures: u32,
    waiting_streak: u32,
    next_poll_at: u64,
    next_catalog_at: u64,
    severity: Option<Severity>,
}

impl Default for InstanceState {
    fn default() -> Self {
        Self {
            counters: Some(CounterState::default()),
            probe_started_at: None,
            models: Vec::new(),
            status: None,
            metrics: None,
            history: Vec::new(),
            events: VecDeque::new(),
            last_success_at: None,
            last_error: None,
            consecutive_failures: 0,
            waiting_streak: 0,
            next_poll_at: 0,
            next_catalog_at: 0,
            severity: None,
        }
    }
}

impl InstanceState {
    fn record_event(&mut self, at: u64, kind: &str, code: &str, detail: Option<String>) {
        self.events.push_back(LlmEvent {
            at,
            kind: kind.to_string(),
            code: code.to_string(),
            detail,
        });
        while self.events.len() > MAX_EVENTS {
            self.events.pop_front();
        }
    }
}

fn apply_outcome(state: &mut InstanceState, instance: &LlmInstance, outcome: ProbeOutcome) {
    let now = outcome.now;
    state.counters = Some(outcome.counters);
    state.probe_started_at = None;

    let previous_status = state.status.as_ref().map(|status| status.status.clone());
    let succeeded = outcome.status.status == "online" || outcome.status.status == "degraded";

    if succeeded {
        state.consecutive_failures = 0;
        state.last_success_at = Some(now);
    } else {
        state.consecutive_failures = state.consecutive_failures.saturating_add(1);
    }

    if let (Some(code), Some(message)) = (
        outcome.status.error_code.clone(),
        outcome.status.error_message.clone(),
    ) {
        state.last_error = Some(LlmErrorInfo {
            at: now,
            code: code.clone(),
            message,
        });
        state.record_event(now, "error", &code, None);
    }

    if previous_status.as_deref() != Some(outcome.status.status.as_str()) {
        state.record_event(
            now,
            "status_changed",
            &outcome.status.status,
            previous_status,
        );
    }

    if outcome.counters_reset {
        // The counters restarting is the only evidence we get that the server
        // was restarted, and it explains the gap in the throughput chart.
        state.record_event(now, "counters_reset", "counters_reset", None);
    }

    if let Some(models) = outcome.models {
        state.models = models;
    }
    if outcome.catalog_refreshed {
        state.next_catalog_at = now + catalog_interval_secs(instance.runtime_type);
    }
    if succeeded {
        state.metrics = outcome.metrics;
    }

    let waiting = state
        .metrics
        .as_ref()
        .and_then(|metrics| metrics.requests_waiting);
    state.waiting_streak = match waiting {
        Some(count) if count >= 1.0 => state.waiting_streak.saturating_add(1),
        _ => 0,
    };

    state.severity = Some(severity::evaluate(&SeverityInput {
        runtime_type: instance.runtime_type,
        status: &outcome.status.status,
        error_code: outcome.status.error_code.as_deref(),
        response_time_ms: outcome.status.response_time_ms,
        consecutive_failures: state.consecutive_failures,
        metrics: state.metrics.as_ref(),
        waiting_streak: state.waiting_streak,
    }));

    push_history(&mut state.history, &outcome.status, state.metrics.as_ref(), now);
    state.status = Some(outcome.status);
    state.next_poll_at = now + next_delay_secs(instance.poll_interval_secs, state.consecutive_failures);
}

/// `has_key` reports whether an instance has a stored API key and
/// `profile_name` names its SSH tunnel profile. Both are callbacks rather than
/// the vault and profile store themselves, so this stays a pure function under
/// test — the tests must not reach into real files on the machine.
fn build_payload(
    instances: &[LlmInstance],
    states: &HashMap<String, InstanceState>,
    has_key: &dyn Fn(&str) -> bool,
    profile_name: &dyn Fn(&str) -> Option<String>,
    now: u64,
) -> LlmTelemetryPayload {
    let mut summary = LlmSummary {
        registered: instances.len(),
        ..Default::default()
    };
    let mut vllm_running: Option<f64> = None;
    let mut vllm_waiting: Option<f64> = None;
    let mut entries = Vec::with_capacity(instances.len());

    for instance in instances {
        let state = states.get(&instance.id);
        let severity = state
            .and_then(|state| state.severity.clone())
            .unwrap_or(Severity {
                level: SeverityLevel::Unknown,
                reasons: Vec::new(),
            });

        if instance.enabled {
            summary.enabled += 1;
            match severity.level {
                SeverityLevel::Normal => summary.normal += 1,
                SeverityLevel::Warning | SeverityLevel::Congested => summary.warning += 1,
                SeverityLevel::Critical => summary.error += 1,
                SeverityLevel::Unknown => summary.unknown += 1,
            }
        }

        let models = state.map(|state| state.models.clone()).unwrap_or_default();
        let running_model_count = models
            .iter()
            .filter(|model| model.status == "running" || model.status == "served")
            .count();
        summary.models += running_model_count;

        let metrics = state.and_then(|state| state.metrics.clone());
        if instance.runtime_type == RuntimeType::Vllm {
            if let Some(metrics) = &metrics {
                if let Some(running) = metrics.requests_running {
                    vllm_running = Some(vllm_running.unwrap_or(0.0) + running);
                }
                if let Some(waiting) = metrics.requests_waiting {
                    vllm_waiting = Some(vllm_waiting.unwrap_or(0.0) + waiting);
                }
            }
        }

        entries.push(LlmInstanceTelemetry {
            instance: instance.clone(),
            has_api_key: has_key(&instance.id),
            status: state.and_then(|state| state.status.clone()),
            severity: severity.level.key().to_string(),
            severity_reasons: severity.reasons,
            models,
            running_model_count,
            metrics,
            history: state.map(|state| state.history.clone()).unwrap_or_default(),
            events: state
                .map(|state| state.events.iter().cloned().collect())
                .unwrap_or_default(),
            last_success_at: state.and_then(|state| state.last_success_at),
            last_error: state.and_then(|state| state.last_error.clone()),
            ssh_profile_name: instance
                .ssh_profile_id
                .as_deref()
                .and_then(profile_name),
            consecutive_failures: state.map(|state| state.consecutive_failures).unwrap_or(0),
        });
    }

    summary.vllm_requests_running = vllm_running;
    summary.vllm_requests_waiting = vllm_waiting;

    LlmTelemetryPayload {
        generated_at: now,
        summary,
        instances: entries,
    }
}

// ---------------------------------------------------------------------------
// Coordinator
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Probes {
    finished: Mutex<Vec<ProbeOutcome>>,
}

/// Reads the stored API key for an instance, if it has one.
///
/// A locked vault holding a key is reported rather than quietly sending an
/// unauthenticated request, which the server would answer with a 401 that
/// blames the key instead of the lock.
pub fn credential_for(
    instance: &LlmInstance,
    credentials: &SecureCredentialStore,
) -> Result<Option<String>, RuntimeError> {
    let key = vault_key(&instance.id);
    match credentials.get_password(&key) {
        Ok(secret) => Ok(secret),
        Err(_) if credentials.has_saved_credential(&key) => Err(RuntimeError::new(
            crate::llm::adapter::RuntimeErrorCode::AuthenticationError,
            "An API key is stored for this instance but the credential vault is locked. Unlock it to resume monitoring.",
        )),
        // No key was ever stored, so there is nothing the lock is withholding.
        Err(_) => Ok(None),
    }
}

/// Builds the client for one instance.
///
/// The key exists only inside this function's caller chain: it is never stored
/// on `LlmInstance`, never serialized, and never logged.
pub fn client_for(
    instance: &LlmInstance,
    endpoint: &str,
    api_key: Option<String>,
) -> Box<dyn HttpClient + Send> {
    Box::new(UreqClient::new(
        endpoint,
        api_key,
        Duration::from_millis(instance.request_timeout_ms),
    ))
}

/// Starts the polling coordinator. Returns immediately; the thread exits once
/// `MonitorHandle::stop` is called.
pub fn start(app: AppHandle, context: MonitorContext) {
    thread::spawn(move || run_coordinator(app, context));
}

/// The pieces of `AppState` the coordinator needs, cloned so it does not hold a
/// `State<'_>` borrow across threads.
#[derive(Clone)]
pub struct MonitorContext {
    pub instances: Arc<Mutex<Vec<LlmInstance>>>,
    pub credentials: SecureCredentialStore,
    pub handle: Arc<MonitorHandle>,
    /// Lets the tunnel resolver prefer a profile that is already connected in a
    /// terminal, reusing its resolved credentials and proxy chain.
    pub active_connections: Arc<Mutex<HashMap<String, ActiveConnection>>>,
}

fn run_coordinator(app: AppHandle, context: MonitorContext) {
    let probes = Arc::new(Probes::default());
    let mut states: HashMap<String, InstanceState> = HashMap::new();
    let mut last_signature = String::new();
    let mut tunnels = TunnelManager::new(SshBackend::new(
        Arc::clone(&context.active_connections),
        context.credentials.clone(),
    ));
    let mut profile_names: HashMap<String, String> = HashMap::new();

    while !context.handle.is_stopped() {
        let now = now_epoch_seconds();
        let mut changed = false;

        // 1. Fold in whatever finished since the last tick.
        let finished: Vec<ProbeOutcome> = probes
            .finished
            .lock()
            .map(|mut queue| queue.drain(..).collect())
            .unwrap_or_default();
        let instances: Vec<LlmInstance> = context
            .instances
            .lock()
            .map(|instances| instances.clone())
            .unwrap_or_default();

        for outcome in finished {
            let Some(instance) = instances
                .iter()
                .find(|instance| instance.id == outcome.instance_id)
            else {
                // Deleted while its probe was running.
                continue;
            };
            let state = states.entry(instance.id.clone()).or_default();
            apply_outcome(state, instance, outcome);
            changed = true;
        }

        // 2. Forget instances that were deleted or switched off.
        let live: HashSet<&str> = instances
            .iter()
            .filter(|instance| instance.enabled)
            .map(|instance| instance.id.as_str())
            .collect();
        let stale: Vec<String> = states
            .keys()
            .filter(|id| !live.contains(id.as_str()))
            .cloned()
            .collect();
        for id in stale {
            states.remove(&id);
            changed = true;
        }
        // `live` is already "enabled and still registered", so this covers a
        // disable and a delete in one call.
        let live_owned: HashSet<String> = live.iter().map(|id| id.to_string()).collect();
        tunnels.retain(&live_owned);

        // 3. Start probes that are due.
        let forced = context.handle.take_refresh();
        for instance in instances.iter().filter(|instance| instance.enabled) {
            let state = states.entry(instance.id.clone()).or_default();
            if !forced && now < state.next_poll_at {
                continue;
            }
            let counters = match state.counters.take() {
                Some(counters) => counters,
                None => {
                    // A probe still owns the counter state, so one is in flight
                    // and this instance must not be asked twice. If that thread
                    // never reported back, recover rather than going silent.
                    let stalled = state
                        .probe_started_at
                        .is_some_and(|started| now.saturating_sub(started) > stall_limit(instance));
                    if !stalled {
                        continue;
                    }
                    CounterState::default()
                }
            };

            // Resolved before the counters are committed to a probe, so a
            // still-connecting tunnel does not disturb the in-flight guard.
            let endpoint = match tunnels.endpoint_for(instance, now) {
                Endpoint::Ready(endpoint) => endpoint,
                Endpoint::Pending => {
                    // The SSH hop is still being established. Nothing has gone
                    // wrong, so this is not recorded as a failure.
                    state.counters = Some(counters);
                    state.probe_started_at = None;
                    state.next_poll_at = now + 1;
                    continue;
                }
                Endpoint::Failed(error) => {
                    // Reported through the same path a probe failure takes, so
                    // backoff, events, history, and severity all apply.
                    let outcome = ProbeOutcome::failed(instance, &error, counters, now);
                    if let Ok(mut finished) = probes.finished.lock() {
                        finished.push(outcome);
                    }
                    state.probe_started_at = None;
                    state.next_poll_at = now + instance.poll_interval_secs.max(1);
                    continue;
                }
            };
            state.probe_started_at = Some(now);

            let request = ProbeRequest {
                instance: instance.clone(),
                endpoint,
                include_catalog: now >= state.next_catalog_at,
                cached_models: state.models.clone(),
                now,
            };
            spawn_probe(&probes, &context.credentials, request, counters);
            // Push the next attempt out so a slow probe is not re-queued the
            // instant it returns; the real schedule is set by `apply_outcome`.
            state.next_poll_at = now + instance.poll_interval_secs.max(1);
        }

        // 4. Publish when anything moved, or when the registration list changed.
        let signature = instances
            .iter()
            .map(|instance| format!("{}:{}", instance.id, instance.updated_at))
            .collect::<Vec<_>>()
            .join(",");
        if signature != last_signature {
            last_signature = signature;
            changed = true;
        }
        if changed {
            refresh_profile_names(&mut profile_names, &instances);
            let credentials = &context.credentials;
            let payload = build_payload(
                &instances,
                &states,
                &|id| credentials.has_saved_credential(&vault_key(id)),
                &|id| profile_names.get(id).cloned(),
                now,
            );
            context.handle.publish(payload.clone());
            let _ = app.emit(TELEMETRY_EVENT, payload);
        }

        thread::sleep(TICK);
    }

    // Explicit rather than relying on drop, so every stop flag is set before
    // this function returns. The pumps are not joined: `RunEvent::Exit` must not
    // be delayed, and the OS closes the sockets.
    tunnels.shutdown();
}

/// Caches SSH profile names for the payload, re-reading the profile file only
/// when a tunneled instance names one that is not cached yet.
fn refresh_profile_names(cache: &mut HashMap<String, String>, instances: &[LlmInstance]) {
    let missing = instances.iter().any(|instance| {
        instance
            .ssh_profile_id
            .as_deref()
            .is_some_and(|id| !cache.contains_key(id))
    });
    if !missing {
        return;
    }
    if let Ok(profiles) = crate::ssh::session::list_profiles() {
        for profile in profiles {
            cache.insert(profile.id, profile.name);
        }
    }
}

/// Runs one probe on its own thread so a hung server delays only itself.
fn spawn_probe(
    probes: &Arc<Probes>,
    credentials: &SecureCredentialStore,
    request: ProbeRequest,
    counters: CounterState,
) {
    let probes = Arc::clone(probes);
    let credentials = credentials.clone();
    thread::spawn(move || {
        let outcome = match credential_for(&request.instance, &credentials) {
            Ok(api_key) => {
                let client = client_for(&request.instance, &request.endpoint, api_key);
                run_probe(request, client.as_ref(), counters)
            }
            Err(error) => ProbeOutcome::failed(&request.instance, &error, counters, request.now),
        };
        if let Ok(mut finished) = probes.finished.lock() {
            finished.push(outcome);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::adapter::RuntimeErrorCode;
    use crate::llm::http::testing::FakeHttpClient;
    use crate::llm::instance::{DEFAULT_POLL_INTERVAL_SECS, DEFAULT_REQUEST_TIMEOUT_MS};
    use crate::llm::ollama::{PS_PATH, TAGS_PATH};

    const TEST_NOW: u64 = 1_799_000_000;

    fn instance(runtime_type: RuntimeType) -> LlmInstance {
        LlmInstance {
            id: "inst-1".to_string(),
            name: "test".to_string(),
            runtime_type,
            base_url: "http://host:11434".to_string(),
            enabled: true,
            request_timeout_ms: DEFAULT_REQUEST_TIMEOUT_MS,
            poll_interval_secs: DEFAULT_POLL_INTERVAL_SECS,
            created_at: TEST_NOW,
            updated_at: TEST_NOW,
            ssh_profile_id: None,
        }
    }

    fn status(kind: &str, response_time_ms: Option<u64>) -> RuntimeStatus {
        RuntimeStatus {
            instance_id: "inst-1".to_string(),
            runtime_type: "vllm".to_string(),
            status: kind.to_string(),
            response_time_ms,
            checked_at: TEST_NOW,
            error_code: None,
            error_message: None,
        }
    }

    #[test]
    fn a_rising_counter_yields_a_rate_and_the_first_reading_yields_none() {
        let mut state = CounterState::default();
        assert_eq!(state.observe("c", 100.0, TEST_NOW), CounterOutcome::default());

        let outcome = state.observe("c", 160.0, TEST_NOW + 10);
        assert_eq!(outcome.per_second, Some(6.0));
        assert_eq!(outcome.delta, Some(60.0));
        assert!(!outcome.reset);
    }

    #[test]
    fn a_falling_counter_is_a_reset_and_rebaselines() {
        let mut state = CounterState::default();
        state.observe("c", 100.0, TEST_NOW);

        let outcome = state.observe("c", 4.0, TEST_NOW + 10);
        assert!(outcome.reset);
        assert_eq!(outcome.per_second, None, "never a negative rate");
        assert!(state.take_reset());
        assert!(!state.take_reset(), "the flag is consumed once");

        // The restarted counter is the new baseline.
        let outcome = state.observe("c", 24.0, TEST_NOW + 20);
        assert_eq!(outcome.per_second, Some(2.0));
    }

    #[test]
    fn two_scrapes_in_one_second_keep_the_older_baseline() {
        let mut state = CounterState::default();
        state.observe("c", 100.0, TEST_NOW);
        // Same second: no division by zero, and no data thrown away.
        assert_eq!(
            state.observe("c", 130.0, TEST_NOW),
            CounterOutcome::default()
        );
        // Measured against the original reading, not the discarded one.
        let outcome = state.observe("c", 200.0, TEST_NOW + 10);
        assert_eq!(outcome.per_second, Some(10.0));
    }

    #[test]
    fn a_non_finite_counter_is_ignored_rather_than_poisoning_the_baseline() {
        let mut state = CounterState::default();
        state.observe("c", 100.0, TEST_NOW);
        assert_eq!(
            state.observe("c", f64::NAN, TEST_NOW + 5),
            CounterOutcome::default()
        );
        let outcome = state.observe("c", 150.0, TEST_NOW + 10);
        assert_eq!(outcome.per_second, Some(5.0));
    }

    #[test]
    fn history_collapses_a_bucket_and_is_trimmed_by_time_and_count() {
        let mut history = Vec::new();
        let metrics = RuntimeMetrics {
            requests_running: Some(1.0),
            ..Default::default()
        };
        // Three readings inside one 10 s bucket produce one point.
        for offset in 0..3 {
            push_history(
                &mut history,
                &status("online", Some(12)),
                Some(&metrics),
                TEST_NOW + offset,
            );
        }
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].captured_at % HISTORY_BUCKET_SECONDS, 0);
        assert_eq!(history[0].requests_running, Some(1.0));

        push_history(
            &mut history,
            &status("online", Some(12)),
            Some(&metrics),
            TEST_NOW + HISTORY_BUCKET_SECONDS,
        );
        assert_eq!(history.len(), 2);

        // Everything older than the window is dropped: the first bucket falls
        // outside it, the second is exactly on the cutoff and survives.
        push_history(
            &mut history,
            &status("online", Some(12)),
            Some(&metrics),
            TEST_NOW + HISTORY_WINDOW_SECONDS + HISTORY_BUCKET_SECONDS,
        );
        assert_eq!(history.len(), 2);
        assert!(history.iter().all(|point| point.captured_at >= TEST_NOW + HISTORY_BUCKET_SECONDS));
    }

    #[test]
    fn history_never_grows_past_the_point_cap() {
        let mut history = Vec::new();
        // A clock that never advances past the window still cannot grow forever.
        for index in 0..(HISTORY_MAX_POINTS + 50) as u64 {
            push_history(
                &mut history,
                &status("online", Some(1)),
                None,
                TEST_NOW + index * HISTORY_BUCKET_SECONDS,
            );
        }
        assert!(history.len() <= HISTORY_MAX_POINTS, "{}", history.len());
    }

    #[test]
    fn missing_metrics_are_stored_as_null_not_zero() {
        let mut history = Vec::new();
        push_history(&mut history, &status("online", None), None, TEST_NOW);
        assert_eq!(history[0].requests_running, None);
        assert_eq!(history[0].kv_cache_usage_ratio, None);
        assert_eq!(history[0].response_time_ms, None);
    }

    #[test]
    fn backoff_climbs_on_failure_and_snaps_back_on_success() {
        assert_eq!(next_delay_secs(5, 0), 5);
        assert_eq!(next_delay_secs(5, 1), 5);
        assert_eq!(next_delay_secs(5, 2), 10);
        assert_eq!(next_delay_secs(5, 3), 30);
        assert_eq!(next_delay_secs(5, 4), 60);
        assert_eq!(next_delay_secs(5, 99), 60, "capped at the last step");
        // A long user interval is respected even while backing off.
        assert_eq!(next_delay_secs(120, 1), 120);
    }

    #[test]
    fn a_healthy_ollama_probe_collects_models_without_touching_generation() {
        let ps = r#"{"models":[{"name":"llama3:8b","model":"llama3:8b","size":1000,"size_vram":800}]}"#;
        let tags = r#"{"models":[{"name":"llama3:8b","model":"llama3:8b","size":1000},
                                 {"name":"qwen:7b","model":"qwen:7b","size":2000}]}"#;
        let client = FakeHttpClient::new()
            .with_body(PS_PATH, 200, ps)
            .with_body(TAGS_PATH, 200, tags);

        let outcome = run_probe(
            ProbeRequest {
                instance: instance(RuntimeType::Ollama),
                endpoint: "http://host:11434".to_string(),
                include_catalog: true,
                cached_models: Vec::new(),
                now: TEST_NOW,
            },
            &client,
            CounterState::default(),
        );

        assert_eq!(outcome.status.status, "online");
        let models = outcome.models.unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].status, "running");
        assert!(outcome.catalog_refreshed);
        // Ollama exposes no serving metrics on these endpoints.
        assert!(outcome.metrics.is_none());

        let requested = client.requested.lock().unwrap().clone();
        assert!(
            requested
                .iter()
                .all(|path| path == PS_PATH || path == TAGS_PATH),
            "{requested:?}"
        );
        // `/api/ps` serves both the health check and the running-model list, so
        // a full cycle asks for it once, not twice.
        assert_eq!(
            requested.iter().filter(|path| *path == PS_PATH).count(),
            1,
            "{requested:?}"
        );
    }

    #[test]
    fn an_incremental_ollama_cycle_asks_for_ps_once_and_skips_the_catalog() {
        let ps = r#"{"models":[{"name":"llama3:8b","model":"llama3:8b","size":1000,"size_vram":800}]}"#;
        let client = FakeHttpClient::new().with_body(PS_PATH, 200, ps);

        let cached = vec![RuntimeModel::new("qwen:7b", "qwen:7b", "installed")];
        let outcome = run_probe(
            ProbeRequest {
                instance: instance(RuntimeType::Ollama),
                endpoint: "http://host:11434".to_string(),
                include_catalog: false,
                cached_models: cached,
                now: TEST_NOW,
            },
            &client,
            CounterState::default(),
        );

        let requested = client.requested.lock().unwrap().clone();
        assert_eq!(requested, vec![PS_PATH.to_string()]);
        assert!(!outcome.catalog_refreshed);
        // The installed model from the last catalog fetch is still listed.
        let models = outcome.models.unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].status, "running");
        assert_eq!(models[1].name, "qwen:7b");
    }

    #[test]
    fn an_unreachable_host_short_circuits_instead_of_timing_out_three_times() {
        let client = FakeHttpClient::new().with_error(
            PS_PATH,
            RuntimeError::new(RuntimeErrorCode::ConnectionRefused, "refused"),
        );

        let outcome = run_probe(
            ProbeRequest {
                instance: instance(RuntimeType::Ollama),
                endpoint: "http://host:11434".to_string(),
                include_catalog: true,
                cached_models: Vec::new(),
                now: TEST_NOW,
            },
            &client,
            CounterState::default(),
        );

        assert_eq!(outcome.status.status, "offline");
        assert!(outcome.models.is_none(), "the cached list is kept");
        assert_eq!(client.requested.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_readable_health_with_an_unreadable_body_is_degraded() {
        let client = FakeHttpClient::new()
            .with_body(crate::llm::vllm::HEALTH_PATH, 200, "")
            .with_body(crate::llm::vllm::MODELS_PATH, 200, "not json")
            .with_body(crate::llm::vllm::METRICS_PATH, 200, "");

        let outcome = run_probe(
            ProbeRequest {
                instance: instance(RuntimeType::Vllm),
                endpoint: "http://host:11434".to_string(),
                include_catalog: true,
                cached_models: Vec::new(),
                now: TEST_NOW,
            },
            &client,
            CounterState::default(),
        );

        assert_eq!(outcome.status.status, "degraded");
        assert_eq!(
            outcome.partial_error.map(|error| error.code),
            Some(RuntimeErrorCode::ParseError)
        );
    }

    #[test]
    fn a_model_that_stopped_running_is_demoted_rather_than_shown_stale() {
        let mut cached = RuntimeModel::new("llama3:8b", "llama3:8b", "running");
        cached.vram_size_bytes = Some(800);
        cached.vram_resident_percent = Some(80.0);
        cached.expires_in_seconds = Some(30);
        let other = RuntimeModel::new("qwen:7b", "qwen:7b", "installed");

        let merged = merge_live_with_cache(Vec::new(), &[cached, other]);

        assert_eq!(merged.len(), 2);
        let llama = merged.iter().find(|m| m.id == "llama3:8b").unwrap();
        assert_eq!(llama.status, "installed");
        assert_eq!(llama.vram_size_bytes, None, "stale residency is dropped");
        assert_eq!(llama.expires_in_seconds, None);
    }

    #[test]
    fn a_live_model_wins_over_the_cached_copy() {
        let mut live = RuntimeModel::new("llama3:8b", "llama3:8b", "running");
        live.vram_size_bytes = Some(900);
        let cached = RuntimeModel::new("llama3:8b", "llama3:8b", "installed");

        let merged = merge_live_with_cache(vec![live], &[cached]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].status, "running");
        assert_eq!(merged[0].vram_size_bytes, Some(900));
    }

    #[test]
    fn the_summary_counts_instances_by_severity_and_never_leaks_a_key() {
        let mut instances = vec![instance(RuntimeType::Ollama), instance(RuntimeType::Vllm)];
        instances[1].id = "inst-2".to_string();
        instances[1].base_url = "http://host:8000".to_string();

        let mut states = HashMap::new();
        states.insert(
            "inst-1".to_string(),
            InstanceState {
                severity: Some(Severity {
                    level: SeverityLevel::Normal,
                    reasons: Vec::new(),
                }),
                models: vec![RuntimeModel::new("a", "a", "running")],
                ..Default::default()
            },
        );
        states.insert(
            "inst-2".to_string(),
            InstanceState {
                severity: Some(Severity {
                    level: SeverityLevel::Critical,
                    reasons: vec!["engine_dead".to_string()],
                }),
                metrics: Some(RuntimeMetrics {
                    requests_running: Some(3.0),
                    requests_waiting: Some(2.0),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );

        let payload = build_payload(&instances, &states, &|_| true, &|_| None, TEST_NOW);
        assert_eq!(payload.summary.registered, 2);
        assert_eq!(payload.summary.enabled, 2);
        assert_eq!(payload.summary.normal, 1);
        assert_eq!(payload.summary.error, 1);
        assert_eq!(payload.summary.models, 1);
        assert_eq!(payload.summary.vllm_requests_running, Some(3.0));
        assert_eq!(payload.summary.vllm_requests_waiting, Some(2.0));

        // Nothing credential-shaped is serialized, only whether one exists.
        let json = serde_json::to_string(&payload).unwrap();
        assert!(!json.contains("apiKey"), "{json}");
        assert!(!json.contains("Bearer"), "{json}");
        assert!(json.contains("hasApiKey"));
    }

    #[test]
    fn a_runtime_with_no_vllm_instance_reports_null_rather_than_zero_requests() {
        let instances = vec![instance(RuntimeType::Ollama)];
        let payload = build_payload(&instances, &HashMap::new(), &|_| false, &|_| None, TEST_NOW);

        assert_eq!(payload.summary.vllm_requests_running, None);
        assert_eq!(payload.summary.vllm_requests_waiting, None);
        // Never polled is `unknown`, not healthy.
        assert_eq!(payload.summary.unknown, 1);
        assert_eq!(payload.instances[0].severity, "unknown");
    }

    #[test]
    fn consecutive_failures_accumulate_and_reset_on_the_next_success() {
        let instance = instance(RuntimeType::Vllm);
        let mut state = InstanceState::default();

        for attempt in 1..=3 {
            let mut failed = status("offline", None);
            failed.error_code = Some("timeout".to_string());
            failed.error_message = Some("The request timed out.".to_string());
            apply_outcome(
                &mut state,
                &instance,
                ProbeOutcome {
                    instance_id: instance.id.clone(),
                    status: failed,
                    models: None,
                    catalog_refreshed: false,
                    metrics: None,
                    counters_reset: false,
                    partial_error: None,
                    counters: CounterState::default(),
                    now: TEST_NOW + attempt,
                },
            );
            assert_eq!(state.consecutive_failures, attempt as u32);
        }
        assert_eq!(
            state.severity.as_ref().unwrap().level,
            SeverityLevel::Critical
        );
        assert!(state.last_error.is_some());

        apply_outcome(
            &mut state,
            &instance,
            ProbeOutcome {
                instance_id: instance.id.clone(),
                status: status("online", Some(11)),
                models: Some(Vec::new()),
                catalog_refreshed: true,
                metrics: None,
                counters_reset: false,
                partial_error: None,
                counters: CounterState::default(),
                now: TEST_NOW + 10,
            },
        );
        assert_eq!(state.consecutive_failures, 0);
        assert_eq!(state.last_success_at, Some(TEST_NOW + 10));
        assert_eq!(
            state.severity.as_ref().unwrap().level,
            SeverityLevel::Normal
        );
        // The failure is still on record for the detail view.
        assert!(state.last_error.is_some());
        assert!(state
            .events
            .iter()
            .any(|event| event.kind == "status_changed"));
    }

    #[test]
    fn a_counter_reset_is_recorded_as_an_event() {
        let instance = instance(RuntimeType::Vllm);
        let mut state = InstanceState::default();
        apply_outcome(
            &mut state,
            &instance,
            ProbeOutcome {
                instance_id: instance.id.clone(),
                status: status("online", Some(9)),
                models: None,
                catalog_refreshed: false,
                metrics: Some(RuntimeMetrics::default()),
                counters_reset: true,
                partial_error: None,
                counters: CounterState::default(),
                now: TEST_NOW,
            },
        );
        assert!(state
            .events
            .iter()
            .any(|event| event.kind == "counters_reset"));
    }

    #[test]
    fn a_tunneled_instance_carries_its_profile_name_for_the_ui() {
        let mut tunneled = instance(RuntimeType::Ollama);
        tunneled.ssh_profile_id = Some("wsl".to_string());
        let direct = instance(RuntimeType::Vllm);
        let instances = vec![tunneled, direct];

        let payload = build_payload(
            &instances,
            &HashMap::new(),
            &|_| false,
            &|id| (id == "wsl").then(|| "Wsl".to_string()),
            TEST_NOW,
        );

        // A bare 127.0.0.1 would otherwise be ambiguous in the card.
        assert_eq!(payload.instances[0].ssh_profile_name.as_deref(), Some("Wsl"));
        assert_eq!(payload.instances[1].ssh_profile_name, None);

        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("sshProfileName"), "{json}");
        assert!(json.contains("sshProfileId"), "{json}");
    }

    #[test]
    fn an_ssh_failure_flows_through_the_ordinary_outcome_path() {
        let instance = instance(RuntimeType::Vllm);
        let mut state = InstanceState::default();
        let error = RuntimeError::new(
            RuntimeErrorCode::SshTunnelError,
            "SSH tunnel through host:22 failed: Connection refused",
        );

        apply_outcome(
            &mut state,
            &instance,
            ProbeOutcome::failed(&instance, &error, CounterState::default(), TEST_NOW),
        );

        // Reusing `apply_outcome` is what makes backoff, events, history, and
        // severity apply to a tunnel failure without any parallel bookkeeping.
        assert_eq!(state.consecutive_failures, 1);
        assert_eq!(state.status.as_ref().unwrap().status, "offline");
        assert_eq!(
            state.last_error.as_ref().unwrap().code,
            "ssh_tunnel_error"
        );
        assert!(state.events.iter().any(|event| event.kind == "error"));
        assert!(state
            .events
            .iter()
            .any(|event| event.kind == "status_changed"));
        assert_eq!(state.history.len(), 1);
        assert_eq!(
            state.next_poll_at,
            TEST_NOW + next_delay_secs(instance.poll_interval_secs, 1)
        );
        assert!(state
            .severity
            .as_ref()
            .unwrap()
            .reasons
            .iter()
            .any(|reason| reason == "ssh_unreachable"));
    }

    #[test]
    fn a_probe_that_could_not_run_reports_why_without_naming_the_secret() {
        let instance = instance(RuntimeType::Vllm);
        let error = RuntimeError::new(
            RuntimeErrorCode::AuthenticationError,
            "An API key is stored for this instance but the credential vault is locked. Unlock it to resume monitoring.",
        );
        let outcome =
            ProbeOutcome::failed(&instance, &error, CounterState::default(), TEST_NOW);

        assert_eq!(outcome.status.status, "error");
        assert_eq!(
            outcome.status.error_code.as_deref(),
            Some("authentication_error")
        );
        let message = outcome.status.error_message.unwrap();
        assert!(message.contains("vault is locked"), "{message}");
        assert!(outcome.models.is_none(), "the cached list is kept");
    }

    #[test]
    fn the_event_log_is_bounded() {
        let mut state = InstanceState::default();
        for index in 0..(MAX_EVENTS + 10) as u64 {
            state.record_event(TEST_NOW + index, "error", "timeout", None);
        }
        assert_eq!(state.events.len(), MAX_EVENTS);
        // The oldest are the ones dropped.
        assert_eq!(state.events.front().unwrap().at, TEST_NOW + 10);
    }
}

/// Smoke test against a real socket. Ignored by default because it needs a
/// listener; it is the only coverage of `UreqClient` itself, since every test
/// above uses an in-process fake.
///
/// ```text
/// python3 scripts/fake-llm-runtimes.py &
/// cargo test --manifest-path src-tauri/Cargo.toml --lib live_ -- --ignored --nocapture
/// ```
#[cfg(test)]
mod live_smoke {
    use super::*;
    use crate::llm::instance::{DEFAULT_POLL_INTERVAL_SECS, DEFAULT_REQUEST_TIMEOUT_MS};

    fn instance(id: &str, runtime_type: RuntimeType, base_url: &str) -> LlmInstance {
        LlmInstance {
            id: id.to_string(),
            name: id.to_string(),
            runtime_type,
            base_url: base_url.to_string(),
            enabled: true,
            request_timeout_ms: DEFAULT_REQUEST_TIMEOUT_MS,
            poll_interval_secs: DEFAULT_POLL_INTERVAL_SECS,
            created_at: 0,
            updated_at: 0,
            ssh_profile_id: None,
        }
    }

    #[test]
    #[ignore]
    fn live_ollama_and_vllm() {
        let now = 1_799_000_000;

        let ollama = instance("live-ollama", RuntimeType::Ollama, "http://127.0.0.1:18434");
        let client = client_for(&ollama, &ollama.base_url, None);
        let outcome = run_probe(
            ProbeRequest {
                instance: ollama.clone(),
                endpoint: ollama.base_url.clone(),
                include_catalog: true,
                cached_models: Vec::new(),
                now,
            },
            client.as_ref(),
            CounterState::default(),
        );
        println!("ollama status={:?}", outcome.status);
        println!("ollama models={:?}", outcome.models);
        assert_eq!(outcome.status.status, "online");
        assert_eq!(outcome.models.as_ref().unwrap().len(), 2);

        // Without the key the fake vLLM answers 401.
        let vllm = instance("live-vllm", RuntimeType::Vllm, "http://127.0.0.1:18000");
        let client = client_for(&vllm, &vllm.base_url, None);
        let unauthorized = run_probe(
            ProbeRequest {
                instance: vllm.clone(),
                endpoint: vllm.base_url.clone(),
                include_catalog: true,
                cached_models: Vec::new(),
                now,
            },
            client.as_ref(),
            CounterState::default(),
        );
        println!("vllm unauthenticated={:?}", unauthorized.status);
        assert_eq!(unauthorized.status.error_code.as_deref(), Some("authentication_error"));

        let client = client_for(&vllm, &vllm.base_url, Some("sk-test".to_string()));
        let mut counters = CounterState::default();
        let first = run_probe(
            ProbeRequest {
                instance: vllm.clone(),
                endpoint: vllm.base_url.clone(),
                include_catalog: true,
                cached_models: Vec::new(),
                now,
            },
            client.as_ref(),
            counters,
        );
        counters = first.counters;
        println!("vllm status={:?}", first.status);
        println!("vllm metrics={:?}", first.metrics);
        assert_eq!(first.status.status, "online");
        let metrics = first.metrics.unwrap();
        assert_eq!(metrics.requests_running, Some(3.0));
        assert_eq!(metrics.kv_cache_usage_ratio, Some(0.73));
        assert!(metrics.ttft_p50_seconds.is_some());
        assert_eq!(metrics.prompt_tokens_per_second, None, "no baseline yet");

        let second = run_probe(
            ProbeRequest {
                instance: vllm.clone(),
                endpoint: vllm.base_url.clone(),
                include_catalog: false,
                cached_models: Vec::new(),
                now: now + 10,
            },
            client.as_ref(),
            counters,
        );
        // Same counter value 10 s later: a real zero rate, not a missing one.
        assert_eq!(
            second.metrics.unwrap().prompt_tokens_per_second,
            Some(0.0)
        );

        let severity = crate::llm::severity::evaluate(&crate::llm::severity::SeverityInput {
            runtime_type: RuntimeType::Vllm,
            status: &first.status.status,
            error_code: None,
            response_time_ms: first.status.response_time_ms,
            consecutive_failures: 0,
            metrics: None,
            waiting_streak: 0,
        });
        println!("severity={:?}", severity);
    }
}
