use super::parsers::{
    now_epoch_seconds, parse_agy_usage_output, parse_rate_limits, provider_executable_hint,
    strip_terminal_control, value_string, value_u64, RESET_STALE_GRACE_SECONDS,
};
use super::types::{
    AgentMonitorState, AgentQuotaHistoryLimit, AgentQuotaHistoryPoint, AgentQuotaSnapshot,
    AgentSessionMetadata, ProcessSample, Provider,
};
use crate::ssh::session::{open_ssh_session, readable_connect_error, SshTarget};
use crate::ssh::system_monitor::RemoteOs;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde_json::Value;
use ssh2::{Channel, Session};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub(super) const CODEX_QUOTA_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
pub(super) const AGY_QUOTA_REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);
pub(super) const CODEX_QUOTA_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const AGY_STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const AGY_QUOTA_TIMEOUT: Duration = Duration::from_secs(15);
pub(super) const AGY_USAGE_COMMAND: &[u8] = b"/usage\r";
pub(super) const AGY_QUOTA_HISTORY_WINDOW_SECONDS: u64 = 24 * 60 * 60;
pub(super) const AGY_QUOTA_HISTORY_BUCKET_SECONDS: u64 = 5 * 60;
pub(super) const AGY_QUOTA_HISTORY_MAX_POINTS: usize = 288;

pub(super) fn quota_refresh_due(
    state: &mut AgentMonitorState,
    provider: Provider,
    interval: Duration,
) -> bool {
    if state.forced_quota_refresh.remove(&provider) {
        return true;
    }
    state
        .last_quota_refresh
        .get(&provider)
        .map(|last| last.elapsed() >= interval)
        .unwrap_or(true)
}

/// Builds the unavailable quota, replacing the generic message with the specific
/// blocking step when the collector reported one.
pub(super) fn unavailable_with_setup_hint(
    provider: Provider,
    setup: Option<&AgentSessionMetadata>,
    collection_error: Option<&str>,
) -> AgentQuotaSnapshot {
    let mut snapshot = AgentQuotaSnapshot::unavailable(provider);
    let Some(setup) = setup else {
        // No setup record either, so the scrape itself is the story worth
        // telling: without it the card can only say data is missing.
        if let Some(error) = collection_error {
            snapshot.message = Some(format!("Agent metadata could not be collected: {}", error));
        }
        return snapshot;
    };
    if provider != Provider::Claude {
        return snapshot;
    }
    let helper = setup.setup_helper.as_deref().unwrap_or_default();
    let status_line = setup.setup_status_line.as_deref().unwrap_or_default();
    let message = match (helper, status_line) {
        ("missing", _) => Some(
            "The GpuTerm status-line helper is not installed on this host. Run Set up.".to_string(),
        ),
        ("empty", _) => Some(
            "The GpuTerm status-line helper is present but empty, so Claude publishes nothing. Run Set up again."
                .to_string(),
        ),
        (_, "none") => Some(
            "Claude has no status line configured on this host. Run Set up.".to_string(),
        ),
        (_, "other") => Some(
            "Claude uses a different status line, so GpuTerm is not being fed. Add the GpuTerm helper to that pipeline, or run Set up after removing it."
                .to_string(),
        ),
        ("ok", "ours") => Some(
            "Status line installed. Send one message in a Claude session to publish the 5-hour and weekly limits."
                .to_string(),
        ),
        _ => None,
    };
    match (message, collection_error) {
        (Some(message), _) => snapshot.message = Some(message),
        (None, Some(error)) => {
            snapshot.message = Some(format!("Agent metadata could not be collected: {}", error))
        }
        (None, None) => {}
    }
    snapshot
}

