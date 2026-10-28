//! The MCP tools. Read tools reuse the same backend paths the UI uses;
//! actions ask the user first (`approval`) and everything is audited.

use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Value};
use ssh2::Session;
use tauri::{AppHandle, Manager};

use super::approval::{request_approval, ApprovalRequest};
use super::protocol::ToolOutcome;
use super::{audit, current_settings};
use crate::ssh::container_monitor::container_overview;
use crate::ssh::process_control::{inspect_process, signal_process, ProcessSignal};
use crate::ssh::resource_details::{get_resource_details, resolve_remote_os};
use crate::ssh::session::{
    list_profiles, target_for_active_session, with_ops_session, AppState, SessionProfile,
};
use crate::ssh::sftp::sftp_list_dir;
use crate::ssh::system_monitor::{now_epoch_seconds, shell_quote, RemoteOs};
use crate::ssh::tmux::list_tmux_sessions;

/// Returned (and audited as "denied") when the user says no or does not answer.
const DENIED: &str = "The user denied this request in GpuTerm (or did not answer within two minutes). Do not retry it unless the user asks you to.";
const MAX_DIRECTORY_ENTRIES: usize = 500;
const MAX_HISTORY_POINTS: usize = 60;
const MAX_COMMAND_CHARS: usize = 8_000;
const MAX_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_FILE_BYTES: u64 = 256 * 1024;
const DEFAULT_FILE_BYTES: u64 = 64 * 1024;
const DEFAULT_COMMAND_SECS: u64 = 30;
const MAX_COMMAND_SECS: u64 = 120;
/// Matches the slack `run_remote_command_with_budget` gives libssh2.
const SESSION_TIMEOUT_SLACK_MS: u64 = 3_000;
const SFTP_TIMEOUT_MS: u32 = 15_000;

/// Runs one tool and records it in the audit log.
pub fn call(app: &AppHandle, client: &str, tool: &str, arguments: &Value) -> ToolOutcome {
    let outcome = dispatch(app, client, tool, arguments);
    let (label, error) = match &outcome {
        Ok(_) => ("ok", None),
        Err(message) if message == DENIED => ("denied", None),
        Err(message) => ("error", Some(message.as_str())),
    };
    audit::record(client, tool, arguments, label, error);
    outcome
}

fn dispatch(app: &AppHandle, client: &str, tool: &str, args: &Value) -> ToolOutcome {
    match tool {
        "list_hosts" => list_hosts(app),
        "get_host_status" => get_host_status(app, args),
        "get_usage_history" => get_usage_history(app, args),
        "find_free_gpus" => find_free_gpus(app, args),
        "list_gpu_processes" => resource_details(app, args, "gpu"),
        "list_top_processes" => {
            let sort = optional_str(args, "sort")?.unwrap_or("cpu");
            if sort != "cpu" && sort != "memory" {
                return Err("sort must be \"cpu\" or \"memory\"".into());
            }
            resource_details(app, args, sort)
        }
        "list_containers" => {
            let profile = connected_host(app, args)?;
            remote_only(&profile)?;
            block(container_overview(
                app.state::<AppState>(),
                profile.id.clone(),
            ))
        }
        "list_tmux_sessions" => {
            let profile = connected_host(app, args)?;
            block(list_tmux_sessions(
                app.state::<AppState>(),
                profile.id.clone(),
            ))
        }
        "list_directory" => list_directory(app, args),
        "get_llm_runtimes" => Ok(app
            .state::<AppState>()
            .llm_monitor
            .snapshot()
            .map(|payload| serde_json::to_value(payload).unwrap_or(Value::Null))
            .unwrap_or_else(
                || json!({ "instances": [], "note": "No LLM runtimes have been polled yet." }),
            )),
        "get_active_alerts" => {
            Ok(json!({ "alerts": app.state::<AppState>().alerts.active_alerts() }))
        }
        "run_command" | "read_file" | "kill_process" if !current_settings(app).allow_actions => {
            Err("Actions are turned off in GpuTerm's MCP settings.".into())
        }
        "run_command" => run_command(app, client, args),
        "read_file" => read_file(app, client, args),
        "kill_process" => kill_process(app, client, args),
        _ => Err(format!("Unknown tool: {tool}")),
    }
}