pub(super) fn merge_metadata_quota_fallbacks(
    state: &mut AgentMonitorState,
    providers: &HashSet<Provider>,
    now: u64,
) {
    for provider in providers {
        // AGY account quota comes only from the experimental live `/usage`
        // probe. Cached status payloads may still enrich session/context
        // metadata, but must never masquerade as a successful live quota read.
        if *provider == Provider::Agy {
            state
                .quotas
                .entry(*provider)
                .or_insert_with(|| AgentQuotaSnapshot::unavailable(*provider));
            continue;
        }
        let Some(sessions) = state.metadata.get(provider) else {
            let error = state.last_metadata_error.clone();
            state.quotas.entry(*provider).or_insert_with(|| {
                unavailable_with_setup_hint(*provider, None, error.as_deref())
            });
            continue;
        };
        let newest = sessions
            .iter()
            .filter(|metadata| !metadata.rate_limits.is_empty())
            .max_by_key(|metadata| metadata.captured_at.unwrap_or(0));
        let Some(metadata) = newest else {
            // No published limits: say which install step is incomplete rather
            // than only that the data is missing.
            let hint = sessions.iter().find(|entry| entry.setup_scope);
            let snapshot = unavailable_with_setup_hint(
                *provider,
                hint,
                state.last_metadata_error.as_deref(),
            );
            state
                .quotas
                .entry(*provider)
                .and_modify(|current| {
                    if current.limits.is_empty() {
                        *current = snapshot.clone();
                    }
                })
                .or_insert(snapshot);
            continue;
        };
        let source = match provider {
            Provider::Codex => "codex-session-log",
            Provider::Claude => "claude-statusline",
            Provider::Agy => unreachable!("AGY quota is collected only from the live PTY probe"),
        };
        let fallback = AgentQuotaSnapshot::available(
            source,
            metadata.captured_at,
            metadata.rate_limits.clone(),
            now,
        );
        let replace = match state.quotas.get(provider) {
            None => true,
            Some(current) if *provider == Provider::Claude => {
                fallback.captured_at.unwrap_or(0) >= current.captured_at.unwrap_or(0)
            }
            Some(current) if *provider == Provider::Codex => current.source != "codex-app-server",
            Some(_) => false,
        };
        if replace {
            state.quotas.insert(*provider, fallback);
        }
    }
}

/// Latest account-quota probe results, published by background threads.
///
/// The provider CLIs these probes drive can take tens of seconds (AGY allows a
/// 5 s start-up plus a 15 s read). Running them inline froze every telemetry
/// card — CPU, GPU, memory — for that whole time, so the probe now happens off
/// the poll thread and each tick simply picks up whatever has landed.
#[derive(Default)]
pub(super) struct QuotaProbes {
    pub(super) finished: Mutex<Vec<(Provider, Result<AgentQuotaSnapshot, String>)>>,
    pub(super) in_flight: Mutex<HashSet<Provider>>,
}

/// Starts a probe unless one for the same provider is still running.
pub(super) fn spawn_quota_probe<F>(probes: &Arc<QuotaProbes>, provider: Provider, probe: F)
where
    F: FnOnce() -> Result<AgentQuotaSnapshot, String> + Send + 'static,
{
    match probes.in_flight.lock() {
        Ok(mut in_flight) => {
            if !in_flight.insert(provider) {
                return;
            }
        }
        Err(_) => return,
    }
    let probes = Arc::clone(probes);
    thread::spawn(move || {
        let result = probe();
        if let Ok(mut finished) = probes.finished.lock() {
            finished.push((provider, result));
        }
        if let Ok(mut in_flight) = probes.in_flight.lock() {
            in_flight.remove(&provider);
        }
    });
}

/// Folds completed probe results into the monitor state.
///
/// All state mutation stays on the poll thread, so the bookkeeping (history,
/// fallbacks, unavailable messages) is identical to the previous inline version.
pub(super) fn apply_finished_quota_probes(
    state: &mut AgentMonitorState,
    providers: &HashSet<Provider>,
    now: u64,
) {
    let finished = state
        .quota_probes
        .finished
        .lock()
        .map(|mut finished| std::mem::take(&mut *finished))
        .unwrap_or_default();
    for (provider, result) in finished {
        match (provider, result) {
            (Provider::Agy, Ok(snapshot)) => {
                record_agy_history(state, now, Some(&snapshot));
                state.quotas.insert(Provider::Agy, snapshot);
            }
            (Provider::Agy, Err(error)) => {
                record_agy_history(state, now, None);
                let mut unavailable = AgentQuotaSnapshot::unavailable(Provider::Agy);
                unavailable.message = Some(format!(
                    "AGY /usage automatic read failed: {}. Open /usage in AGY to view the current quota.",
                    error
                ));
                state.quotas.insert(Provider::Agy, unavailable);
            }
            (provider, Ok(snapshot)) => {
                state.quotas.insert(provider, snapshot);
            }
            (provider, Err(error)) => {
                // A failed live read must not leave an arbitrarily old
                // app-server value looking current. Re-evaluate the newest
                // session-log snapshot as the documented fallback.
                state.quotas.remove(&provider);
                merge_metadata_quota_fallbacks(state, providers, now);
                if provider == Provider::Codex {
                    let quota = state
                        .quotas
                        .entry(provider)
                        .or_insert_with(|| AgentQuotaSnapshot::unavailable(provider));
                    if quota.limits.is_empty() {
                        quota.message =
                            Some(format!("Codex account limits are unavailable: {}", error));
                    }
                }
            }
        }
    }
}

pub(super) fn refresh_provider_quotas_local(
    os: RemoteOs,
    providers: &HashSet<Provider>,
    processes: &[ProcessSample],
    state: &mut AgentMonitorState,
) {
    let now = now_epoch_seconds();
    apply_finished_quota_probes(state, providers, now);
    if providers.contains(&Provider::Codex)
        && quota_refresh_due(state, Provider::Codex, CODEX_QUOTA_REFRESH_INTERVAL)
    {
        state
            .last_quota_refresh
            .insert(Provider::Codex, Instant::now());
        let launch_hint = provider_executable_hint(processes, Provider::Codex);
        spawn_quota_probe(&state.quota_probes, Provider::Codex, move || {
            query_codex_quota_local(launch_hint.as_deref(), now_epoch_seconds())
        });
    }
    if providers.contains(&Provider::Agy)
        && quota_refresh_due(state, Provider::Agy, AGY_QUOTA_REFRESH_INTERVAL)
    {
        state
            .last_quota_refresh
            .insert(Provider::Agy, Instant::now());
        let launch_hint = provider_executable_hint(processes, Provider::Agy);
        spawn_quota_probe(&state.quota_probes, Provider::Agy, move || {
            query_agy_quota_local(os, launch_hint.as_deref(), now_epoch_seconds())
        });
    }
}

pub(super) fn refresh_provider_quotas_remote(
    target: &SshTarget,
    os: RemoteOs,
    providers: &HashSet<Provider>,
    state: &mut AgentMonitorState,
) {
    let now = now_epoch_seconds();
    apply_finished_quota_probes(state, providers, now);
    if providers.contains(&Provider::Codex)
        && quota_refresh_due(state, Provider::Codex, CODEX_QUOTA_REFRESH_INTERVAL)
    {
        state
            .last_quota_refresh
            .insert(Provider::Codex, Instant::now());
        // Its own connection: the telemetry session must stay free for the
        // poll, and libssh2 requires one session to be used serially.
        let target = target.clone();
        spawn_quota_probe(&state.quota_probes, Provider::Codex, move || {
            let connection =
                open_ssh_session(&target).map_err(|error| readable_connect_error(&error))?;
            query_codex_quota_remote(connection.session(), os, now_epoch_seconds())
        });
    }
    if providers.contains(&Provider::Agy)
        && quota_refresh_due(state, Provider::Agy, AGY_QUOTA_REFRESH_INTERVAL)
    {
        state
            .last_quota_refresh
            .insert(Provider::Agy, Instant::now());
        let target = target.clone();
        spawn_quota_probe(&state.quota_probes, Provider::Agy, move || {
            let connection =
                open_ssh_session(&target).map_err(|error| readable_connect_error(&error))?;
            query_agy_quota_remote(connection.session(), os, now_epoch_seconds())
        });
    }
}

pub(super) fn update_quota_snapshots(state: &mut AgentMonitorState, now: u64) {
    sync_agy_history(state, now);
    for snapshot in state.quotas.values_mut() {
        snapshot.snapshot_age_seconds = snapshot
            .captured_at
            .map(|captured_at| now.saturating_sub(captured_at));
        for limit in &mut snapshot.limits {
            limit.stale = limit
                .resets_at
                .is_some_and(|resets_at| resets_at.saturating_add(RESET_STALE_GRACE_SECONDS) < now);
        }
        let all_windows_expired =
            !snapshot.limits.is_empty() && snapshot.limits.iter().all(|limit| limit.stale);
        if all_windows_expired {
            snapshot.status = "stale".to_string();
            snapshot.message = Some(
                "The reported quota window has reset; waiting for a fresh provider snapshot."
                    .to_string(),
            );
        } else if snapshot.status == "stale" {
            snapshot.status = "available".to_string();
            snapshot.message = None;
        }
    }
    if let Some(snapshot) = state.quotas.get_mut(&Provider::Agy) {
        snapshot.history = state.agy_history.clone();
    }
}