/// Runs a Tauri command's future from the MCP connection thread.
fn block<T: serde::Serialize>(
    future: impl std::future::Future<Output = Result<T, String>>,
) -> ToolOutcome {
    let value = tauri::async_runtime::block_on(future)?;
    serde_json::to_value(value).map_err(|error| format!("Could not encode the result: {error}"))
}

// ---- arguments ----

fn optional_str<'a>(args: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value)),
        Some(_) => Err(format!("{key} must be a string")),
    }
}

fn required_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    optional_str(args, key)?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{key} is required"))
}

fn optional_f64(args: &Value, key: &str) -> Result<Option<f64>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_f64()
            .filter(|value| value.is_finite())
            .map(Some)
            .ok_or_else(|| format!("{key} must be a number")),
    }
}

fn optional_u64(args: &Value, key: &str) -> Result<Option<u64>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("{key} must be a non-negative integer")),
    }
}

// ---- hosts ----

/// Saved profiles plus any connected one that is not saved (the local
/// shell), minus the ones hidden from MCP.
fn visible_profiles(app: &AppHandle) -> Vec<(SessionProfile, bool)> {
    let hidden = current_settings(app).hidden_profiles;
    let connected: Vec<SessionProfile> = app
        .state::<AppState>()
        .active_connections
        .lock()
        .map(|active| {
            active
                .values()
                .map(|connection| connection.profile.clone())
                .collect()
        })
        .unwrap_or_default();
    let mut profiles = list_profiles().unwrap_or_default();
    for profile in &connected {
        if !profiles.iter().any(|saved| saved.id == profile.id) {
            profiles.push(profile.clone());
        }
    }
    profiles
        .into_iter()
        .filter(|profile| !hidden.contains(&profile.id))
        .map(|profile| {
            let is_connected = connected.iter().any(|active| active.id == profile.id);
            (profile, is_connected)
        })
        .collect()
}