pub(super) fn record_agy_history(
    state: &mut AgentMonitorState,
    captured_at: u64,
    snapshot: Option<&AgentQuotaSnapshot>,
) {
    let point = AgentQuotaHistoryPoint {
        captured_at,
        status: if snapshot.is_some() {
            "available"
        } else {
            "unavailable"
        }
        .to_string(),
        limits: snapshot
            .into_iter()
            .flat_map(|snapshot| snapshot.limits.iter())
            .filter_map(|limit| {
                let window_minutes = limit.window_minutes?;
                matches!(window_minutes, 300 | 10_080).then(|| AgentQuotaHistoryLimit {
                    group: limit.group.clone(),
                    window_minutes,
                    remaining_percent: (!limit.stale).then_some(limit.remaining_percent).flatten(),
                })
            })
            .collect(),
    };
    let key = state.agy_history_key.clone();
    let histories = state.agy_histories.clone();
    if let (Some(key), Some(histories)) = (key, histories) {
        if let Ok(mut histories) = histories.lock() {
            let history = histories.entry(key).or_default();
            upsert_agy_history_point(history, point, captured_at);
            state.agy_history = history.clone();
            return;
        }
    }
    upsert_agy_history_point(&mut state.agy_history, point, captured_at);
}

pub(super) fn sync_agy_history(state: &mut AgentMonitorState, now: u64) {
    let key = state.agy_history_key.clone();
    let histories = state.agy_histories.clone();
    if let (Some(key), Some(histories)) = (key, histories) {
        if let Ok(mut histories) = histories.lock() {
            let history = histories.entry(key).or_default();
            trim_agy_history(history, now);
            state.agy_history = history.clone();
            return;
        }
    }
    trim_agy_history(&mut state.agy_history, now);
}

pub(super) fn upsert_agy_history_point(
    history: &mut Vec<AgentQuotaHistoryPoint>,
    point: AgentQuotaHistoryPoint,
    now: u64,
) {
    let bucket = point.captured_at / AGY_QUOTA_HISTORY_BUCKET_SECONDS;
    if let Some(existing) = history
        .iter_mut()
        .find(|existing| existing.captured_at / AGY_QUOTA_HISTORY_BUCKET_SECONDS == bucket)
    {
        *existing = point;
    } else {
        history.push(point);
        history.sort_by_key(|point| point.captured_at);
    }
    trim_agy_history(history, now);
}

pub(super) fn trim_agy_history(history: &mut Vec<AgentQuotaHistoryPoint>, now: u64) {
    let cutoff = now.saturating_sub(AGY_QUOTA_HISTORY_WINDOW_SECONDS);
    history.retain(|point| point.captured_at >= cutoff);
    if history.len() > AGY_QUOTA_HISTORY_MAX_POINTS {
        history.drain(..history.len() - AGY_QUOTA_HISTORY_MAX_POINTS);
    }
}

pub(super) fn codex_initialize_request() -> &'static str {
    r#"{"id":1,"method":"initialize","params":{"clientInfo":{"name":"gputerm-monitor","version":"1"},"capabilities":{"experimentalApi":true}}}"#
}

pub(super) fn codex_rate_limit_request() -> &'static str {
    r#"{"id":2,"method":"account/rateLimits/read","params":null}"#
}

pub(super) fn query_codex_quota_local(
    _launch_hint: Option<&str>,
    now: u64,
) -> Result<AgentQuotaSnapshot, String> {
    #[cfg(target_os = "windows")]
    let mut command = if let Some(path) = _launch_hint {
        let mut command = Command::new(path);
        command.arg("app-server");
        command
    } else {
        // CreateProcess does not resolve npm's `.cmd` shims. cmd.exe does, and
        // it also preserves the user's PATHEXT behavior.
        let mut command = Command::new("cmd.exe");
        command.args(["/D", "/S", "/C", "codex app-server"]);
        command
    };
    #[cfg(not(target_os = "windows"))]
    let mut command = {
        let shell = std::env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into());
        let mut command = Command::new(shell);
        command.args(["-lc", "exec codex app-server"]);
        command
    };
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to start codex app-server: {}", error))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "codex app-server stdin unavailable".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "codex app-server stdout unavailable".to_string())?;
    let (sender, receiver) = mpsc::channel::<String>();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                break;
            }
        }
    });

    let result = (|| {
        writeln!(stdin, "{}", codex_initialize_request())
            .and_then(|_| stdin.flush())
            .map_err(|error| format!("failed to initialize codex app-server: {}", error))?;
        wait_for_json_response(&receiver, 1, CODEX_QUOTA_TIMEOUT)?;
        writeln!(stdin, "{}", codex_rate_limit_request())
            .and_then(|_| stdin.flush())
            .map_err(|error| format!("failed to request Codex limits: {}", error))?;
        let response = wait_for_json_response(&receiver, 2, CODEX_QUOTA_TIMEOUT)?;
        parse_codex_quota_response(&response, now)
    })();
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    result
}

pub(super) fn wait_for_json_response(
    receiver: &mpsc::Receiver<String>,
    id: u64,
    timeout: Duration,
) -> Result<Value, String> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(format!("provider response {} timed out", id));
        }
        let line = receiver
            .recv_timeout(remaining)
            .map_err(|_| format!("provider response {} timed out", id))?;
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if value_u64(&value, "id") == Some(id) {
            if let Some(error) = value.get("error") {
                return Err(format!("provider request failed: {}", error));
            }
            return Ok(value);
        }
    }
}

pub(super) fn query_codex_quota_remote(
    session: &Session,
    os: RemoteOs,
    now: u64,
) -> Result<AgentQuotaSnapshot, String> {
    let previous_timeout = session.timeout();
    session.set_timeout(CODEX_QUOTA_TIMEOUT.as_millis() as u32);
    let result = (|| {
        let mut channel = session
            .channel_session()
            .map_err(|error| format!("failed to open Codex quota channel: {}", error))?;
        let command = if os == RemoteOs::Windows {
            "codex app-server"
        } else {
            "exec \"${SHELL:-/bin/sh}\" -lc 'exec codex app-server'"
        };
        channel
            .exec(command)
            .map_err(|error| format!("failed to start remote codex app-server: {}", error))?;
        writeln!(channel, "{}", codex_initialize_request())
            .and_then(|_| channel.flush())
            .map_err(|error| format!("failed to initialize remote codex app-server: {}", error))?;
        read_channel_json_response(&mut channel, 1)?;
        writeln!(channel, "{}", codex_rate_limit_request())
            .and_then(|_| channel.flush())
            .map_err(|error| format!("failed to request remote Codex limits: {}", error))?;
        let response = read_channel_json_response(&mut channel, 2)?;
        let _ = channel.send_eof();
        let _ = channel.close();
        parse_codex_quota_response(&response, now)
    })();
    session.set_timeout(previous_timeout);
    result
}

pub(super) fn read_channel_json_response(channel: &mut Channel, id: u64) -> Result<Value, String> {
    let mut pending = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        let count = channel
            .read(&mut buffer)
            .map_err(|error| format!("provider response read failed: {}", error))?;
        if count == 0 {
            return Err(format!("provider response {} closed early", id));
        }
        pending.extend_from_slice(&buffer[..count]);
        while let Some(position) = pending.iter().position(|byte| *byte == b'\n') {
            let line = pending.drain(..=position).collect::<Vec<_>>();
            let Ok(value) = serde_json::from_slice::<Value>(&line) else {
                continue;
            };
            if value_u64(&value, "id") == Some(id) {
                if let Some(error) = value.get("error") {
                    return Err(format!("provider request failed: {}", error));
                }
                return Ok(value);
            }
        }
    }
}