/// Finds a host by id, or by name (case-insensitive) when unambiguous.
fn find_host(
    profiles: &[(SessionProfile, bool)],
    wanted: &str,
) -> Result<(SessionProfile, bool), String> {
    let wanted = wanted.trim();
    if let Some(found) = profiles.iter().find(|(profile, _)| profile.id == wanted) {
        return Ok(found.clone());
    }
    let by_name: Vec<_> = profiles
        .iter()
        .filter(|(profile, _)| profile.name.eq_ignore_ascii_case(wanted))
        .collect();
    match by_name.as_slice() {
        [single] => Ok((*single).clone()),
        [] => Err(format!(
            "No GpuTerm host named \"{wanted}\". Call list_hosts to see the available hosts."
        )),
        several => Err(format!(
            "Several hosts are named \"{wanted}\"; pass one of these ids: {}",
            several
                .iter()
                .map(|(profile, _)| profile.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn connected_host(app: &AppHandle, args: &Value) -> Result<SessionProfile, String> {
    let (profile, connected) = find_host(&visible_profiles(app), required_str(args, "host")?)?;
    if !connected {
        return Err(format!(
            "{} is not connected. Ask the user to connect to it in GpuTerm first.",
            profile.name
        ));
    }
    Ok(profile)
}

fn remote_only(profile: &SessionProfile) -> Result<(), String> {
    if profile.is_local {
        Err(format!(
            "{} is this computer's local shell; this tool works on SSH hosts only.",
            profile.name
        ))
    } else {
        Ok(())
    }
}

fn latest_reading(app: &AppHandle, id: &str) -> Option<Value> {
    app.state::<AppState>()
        .latest_telemetry
        .lock()
        .ok()
        .and_then(|latest| latest.get(id).cloned())
}

fn list_hosts(app: &AppHandle) -> ToolOutcome {
    let hosts: Vec<Value> = visible_profiles(app)
        .into_iter()
        .map(|(profile, connected)| {
            let gpu_count = latest_reading(app, &profile.id)
                .and_then(|reading| reading.get("gpu").and_then(Value::as_array).map(Vec::len));
            json!({
                "id": profile.id,
                "name": profile.name,
                "host": if profile.is_local { Value::Null } else { json!(profile.host) },
                "port": if profile.is_local { Value::Null } else { json!(profile.port) },
                "username": if profile.is_local { Value::Null } else { json!(profile.username) },
                "local": profile.is_local,
                "connected": connected,
                "gpuCount": gpu_count,
            })
        })
        .collect();
    Ok(json!({ "hosts": hosts }))
}

fn get_host_status(app: &AppHandle, args: &Value) -> ToolOutcome {
    let profile = connected_host(app, args)?;
    let mut reading = latest_reading(app, &profile.id).ok_or_else(|| {
        format!(
            "No reading from {} yet; telemetry arrives a few seconds after connecting.",
            profile.name
        )
    })?;
    if let Some(object) = reading.as_object_mut() {
        // Claude/AGY quota details are the user's account data, not host state.
        object.remove("agents");
        object.remove("sessionId");
        object.insert("host".into(), json!(profile.name));
    }
    Ok(reading)
}

/// Keeps at most `max` evenly spaced points, always including the last.
fn downsample(points: &mut Vec<Value>, max: usize) {
    if points.len() <= max || max == 0 {
        return;
    }
    let len = points.len();
    let picked: Vec<Value> = (0..max)
        .map(|i| points[(i * (len - 1)) / (max - 1).max(1)].clone())
        .collect();
    *points = picked;
}

fn get_usage_history(app: &AppHandle, args: &Value) -> ToolOutcome {
    let profile = connected_host(app, args)?;
    let minutes = optional_u64(args, "minutes")?.unwrap_or(15);
    if minutes != 15 && minutes != 60 {
        return Err("minutes must be 15 or 60".into());
    }
    let since = now_epoch_seconds().saturating_sub(minutes * 60);
    let payload = app
        .state::<AppState>()
        .telemetry_history
        .snapshot(&profile.id, Some(since))
        .ok_or_else(|| format!("No history for {} yet.", profile.name))?;
    let mut value = serde_json::to_value(payload).map_err(|error| error.to_string())?;
    if let Some(system) = value.get_mut("system").and_then(Value::as_array_mut) {
        downsample(system, MAX_HISTORY_POINTS);
    }
    if let Some(gpus) = value.get_mut("gpus").and_then(Value::as_array_mut) {
        for gpu in gpus {
            if let Some(points) = gpu.get_mut("points").and_then(Value::as_array_mut) {
                downsample(points, MAX_HISTORY_POINTS);
            }
        }
    }
    if let Some(object) = value.as_object_mut() {
        object.remove("sessionId");
        object.insert("host".into(), json!(profile.name));
        object.insert("minutes".into(), json!(minutes));
    }
    Ok(value)
}

/// GPUs from telemetry readings that pass the filters, most free memory first.
fn free_gpus(readings: &[(String, Value)], min_free_mib: f64, max_util: f64) -> Vec<Value> {
    let mut found: Vec<(f64, Value)> = Vec::new();
    for (host, reading) in readings {
        let Some(gpus) = reading.get("gpu").and_then(Value::as_array) else {
            continue;
        };
        for gpu in gpus {
            let total = gpu.get("memoryTotalMiB").and_then(Value::as_f64);
            let used = gpu.get("memoryUsedMiB").and_then(Value::as_f64);
            let util = gpu.get("gpuUtilPercent").and_then(Value::as_f64);
            let (Some(total), Some(used), Some(util)) = (total, used, util) else {
                continue;
            };
            let free = (total - used).max(0.0);
            if free < min_free_mib || util > max_util {
                continue;
            }
            found.push((
                free,
                json!({
                    "host": host,
                    "index": gpu.get("index"),
                    "name": gpu.get("name"),
                    "utilPercent": util,
                    "freeMiB": free,
                    "totalMiB": total,
                    "temperatureC": gpu.get("temperatureC"),
                }),
            ));
        }
    }
    found.sort_by(|a, b| b.0.total_cmp(&a.0));
    found.into_iter().map(|(_, gpu)| gpu).collect()
}

fn find_free_gpus(app: &AppHandle, args: &Value) -> ToolOutcome {
    let min_free_gb = optional_f64(args, "min_free_vram_gb")?
        .unwrap_or(0.0)
        .max(0.0);
    let max_util = optional_f64(args, "max_util_percent")?.unwrap_or(10.0);
    let readings: Vec<(String, Value)> = visible_profiles(app)
        .into_iter()
        .filter(|(_, connected)| *connected)
        .filter_map(|(profile, _)| {
            latest_reading(app, &profile.id).map(|reading| (profile.name, reading))
        })
        .collect();
    let checked: Vec<&str> = readings.iter().map(|(host, _)| host.as_str()).collect();
    Ok(json!({
        "gpus": free_gpus(&readings, min_free_gb * 1024.0, max_util),
        "hostsChecked": checked,
    }))
}

fn resource_details(app: &AppHandle, args: &Value, kind: &str) -> ToolOutcome {
    let profile = connected_host(app, args)?;
    block(get_resource_details(
        app.state::<AppState>(),
        profile.id.clone(),
        kind.to_string(),
    ))
}

fn list_directory(app: &AppHandle, args: &Value) -> ToolOutcome {
    let profile = connected_host(app, args)?;
    remote_only(&profile)?;
    let path = optional_str(args, "path")?.unwrap_or("").to_string();
    let mut listing = block(sftp_list_dir(
        app.state::<AppState>(),
        profile.id.clone(),
        path,
    ))?;
    if let Some(entries) = listing.get_mut("entries").and_then(Value::as_array_mut) {
        if entries.len() > MAX_DIRECTORY_ENTRIES {
            let total = entries.len();
            entries.truncate(MAX_DIRECTORY_ENTRIES);
            listing["truncated"] = json!(true);
            listing["totalEntries"] = json!(total);
        }
    }
    Ok(listing)
}

// ---- actions ----

fn ask(
    app: &AppHandle,
    client: &str,
    tool: &str,
    profile: &SessionProfile,
    summary: String,
    detail: String,
) -> Result<(), String> {
    let request = ApprovalRequest::new(client, tool, &profile.name, summary, detail);
    if request_approval(app, request) {
        Ok(())
    } else {
        Err(DENIED.to_string())
    }
}

/// Keeps the first `max` bytes as text, noting whether anything was cut.
fn capped_text(bytes: &[u8], max: usize) -> (String, bool) {
    let truncated = bytes.len() > max;
    let kept = &bytes[..bytes.len().min(max)];
    (String::from_utf8_lossy(kept).into_owned(), truncated)
}

/// Reads a stream to its end, keeping at most `max` + 1 bytes so the caller
/// can tell it was cut. The rest is drained so the remote side is not
/// blocked on a full window.
fn read_capped(reader: &mut impl Read, max: usize) -> std::io::Result<Vec<u8>> {
    let mut kept = Vec::new();
    let mut buffer = [0u8; 16 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            return Ok(kept);
        }
        let room = (max + 1).saturating_sub(kept.len());
        kept.extend_from_slice(&buffer[..read.min(room)]);
    }
}

fn exec_capped(
    session: &Session,
    os: RemoteOs,
    command: &str,
    budget_secs: u64,
) -> Result<Value, String> {
    let wire = match os {
        RemoteOs::Windows => command.to_string(),
        _ => {
            let quoted = shell_quote(command);
            format!(
                "command -v timeout >/dev/null 2>&1 && exec timeout {budget_secs}s sh -lc {quoted}; exec sh -lc {quoted}"
            )
        }
    };
    let previous = session.timeout();
    session
        .set_timeout((budget_secs * 1000 + SESSION_TIMEOUT_SLACK_MS).min(u32::MAX as u64) as u32);
    let result = (|| {
        let mut channel = session
            .channel_session()
            .map_err(|error| format!("Could not open a command channel: {error}"))?;
        channel
            .exec(&wire)
            .map_err(|error| format!("Could not run the command: {error}"))?;
        let stdout = read_capped(&mut channel, MAX_OUTPUT_BYTES)
            .map_err(|error| format!("The command's output could not be read: {error}"))?;
        let stderr = read_capped(&mut channel.stderr(), MAX_OUTPUT_BYTES).unwrap_or_default();
        let _ = channel.wait_close();
        let exit_code = channel.exit_status().unwrap_or(-1);
        let (stdout, stdout_truncated) = capped_text(&stdout, MAX_OUTPUT_BYTES);
        let (stderr, stderr_truncated) = capped_text(&stderr, MAX_OUTPUT_BYTES);
        Ok(json!({
            "exitCode": exit_code,
            "timedOut": os != RemoteOs::Windows && exit_code == 124,
            "truncated": stdout_truncated || stderr_truncated,
            "untrusted_output": { "stdout": stdout, "stderr": stderr },
        }))
    })();
    session.set_timeout(previous);
    result
}

fn run_command(app: &AppHandle, client: &str, args: &Value) -> ToolOutcome {
    let profile = connected_host(app, args)?;
    remote_only(&profile)?;
    let command = required_str(args, "command")?;
    if command.chars().count() > MAX_COMMAND_CHARS {
        return Err(format!(
            "command is longer than {MAX_COMMAND_CHARS} characters"
        ));
    }
    let budget = optional_u64(args, "timeout_secs")?
        .unwrap_or(DEFAULT_COMMAND_SECS)
        .clamp(1, MAX_COMMAND_SECS);
    ask(
        app,
        client,
        "run_command",
        &profile,
        format!(
            "Run a command on {} as {} (time limit {budget}s)",
            profile.name, profile.username
        ),
        command.to_string(),
    )?;
    let state = app.state::<AppState>();
    let target = target_for_active_session(&state, &profile.id)?;
    let ops = Arc::clone(&state.ops_sessions);
    let os_cache = Arc::clone(&state.remote_os_cache);
    let timeout_ms = ((budget * 1000) + SESSION_TIMEOUT_SLACK_MS).min(u32::MAX as u64) as u32;
    let mut result = with_ops_session(&ops, &target, timeout_ms, |session| {
        let os = resolve_remote_os(&os_cache, &target.session_id, session);
        exec_capped(session, os, command, budget)
    })?;
    result["host"] = json!(profile.name);
    Ok(result)
}

/// Text only: NUL bytes or invalid UTF-8 (beyond a cut at the end) mean binary.
fn looks_binary(bytes: &[u8]) -> bool {
    if bytes.contains(&0) {
        return true;
    }
    match std::str::from_utf8(bytes) {
        Ok(_) => false,
        // A multi-byte character split by the size cap is still text.
        Err(error) => error.error_len().is_some(),
    }
}

fn read_file(app: &AppHandle, client: &str, args: &Value) -> ToolOutcome {
    let profile = connected_host(app, args)?;
    remote_only(&profile)?;
    let path = required_str(args, "path")?.trim().to_string();
    let max_bytes = optional_u64(args, "max_bytes")?
        .unwrap_or(DEFAULT_FILE_BYTES)
        .clamp(1, MAX_FILE_BYTES);
    ask(
        app,
        client,
        "read_file",
        &profile,
        format!(
            "Read a file on {} (up to {} KiB)",
            profile.name,
            max_bytes.div_ceil(1024)
        ),
        path.clone(),
    )?;
    let state = app.state::<AppState>();
    let target = target_for_active_session(&state, &profile.id)?;
    let ops = Arc::clone(&state.ops_sessions);
    let (bytes, size) = with_ops_session(&ops, &target, SFTP_TIMEOUT_MS, |session| {
        let sftp = session
            .sftp()
            .map_err(|error| format!("SFTP failed to start: {error}"))?;
        let remote = Path::new(&path);
        let stat = sftp
            .stat(remote)
            .map_err(|error| format!("{path}: {error}"))?;
        if stat.is_dir() {
            return Err(format!("{path} is a directory; use list_directory"));
        }
        let mut file = sftp
            .open(remote)
            .map_err(|error| format!("{path} could not be opened: {error}"))?;
        let mut bytes = Vec::new();
        file.by_ref()
            .take(max_bytes + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("{path} could not be read: {error}"))?;
        Ok((bytes, stat.size))
    })?;
    let truncated = bytes.len() as u64 > max_bytes;
    let kept = &bytes[..bytes.len().min(max_bytes as usize)];
    if looks_binary(kept) {
        return Err(format!(
            "{path} looks like a binary file; only text files can be read."
        ));
    }
    Ok(json!({
        "host": profile.name,
        "path": path,
        "size": size,
        "truncated": truncated,
        "untrusted_output": String::from_utf8_lossy(kept),
    }))
}

fn kill_process(app: &AppHandle, client: &str, args: &Value) -> ToolOutcome {
    let profile = connected_host(app, args)?;
    let pid = optional_u64(args, "pid")?.ok_or("pid is required")?;
    let pid = u32::try_from(pid).map_err(|_| "pid is out of range".to_string())?;
    let force = match args.get("force") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(force)) => *force,
        Some(_) => return Err("force must be true or false".into()),
    };
    let identity = tauri::async_runtime::block_on(inspect_process(
        app.state::<AppState>(),
        profile.id.clone(),
        pid,
    ))?;
    if !identity.alive {
        return Ok(
            json!({ "status": "gone", "message": "No such process is visible to this login." }),
        );
    }
    let marker = identity
        .start_marker
        .clone()
        .ok_or("The process start time could not be read, so it cannot be ended safely.")?;
    let user = identity.user.clone().unwrap_or_else(|| "?".into());
    let command = identity.command.clone().unwrap_or_else(|| "?".into());
    ask(
        app,
        client,
        "kill_process",
        &profile,
        format!(
            "{} process {pid} (user {user}) on {}",
            if force { "Force stop" } else { "End" },
            profile.name
        ),
        command.clone(),
    )?;
    let outcome = tauri::async_runtime::block_on(signal_process(
        app.state::<AppState>(),
        profile.id.clone(),
        pid,
        if force {
            ProcessSignal::Kill
        } else {
            ProcessSignal::Term
        },
        marker,
    ))?;
    Ok(json!({
        "host": profile.name,
        "pid": pid,
        "user": user,
        "command": command,
        "status": outcome.status,
        "message": outcome.message,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(id: &str, name: &str) -> SessionProfile {
        serde_json::from_value(json!({
            "id": id, "name": name, "host": "h", "port": 22, "username": "u"
        }))
        .unwrap()
    }

    #[test]
    fn hosts_resolve_by_id_or_unambiguous_name() {
        let profiles = vec![
            (profile("a1", "gpu-01"), true),
            (profile("b2", "gpu-02"), false),
            (profile("c3", "twin"), true),
            (profile("d4", "Twin"), true),
        ];
        assert_eq!(find_host(&profiles, "b2").unwrap().0.name, "gpu-02");
        assert_eq!(find_host(&profiles, "GPU-01").unwrap().0.id, "a1");
        let ambiguous = find_host(&profiles, "twin").unwrap_err();
        assert!(ambiguous.contains("c3") && ambiguous.contains("d4"));
        assert!(find_host(&profiles, "nope")
            .unwrap_err()
            .contains("list_hosts"));
    }

    #[test]
    fn free_gpus_are_filtered_and_sorted_by_free_memory() {
        let gpu = |index: u32, total: u64, used: u64, util: f64| {
            json!({ "index": index, "name": "A100", "memoryTotalMiB": total,
                    "memoryUsedMiB": used, "gpuUtilPercent": util })
        };
        let readings = vec![
            (
                "lab".to_string(),
                json!({ "gpu": [gpu(0, 81920, 80000, 0.0), gpu(1, 81920, 1000, 3.0)] }),
            ),
            (
                "box".to_string(),
                json!({ "gpu": [gpu(0, 24576, 0, 0.0), gpu(1, 24576, 0, 95.0)] }),
            ),
            ("cpu-only".to_string(), json!({ "gpu": [] })),
        ];
        let found = free_gpus(&readings, 10.0 * 1024.0, 10.0);
        let summary: Vec<(String, u64)> = found
            .iter()
            .map(|gpu| {
                (
                    gpu["host"].as_str().unwrap().to_string(),
                    gpu["index"].as_u64().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![("lab".to_string(), 1), ("box".to_string(), 0)]
        );
        assert_eq!(found[0]["freeMiB"], 80920.0);
    }

    #[test]
    fn downsampling_keeps_the_ends() {
        let mut points: Vec<Value> = (0..720).map(|i| json!(i)).collect();
        downsample(&mut points, 60);
        assert_eq!(points.len(), 60);
        assert_eq!(points[0], json!(0));
        assert_eq!(points[59], json!(719));
        let mut few: Vec<Value> = (0..5).map(|i| json!(i)).collect();
        downsample(&mut few, 60);
        assert_eq!(few.len(), 5);
    }

    #[test]
    fn output_is_capped_and_marked() {
        let data = [b'a'; 100];
        let kept = read_capped(&mut &data[..], 10).unwrap();
        assert_eq!(kept.len(), 11);
        assert_eq!(capped_text(&kept, 10), ("a".repeat(10), true));
        assert_eq!(capped_text(b"ok", 10), ("ok".to_string(), false));
    }

    #[test]
    fn binary_content_is_detected() {
        assert!(!looks_binary(b"plain text\n"));
        assert!(looks_binary(b"\x7fELF\x00\x01"));
        assert!(looks_binary(&[0xff, 0xfe, b'a']));
        // "é" cut in half by the size cap is still text.
        assert!(!looks_binary(&"caf\u{e9}".as_bytes()[..4]));
    }
}