pub(super) fn parse_codex_quota_response(response: &Value, now: u64) -> Result<AgentQuotaSnapshot, String> {
    let result = response
        .get("result")
        .ok_or_else(|| "Codex rate-limit response has no result".to_string())?;
    let mut snapshots = Vec::<(&str, &Value)>::new();
    if let Some(by_id) = result.get("rateLimitsByLimitId").and_then(Value::as_object) {
        for (limit_id, snapshot) in by_id {
            snapshots.push((limit_id.as_str(), snapshot));
        }
    }
    if snapshots.is_empty() {
        if let Some(snapshot) = result.get("rateLimits") {
            snapshots.push(("codex", snapshot));
        }
    }
    let grouped = snapshots.len() > 1;
    let mut limits = Vec::new();
    for (limit_id, snapshot) in snapshots {
        let group = value_string(snapshot, "limitName")
            .or_else(|| value_string(snapshot, "limitId"))
            .or_else(|| grouped.then(|| limit_id.to_string()));
        let mut parsed = parse_rate_limits(snapshot.as_object().map(|_| snapshot), now);
        if grouped || group.as_deref().is_some_and(|value| value != "codex") {
            for limit in &mut parsed {
                limit.group = group.clone();
            }
        }
        limits.extend(parsed);
    }
    if limits.is_empty() {
        return Err("Codex account returned no rate-limit windows".to_string());
    }
    Ok(AgentQuotaSnapshot::available(
        "codex-app-server",
        Some(now),
        limits,
        now,
    ))
}

pub(super) fn query_agy_quota_local(
    os: RemoteOs,
    launch_hint: Option<&str>,
    now: u64,
) -> Result<AgentQuotaSnapshot, String> {
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 44,
            cols: 140,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| format!("failed to open AGY PTY: {}", error))?;
    let mut command = if os == RemoteOs::Windows {
        if let Some(path) = launch_hint {
            CommandBuilder::new(path)
        } else {
            // A PTY can host cmd.exe directly, which lets npm-installed
            // `agy.cmd` resolve and retain its interactive terminal behavior.
            let mut command = CommandBuilder::new("cmd.exe");
            command.args(["/D", "/S", "/C", "agy"]);
            command
        }
    } else {
        let shell = std::env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into());
        let mut command = CommandBuilder::new(shell);
        command.args(["-lc", "exec agy"]);
        command
    };
    command.env("TERM", "xterm-256color");
    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| format!("failed to start AGY: {}", error))?;
    drop(pair.slave);
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|error| format!("failed to open AGY input: {}", error))?;
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| format!("failed to open AGY output: {}", error))?;
    let (sender, receiver) = mpsc::channel::<Vec<u8>>();
    thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    if sender.send(buffer[..count].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    let result = (|| {
        wait_for_agy_startup(&receiver, AGY_STARTUP_TIMEOUT)?;
        write_agy_usage_command(&mut writer)
            .map_err(|error| format!("failed to request AGY /usage: {}", error))?;
        let output = collect_pty_probe_output(&receiver, AGY_QUOTA_TIMEOUT);
        parse_agy_usage_output(&String::from_utf8_lossy(&output), now)
    })();
    let _ = writer.write_all(b"\x1b\x03");
    let _ = writer.flush();
    let _ = child.kill();
    let _ = child.wait();
    result
}

pub(super) fn wait_for_agy_startup(
    receiver: &mpsc::Receiver<Vec<u8>>,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    let mut output = Vec::new();
    let mut last_visible_output = None;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return last_visible_output
                .map(|_| ())
                .ok_or_else(|| "AGY produced no startup output".to_string());
        }
        match receiver.recv_timeout(remaining.min(Duration::from_millis(100))) {
            Ok(chunk) => {
                output.extend_from_slice(&chunk);
                if output.len() > 64 * 1024 {
                    let keep_from = output.len() - 64 * 1024;
                    output.drain(..keep_from);
                }
                if !strip_terminal_control(&String::from_utf8_lossy(&output))
                    .trim()
                    .is_empty()
                {
                    last_visible_output = Some(Instant::now());
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if last_visible_output
                    .is_some_and(|last_output| last_output.elapsed() >= Duration::from_millis(200))
                {
                    return Ok(());
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("AGY closed before accepting /usage".to_string());
            }
        }
    }
}

pub(super) fn write_agy_usage_command(writer: &mut impl Write) -> std::io::Result<()> {
    writer.write_all(AGY_USAGE_COMMAND)?;
    writer.flush()
}

pub(super) fn collect_pty_probe_output(receiver: &mpsc::Receiver<Vec<u8>>, timeout: Duration) -> Vec<u8> {
    let deadline = Instant::now() + timeout;
    let mut output = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match receiver.recv_timeout(remaining.min(Duration::from_millis(250))) {
            Ok(chunk) => {
                output.extend_from_slice(&chunk);
                let cleaned = strip_terminal_control(&String::from_utf8_lossy(&output));
                if agy_usage_output_complete(&cleaned) {
                    thread::sleep(Duration::from_millis(300));
                    while let Ok(chunk) = receiver.try_recv() {
                        output.extend_from_slice(&chunk);
                    }
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    output
}

pub(super) fn agy_usage_output_complete(output: &str) -> bool {
    parse_agy_usage_output(output, 0).is_ok()
}

pub(super) fn query_agy_quota_remote(
    session: &Session,
    os: RemoteOs,
    now: u64,
) -> Result<AgentQuotaSnapshot, String> {
    let previous_timeout = session.timeout();
    session.set_timeout(1_000);
    let result = (|| {
        let mut channel = session
            .channel_session()
            .map_err(|error| format!("failed to open AGY quota channel: {}", error))?;
        channel
            .request_pty("xterm-256color", None, Some((140, 44, 0, 0)))
            .map_err(|error| format!("failed to request AGY PTY: {}", error))?;
        let command = if os == RemoteOs::Windows {
            "agy"
        } else {
            "exec \"${SHELL:-/bin/sh}\" -lc 'exec agy'"
        };
        channel
            .exec(command)
            .map_err(|error| format!("failed to start remote AGY: {}", error))?;
        session.set_blocking(false);
        let result = (|| {
            wait_for_remote_agy_startup(&mut channel, AGY_STARTUP_TIMEOUT)?;
            session.set_blocking(true);
            write_agy_usage_command(&mut channel)
                .map_err(|error| format!("failed to request remote AGY /usage: {}", error))?;
            session.set_blocking(false);
            let deadline = Instant::now() + AGY_QUOTA_TIMEOUT;
            let mut output = Vec::new();
            let mut buffer = [0_u8; 4096];
            while Instant::now() < deadline {
                match channel.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        output.extend_from_slice(&buffer[..count]);
                        let cleaned = strip_terminal_control(&String::from_utf8_lossy(&output));
                        if agy_usage_output_complete(&cleaned) {
                            break;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(50));
                    }
                    Err(error) => {
                        return Err(format!("failed to read remote AGY /usage: {}", error))
                    }
                }
            }
            parse_agy_usage_output(&String::from_utf8_lossy(&output), now)
        })();
        session.set_blocking(true);
        let _ = channel.write_all(b"\x1b\x03");
        let _ = channel.send_eof();
        let _ = channel.close();
        result
    })();
    session.set_blocking(true);
    session.set_timeout(previous_timeout);
    result
}

pub(super) fn wait_for_remote_agy_startup(channel: &mut Channel, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    let mut output = Vec::new();
    let mut last_visible_output = None;
    let mut buffer = [0_u8; 4096];
    loop {
        if Instant::now() >= deadline {
            return last_visible_output
                .map(|_| ())
                .ok_or_else(|| "remote AGY produced no startup output".to_string());
        }
        match channel.read(&mut buffer) {
            Ok(0) => return Err("remote AGY closed before accepting /usage".to_string()),
            Ok(count) => {
                output.extend_from_slice(&buffer[..count]);
                if output.len() > 64 * 1024 {
                    let keep_from = output.len() - 64 * 1024;
                    output.drain(..keep_from);
                }
                if !strip_terminal_control(&String::from_utf8_lossy(&output))
                    .trim()
                    .is_empty()
                {
                    last_visible_output = Some(Instant::now());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if last_visible_output
                    .is_some_and(|last_output| last_output.elapsed() >= Duration::from_millis(200))
                {
                    return Ok(());
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(format!("failed to read remote AGY startup: {}", error)),
        }
    }
}
