use super::types::{
    AgentMetric, AgentMonitorState, AgentQuotaSnapshot, AgentRateLimitMetric,
    AgentSessionMetadata, AgentWorkMetric, ProcessSample, Provider,
};
use crate::ssh::system_monitor::RemoteOs;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub(super) fn detected_providers(processes: &[ProcessSample]) -> HashSet<Provider> {
    processes.iter().filter_map(provider_for_process).collect()
}

/// Returns the native provider executable already observed in the Windows
/// process list. This is more reliable than the GUI application's PATH, which
/// may not contain CLI directories added by a terminal profile.
pub(super) fn provider_executable_hint(processes: &[ProcessSample], provider: Provider) -> Option<String> {
    processes
        .iter()
        .filter(|process| provider_for_process(process) == Some(provider))
        .filter_map(|process| process.executable_path.as_deref())
        .find(|path| {
            // Parse both separators even when this unit test is running on a
            // non-Windows host.
            let file_name = path
                .rsplit(['\\', '/'])
                .next()
                .unwrap_or(path)
                .to_ascii_lowercase();
            let file_name = file_name.strip_suffix(".exe").unwrap_or(&file_name);
            match provider {
                Provider::Codex => file_name == "codex" || file_name.starts_with("codex-"),
                Provider::Agy => file_name == "agy" || file_name == "antigravity",
                Provider::Claude => file_name == "claude",
            }
        })
        .map(str::to_string)
}

pub(super) fn parse_processes(
    os: RemoteOs,
    output: &str,
    state: &mut AgentMonitorState,
) -> Result<Vec<ProcessSample>, String> {
    if os == RemoteOs::Windows {
        parse_windows_processes(output, state)
    } else {
        Ok(parse_posix_processes(output))
    }
}

pub(super) fn parse_posix_processes(output: &str) -> Vec<ProcessSample> {
    output
        .lines()
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 7 {
                return None;
            }
            Some(ProcessSample {
                pid: fields[0].parse().ok()?,
                ppid: fields[1].parse().ok()?,
                user: nonempty(fields[2]),
                cpu_percent: fields[3].parse().ok(),
                rss_bytes: fields[4]
                    .parse::<u64>()
                    .ok()
                    .map(|kib| kib.saturating_mul(1024)),
                elapsed_seconds: parse_elapsed(fields[5]),
                name: String::new(),
                command: fields[6..].join(" "),
                executable_path: None,
            })
        })
        .collect()
}

pub(super) fn parse_windows_processes(
    output: &str,
    state: &mut AgentMonitorState,
) -> Result<Vec<ProcessSample>, String> {
    let root: Value = serde_json::from_str(output.trim())
        .map_err(|error| format!("Agent process monitoring unavailable: {}", error))?;
    let logical = value_u64(&root, "logicalCores").unwrap_or(1).max(1) as f64;
    let rows = root
        .get("processes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let now = Instant::now();
    let elapsed = state
        .windows_cpu_sampled_at
        .map(|last| now.duration_since(last).as_secs_f64())
        .filter(|seconds| *seconds > 0.0);
    let mut next_cpu = HashMap::new();
    let samples = rows
        .iter()
        .filter_map(|row| {
            let pid = value_u64(row, "pid")? as u32;
            let cpu_seconds = value_f64(row, "cpuSeconds");
            if let Some(value) = cpu_seconds {
                next_cpu.insert(pid, value);
            }
            let cpu_percent = match (cpu_seconds, state.windows_cpu.get(&pid), elapsed) {
                (Some(current), Some(previous), Some(seconds)) if current >= *previous => {
                    Some((current - previous) / seconds / logical * 100.0)
                }
                _ => None,
            };
            let name = value_string(row, "name").unwrap_or_default();
            let command = value_string(row, "commandLine")
                .or_else(|| value_string(row, "executablePath"))
                .unwrap_or_else(|| name.clone());
            let executable_path = value_string(row, "executablePath");
            Some(ProcessSample {
                pid,
                ppid: value_u64(row, "ppid").unwrap_or(0) as u32,
                cpu_percent,
                rss_bytes: value_u64(row, "rssBytes"),
                elapsed_seconds: value_u64(row, "elapsedSeconds"),
                name,
                command,
                executable_path,
                ..Default::default()
            })
        })
        .collect();
    state.windows_cpu = next_cpu;
    state.windows_cpu_sampled_at = Some(now);
    Ok(samples)
}

/// Executable names this process could be known by: the reported process name
/// plus two readings of argv[0] — cut at the first flag, which survives paths
/// containing spaces, and the first whitespace token, which survives
/// subcommands such as `codex exec`.
pub(super) fn executable_candidates(process: &ProcessSample) -> Vec<String> {
    let command = process.command.trim();
    let until_flag = command
        .split_once(" -")
        .map(|(head, _)| head)
        .unwrap_or(command);
    let first_token = command.split_whitespace().next().unwrap_or_default();
    [process.name.as_str(), until_flag, first_token]
        .into_iter()
        .filter_map(|value| {
            let value = value.trim().trim_matches('"');
            let name = value.rsplit(['/', '\\']).next().unwrap_or(value).trim();
            let name = name
                .strip_suffix(".exe")
                .or_else(|| name.strip_suffix(".EXE"))
                .unwrap_or(name);
            (!name.is_empty()).then(|| name.to_string())
        })
        .collect()
}

pub(super) fn provider_for_process(process: &ProcessSample) -> Option<Provider> {
    // Matched case-sensitively: a desktop shell such as
    // `/Applications/Claude.app/Contents/MacOS/Claude` launches the agent binary
    // `.../claude` and must not be mistaken for it.
    let candidates = executable_candidates(process);
    let named = |executable: &str| candidates.iter().any(|name| name == executable);
    let command = process.command.to_ascii_lowercase();

    if named("agy") {
        return Some(Provider::Agy);
    }
    if named("codex")
        || command.contains("@openai/codex")
        || command.contains("/codex/bin/codex")
        || command.contains("\\codex\\bin\\codex")
    {
        return Some(Provider::Codex);
    }
    if named("claude")
        || command.contains("@anthropic-ai/claude-code")
        || command.contains("/claude-code/cli.js")
        || command.contains("\\claude-code\\cli.js")
    {
        return Some(Provider::Claude);
    }
    None
}

pub(super) fn build_agent_metrics(
    processes: &[ProcessSample],
    metadata: &HashMap<Provider, Vec<AgentSessionMetadata>>,
    quotas: &HashMap<Provider, AgentQuotaSnapshot>,
) -> Vec<AgentMetric> {
    let by_pid = processes
        .iter()
        .map(|process| (process.pid, process))
        .collect::<HashMap<_, _>>();
    let children = processes
        .iter()
        .fold(HashMap::<u32, Vec<u32>>::new(), |mut map, process| {
            map.entry(process.ppid).or_default().push(process.pid);
            map
        });
    let matched = processes
        .iter()
        .filter_map(|process| provider_for_process(process).map(|provider| (process.pid, provider)))
        .collect::<HashMap<_, _>>();

    let mut roots = matched
        .iter()
        .filter_map(|(&pid, &provider)| {
            let mut parent = by_pid.get(&pid).map(|process| process.ppid).unwrap_or(0);
            while parent != 0 {
                if matched.get(&parent) == Some(&provider) {
                    return None;
                }
                parent = by_pid.get(&parent).map(|process| process.ppid).unwrap_or(0);
            }
            Some((pid, provider))
        })
        .collect::<Vec<_>>();
    roots.sort_by_key(|(pid, provider)| (provider.key(), *pid));

    let root_pids = roots.iter().map(|(pid, _)| *pid).collect::<HashSet<_>>();
    // Subtrees are resolved before attribution: a snapshot names the agent
    // process, which may sit below a launcher that also looks like the agent.
    let trees = roots
        .iter()
        .map(|(root_pid, provider)| {
            let mut stack = vec![*root_pid];
            let mut included = Vec::new();
            while let Some(pid) = stack.pop() {
                included.push(pid);
                if let Some(child_pids) = children.get(&pid) {
                    for child in child_pids {
                        if *child != *root_pid && root_pids.contains(child) {
                            continue;
                        }
                        stack.push(*child);
                    }
                }
            }
            (*root_pid, *provider, included)
        })
        .collect::<Vec<_>>();
    let assignments = assign_metadata(&trees, metadata);
    trees
        .into_iter()
        .filter_map(|(root_pid, provider, included)| {
            let root = by_pid.get(&root_pid)?;
            let cpu_values = included
                .iter()
                .filter_map(|pid| by_pid.get(pid).and_then(|process| process.cpu_percent))
                .collect::<Vec<_>>();
            let memory_values = included
                .iter()
                .filter_map(|pid| by_pid.get(pid).and_then(|process| process.rss_bytes))
                .collect::<Vec<_>>();
            let provider_metadata = assignments.get(&root_pid).cloned().unwrap_or_default();
            let cpu_percent = (!cpu_values.is_empty()).then(|| cpu_values.iter().sum::<f64>());
            let active = cpu_percent.unwrap_or(0.0) >= 0.5;
            Some(AgentMetric {
                provider: provider.key().to_string(),
                display_name: provider.display_name().to_string(),
                status: provider_metadata
                    .status
                    .clone()
                    .unwrap_or_else(|| if active { "active" } else { "idle" }.to_string()),
                root_pid,
                process_count: included.len() as u32,
                user: root.user.clone(),
                cpu_percent,
                memory_bytes: (!memory_values.is_empty())
                    .then(|| memory_values.iter().copied().sum()),
                elapsed_seconds: root.elapsed_seconds,
                session_id: provider_metadata.session_id,
                cwd: provider_metadata.cwd,
                model: provider_metadata.model,
                input_tokens: provider_metadata.input_tokens,
                output_tokens: provider_metadata.output_tokens,
                total_tokens: provider_metadata.total_tokens,
                context_used_tokens: provider_metadata.context_used_tokens,
                context_window_tokens: provider_metadata.context_window_tokens,
                context_used_percent: provider_metadata.context_used_percent,
                context_remaining_tokens: provider_metadata.context_remaining_tokens,
                context_remaining_percent: provider_metadata.context_remaining_percent,
                last_request_input_tokens: provider_metadata.last_request_input_tokens,
                last_request_output_tokens: provider_metadata.last_request_output_tokens,
                last_request_cache_creation_tokens: provider_metadata
                    .last_request_cache_creation_tokens,
                last_request_cache_read_tokens: provider_metadata.last_request_cache_read_tokens,
                cost_usd: provider_metadata.cost_usd,
                // Claude status-line snapshots expose an API/session duration.
                // The process elapsed time remains a useful read-only fallback.
                session_duration_seconds: provider_metadata
                    .session_duration_seconds
                    .or_else(|| root.elapsed_seconds.map(|seconds| seconds as f64)),
                snapshot_age_seconds: provider_metadata.snapshot_age_seconds,
                quota: quotas
                    .get(&provider)
                    .cloned()
                    .unwrap_or_else(|| AgentQuotaSnapshot::unavailable(provider)),
                subagents: provider_metadata.subagents,
                background_tasks: provider_metadata.background_tasks,
            })
        })
        .collect()
}

/// Attributes parsed sessions to agent process trees. A status-line snapshot
/// records the agent's own pid, so trees containing that pid are matched first;
/// anything left over falls back to file-recency order, which is all the provider
/// session records alone can support.
pub(super) fn assign_metadata(
    trees: &[(u32, Provider, Vec<u32>)],
    metadata: &HashMap<Provider, Vec<AgentSessionMetadata>>,
) -> HashMap<u32, AgentSessionMetadata> {
    let mut assigned = HashMap::<u32, AgentSessionMetadata>::new();
    let mut claimed = HashMap::<Provider, HashSet<usize>>::new();
    for (root_pid, provider, included) in trees {
        let Some(sessions) = metadata.get(provider) else {
            continue;
        };
        let matched = sessions
            .iter()
            .position(|session| session.pid_hint.is_some_and(|pid| included.contains(&pid)));
        if let Some(index) = matched {
            claimed.entry(*provider).or_default().insert(index);
            assigned.insert(*root_pid, sessions[index].clone());
        }
    }
    for (root_pid, provider, _) in trees {
        if assigned.contains_key(root_pid) {
            continue;
        }
        let Some(sessions) = metadata.get(provider) else {
            continue;
        };
        let used = claimed.entry(*provider).or_default();
        // A snapshot naming a pid that is live in another tree belongs to that
        // session, so it is never handed to an unrelated process.
        let next = (0..sessions.len()).find(|index| {
            !used.contains(index)
                && sessions[*index].pid_hint.is_none_or(|pid| {
                    !trees.iter().any(|(_, _, included)| included.contains(&pid))
                })
        });
        if let Some(index) = next {
            used.insert(index);
            assigned.insert(*root_pid, sessions[index].clone());
        }
    }
    assigned
}

pub(super) fn parse_metadata_output(output: &str, now: u64) -> HashMap<Provider, Vec<AgentSessionMetadata>> {
    let mut grouped = HashMap::<Provider, Vec<AgentSessionMetadata>>::new();
    let mut provider = None;
    let mut lines = Vec::<String>::new();
    for line in output.lines() {
        if let Some(marker) = line.strip_prefix("__GPUTERM_AGENT_FILE__\t") {
            if let Some(current) = provider.take() {
                insert_provider_metadata(&mut grouped, current, &lines, now);
            }
            let key = marker.split('\t').next().unwrap_or("");
            provider = Provider::parse(key);
            lines.clear();
        } else if line.trim() == "__GPUTERM_AGENT_END__" {
            if let Some(current) = provider.take() {
                insert_provider_metadata(&mut grouped, current, &lines, now);
            }
            lines.clear();
        } else if provider.is_some() {
            lines.push(line.to_string());
        }
    }
    if let Some(current) = provider {
        insert_provider_metadata(&mut grouped, current, &lines, now);
    }
    grouped
}

pub(super) fn insert_provider_metadata(
    grouped: &mut HashMap<Provider, Vec<AgentSessionMetadata>>,
    provider: Provider,
    lines: &[String],
    now: u64,
) {
    let metadata = parse_provider_metadata(provider, lines, now);
    if !metadata_has_values(&metadata) {
        return;
    }
    let sessions = grouped.entry(provider).or_default();
    // The account-wide quota record names the session that published it, so
    // merging by session id would let it overwrite that session's own context
    // and cost. It is kept as a standalone entry that only quota selection uses.
    if metadata.account_scope || metadata.setup_scope {
        sessions.push(metadata);
        return;
    }
    if let Some(session_id) = metadata.session_id.as_deref() {
        if let Some(existing) = sessions
            .iter_mut()
            .find(|existing| existing.session_id.as_deref() == Some(session_id))
        {
            merge_metadata(existing, metadata);
            return;
        }
    }
    sessions.push(metadata);
}

pub(super) fn metadata_has_values(metadata: &AgentSessionMetadata) -> bool {
    metadata.account_scope
        || metadata.setup_scope
        || metadata.session_id.is_some()
        || metadata.cwd.is_some()
        || metadata.model.is_some()
        || metadata.status.is_some()
        || metadata.input_tokens.is_some()
        || metadata.output_tokens.is_some()
        || metadata.total_tokens.is_some()
        || metadata.context_used_tokens.is_some()
        || metadata.context_window_tokens.is_some()
        || metadata.context_used_percent.is_some()
        || metadata.context_remaining_tokens.is_some()
        || metadata.context_remaining_percent.is_some()
        || metadata.last_request_input_tokens.is_some()
        || metadata.last_request_output_tokens.is_some()
        || metadata.last_request_cache_creation_tokens.is_some()
        || metadata.last_request_cache_read_tokens.is_some()
        || metadata.cost_usd.is_some()
        || metadata.session_duration_seconds.is_some()
        || !metadata.rate_limits.is_empty()
        || !metadata.subagents.is_empty()
        || !metadata.background_tasks.is_empty()
}

pub(super) fn merge_metadata(base: &mut AgentSessionMetadata, newer: AgentSessionMetadata) {
    macro_rules! prefer_new {
        ($field:ident) => {
            if newer.$field.is_some() {
                base.$field = newer.$field;
            }
        };
    }
    prefer_new!(session_id);
    prefer_new!(cwd);
    prefer_new!(model);
    prefer_new!(status);
    prefer_new!(input_tokens);
    prefer_new!(output_tokens);
    prefer_new!(total_tokens);
    prefer_new!(context_used_tokens);
    prefer_new!(context_window_tokens);
    prefer_new!(context_used_percent);
    prefer_new!(context_remaining_tokens);
    prefer_new!(context_remaining_percent);
    prefer_new!(last_request_input_tokens);
    prefer_new!(last_request_output_tokens);
    prefer_new!(last_request_cache_creation_tokens);
    prefer_new!(last_request_cache_read_tokens);
    prefer_new!(cost_usd);
    prefer_new!(session_duration_seconds);
    prefer_new!(snapshot_age_seconds);
    prefer_new!(captured_at);
    prefer_new!(pid_hint);
    if !newer.rate_limits.is_empty() {
        base.rate_limits = newer.rate_limits;
    }
    if !newer.subagents.is_empty() {
        base.subagents = newer.subagents;
    }
    if !newer.background_tasks.is_empty() {
        base.background_tasks = newer.background_tasks;
    }
    finalize_context(base);
}

pub(super) fn parse_provider_metadata(provider: Provider, lines: &[String], now: u64) -> AgentSessionMetadata {
    let values = lines
        .iter()
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .collect::<Vec<_>>();
    match provider {
        Provider::Codex => parse_codex_metadata(&values, now),
        Provider::Claude => parse_claude_metadata(&values, now),
        Provider::Agy => parse_agy_metadata(&values, now),
    }
}

pub(super) fn parse_codex_metadata(values: &[Value], now: u64) -> AgentSessionMetadata {
    let mut metadata = AgentSessionMetadata::default();
    for value in values {
        let payload = value.get("payload").unwrap_or(value);
        let event_type = value_string(payload, "type")
            .or_else(|| value_string(value, "type"))
            .unwrap_or_default();
        if event_type == "session_meta" {
            metadata.session_id = metadata
                .session_id
                .or_else(|| value_string(payload, "id"))
                .or_else(|| value_string(payload, "session_id"));
            metadata.cwd = metadata.cwd.or_else(|| value_string(payload, "cwd"));
            metadata.model = metadata
                .model
                .or_else(|| value_string(payload, "model"))
                .or_else(|| value_string(payload, "model_provider"));
        }
        if event_type == "token_count" {
            let info = payload.get("info").unwrap_or(&Value::Null);
            let total = info.get("total_token_usage").unwrap_or(&Value::Null);
            metadata.input_tokens = value_u64(total, "input_tokens");
            metadata.output_tokens = value_u64(total, "output_tokens");
            metadata.total_tokens = value_u64(total, "total_tokens");
            metadata.context_used_tokens = info
                .get("last_token_usage")
                .and_then(|usage| value_u64(usage, "total_tokens"))
                .or(metadata.total_tokens);
            metadata.context_window_tokens = value_u64(info, "model_context_window");
            metadata.context_used_percent =
                ratio_percent(metadata.context_used_tokens, metadata.context_window_tokens);
            // A later `token_count` event may omit the snapshot; keeping the
            // previous one avoids blanking an already known weekly quota.
            let rate_limits = parse_rate_limits(payload.get("rate_limits"), now);
            if !rate_limits.is_empty() {
                metadata.rate_limits = rate_limits;
                metadata.captured_at = event_epoch_seconds(value).or(Some(now));
            }
        }
        if event_type == "turn_context" {
            metadata.model = value_string(payload, "model").or(metadata.model);
        }
    }
    finalize_context(&mut metadata);
    metadata
}

/// Reads Claude Code session records and status-line snapshots.
///
/// `context_window.*` describes the live context window, not cumulative session
/// totals (Claude Code v2.1.132 onwards), so it only feeds the context gauge.
/// Cumulative token counts are deliberately left unset: the transcript tail this
/// monitor samples covers the newest records only, and reporting that partial
/// sum as a session total would be wrong rather than merely incomplete.
pub(super) fn parse_claude_metadata(values: &[Value], now: u64) -> AgentSessionMetadata {
    let mut metadata = AgentSessionMetadata::default();
    let mut seen_messages = HashSet::new();
    let mut latest_message_context = None;
    for value in values {
        // Subagent records describe a worker's own context rather than the
        // session the user started, so they never contribute to its numbers.
        if value.get("isSidechain").and_then(Value::as_bool) == Some(true)
            || value.get("agentId").is_some()
        {
            continue;
        }
        metadata.session_id = value_string(value, "sessionId")
            .or_else(|| value_string(value, "session_id"))
            .or(metadata.session_id);
        metadata.cwd = value_string(value, "cwd")
            .or_else(|| pointer_string(value, "/workspace/current_dir"))
            .or(metadata.cwd);
        metadata.model = value_string(value, "model")
            .or_else(|| pointer_string(value, "/model/display_name"))
            .or_else(|| pointer_string(value, "/model/id"))
            .or_else(|| pointer_string(value, "/message/model"))
            .or(metadata.model);
        metadata.pid_hint = value_u64(value, "pid")
            .and_then(|pid| u32::try_from(pid).ok())
            .or(metadata.pid_hint);
        match value_string(value, "scope").as_deref() {
            Some("account") => metadata.account_scope = true,
            Some("setup") => {
                metadata.setup_scope = true;
                metadata.setup_helper = value_string(value, "helper");
                metadata.setup_status_line = value_string(value, "status_line");
            }
            _ => {}
        }
        let captured_at = value_u64(value, "captured_at")
            .or_else(|| value_u64(value, "capturedAt"))
            .or_else(|| event_epoch_seconds(value));
        metadata.captured_at = captured_at.or(metadata.captured_at);
        metadata.snapshot_age_seconds = captured_at
            .map(|captured| now.saturating_sub(captured))
            .or(metadata.snapshot_age_seconds);
        let cost = value.get("cost").unwrap_or(value);
        metadata.cost_usd = value_f64(cost, "total_cost_usd")
            .or_else(|| value_f64(cost, "totalCostUsd"))
            .or_else(|| value_f64(cost, "costUSD"))
            .or(metadata.cost_usd);
        metadata.session_duration_seconds = value_f64(cost, "total_duration_ms")
            .or_else(|| value_f64(cost, "totalDurationMs"))
            .or_else(|| value_f64(cost, "duration_ms"))
            .or_else(|| value_f64(cost, "durationMs"))
            .map(|milliseconds| milliseconds / 1000.0)
            .or(metadata.session_duration_seconds);
        if let Some(context) = value
            .get("context_window")
            .or_else(|| value.get("contextWindow"))
        {
            metadata.context_window_tokens = value_u64(context, "context_window_size")
                .or_else(|| value_u64(context, "contextWindowSize"))
                .or(metadata.context_window_tokens);
            metadata.context_used_tokens = value_u64(context, "total_input_tokens")
                .or_else(|| value_u64(context, "totalInputTokens"))
                .or(metadata.context_used_tokens);
            metadata.context_used_percent = value_f64(context, "used_percentage")
                .or_else(|| value_f64(context, "usedPercentage"))
                .or(metadata.context_used_percent);
            metadata.context_remaining_percent = value_f64(context, "remaining_percentage")
                .or_else(|| value_f64(context, "remainingPercentage"))
                .or(metadata.context_remaining_percent);
            metadata.context_remaining_tokens = value_u64(context, "remaining_tokens")
                .or_else(|| value_u64(context, "remainingTokens"))
                .or(metadata.context_remaining_tokens);
            if let Some(usage) = context
                .get("current_usage")
                .or_else(|| context.get("currentUsage"))
            {
                apply_last_request_usage(&mut metadata, usage);
            }
        }
        let rate_limits = parse_rate_limits(
            value.get("rate_limits").or_else(|| value.get("rateLimits")),
            now,
        );
        if !rate_limits.is_empty() {
            metadata.rate_limits = rate_limits;
        }
        let message = value.get("message").unwrap_or(value);
        let message_id = value_string(message, "id");
        // The collector emits a file's first line and its tail, so a short file
        // repeats a record. Only the usage read below needs the guard.
        if message_id
            .as_ref()
            .is_some_and(|id| !seen_messages.insert(id.clone()))
        {
            continue;
        }
        if let Some(usage) = message.get("usage") {
            apply_last_request_usage(&mut metadata, usage);
            latest_message_context = Some(
                value_u64(usage, "input_tokens")
                    .unwrap_or(0)
                    .saturating_add(value_u64(usage, "cache_creation_input_tokens").unwrap_or(0))
                    .saturating_add(value_u64(usage, "cache_read_input_tokens").unwrap_or(0)),
            );
        }
    }
    // Without a status-line snapshot the newest assistant record is the only
    // context measurement available.
    if metadata.context_used_tokens.is_none() {
        metadata.context_used_tokens = latest_message_context;
    }
    finalize_context(&mut metadata);
    metadata
}

/// Records the token breakdown of the most recent API call. Claude reports this
/// per request; it is not a session total.
pub(super) fn apply_last_request_usage(metadata: &mut AgentSessionMetadata, usage: &Value) {
    metadata.last_request_input_tokens =
        value_u64(usage, "input_tokens").or(metadata.last_request_input_tokens);
    metadata.last_request_output_tokens =
        value_u64(usage, "output_tokens").or(metadata.last_request_output_tokens);
    metadata.last_request_cache_creation_tokens = value_u64(usage, "cache_creation_input_tokens")
        .or(metadata.last_request_cache_creation_tokens);
    metadata.last_request_cache_read_tokens =
        value_u64(usage, "cache_read_input_tokens").or(metadata.last_request_cache_read_tokens);
}

pub(super) fn parse_agy_metadata(values: &[Value], now: u64) -> AgentSessionMetadata {
    let mut metadata = AgentSessionMetadata::default();
    for value in values {
        let payload = value.get("payload").unwrap_or(value);
        metadata.captured_at = value_u64(payload, "captured_at")
            .or_else(|| value_u64(payload, "capturedAt"))
            .or_else(|| event_epoch_seconds(value))
            .or(metadata.captured_at);
        metadata.session_id = value_string(payload, "conversation_id")
            .or_else(|| value_string(payload, "conversationId"))
            .or_else(|| value_string(payload, "session_id"))
            .or_else(|| value_string(payload, "sessionId"))
            .or(metadata.session_id);
        metadata.cwd = value_string(payload, "cwd")
            .or_else(|| pointer_string(payload, "/workspace/current_dir"))
            .or_else(|| pointer_string(payload, "/workspace/currentDir"))
            .or(metadata.cwd);
        metadata.model = value_string(payload, "model")
            .or_else(|| pointer_string(payload, "/model/display_name"))
            .or_else(|| pointer_string(payload, "/model/displayName"))
            .or_else(|| pointer_string(payload, "/model/id"))
            .or(metadata.model);
        metadata.status = value_string(payload, "agent_state")
            .or_else(|| value_string(payload, "agentState"))
            .or(metadata.status);
        metadata.input_tokens = value_u64(payload, "input_tokens")
            .or_else(|| value_u64(payload, "inputTokens"))
            .or(metadata.input_tokens);
        metadata.output_tokens = value_u64(payload, "output_tokens")
            .or_else(|| value_u64(payload, "outputTokens"))
            .or(metadata.output_tokens);
        metadata.total_tokens = value_u64(payload, "total_tokens")
            .or_else(|| value_u64(payload, "totalTokens"))
            .or(metadata.total_tokens);
        let context = payload
            .get("context_window")
            .or_else(|| payload.get("contextWindow"))
            .or_else(|| payload.get("context"))
            .unwrap_or(payload);
        metadata.input_tokens = metadata.input_tokens.or_else(|| {
            value_u64(context, "total_input_tokens")
                .or_else(|| value_u64(context, "totalInputTokens"))
                .or_else(|| value_u64(context, "input_tokens"))
                .or_else(|| value_u64(context, "inputTokens"))
        });
        metadata.output_tokens = metadata.output_tokens.or_else(|| {
            value_u64(context, "total_output_tokens")
                .or_else(|| value_u64(context, "totalOutputTokens"))
                .or_else(|| value_u64(context, "output_tokens"))
                .or_else(|| value_u64(context, "outputTokens"))
        });
        metadata.context_window_tokens = value_u64(context, "context_window_size")
            .or_else(|| value_u64(context, "contextWindowSize"))
            .or_else(|| value_u64(context, "max_size"))
            .or_else(|| value_u64(context, "maxSize"))
            .or(metadata.context_window_tokens);
        metadata.context_used_percent = value_f64(context, "used_percentage")
            .or_else(|| value_f64(context, "usedPercentage"))
            .or(metadata.context_used_percent);
        metadata.context_remaining_percent = value_f64(context, "remaining_percentage")
            .or_else(|| value_f64(context, "remainingPercentage"))
            .or(metadata.context_remaining_percent);
        metadata.context_used_tokens = value_u64(context, "context_used_tokens")
            .or_else(|| value_u64(context, "contextUsedTokens"))
            .or_else(|| value_u64(context, "input_tokens"))
            .or_else(|| value_u64(context, "inputTokens"))
            .or_else(|| value_u64(context, "used_size"))
            .or_else(|| value_u64(context, "usedSize"))
            .or(metadata.context_used_tokens);
        metadata.context_remaining_tokens = value_u64(context, "remaining_tokens")
            .or_else(|| value_u64(context, "remainingTokens"))
            .or_else(|| value_u64(context, "remaining_size"))
            .or_else(|| value_u64(context, "remainingSize"))
            .or(metadata.context_remaining_tokens);
        let rate_limits = parse_rate_limits(
            payload
                .get("rate_limits")
                .or_else(|| payload.get("rateLimits"))
                .or_else(|| payload.get("quota")),
            now,
        );
        if !rate_limits.is_empty() {
            metadata.rate_limits = rate_limits;
        }
        if let Some(items) = payload.get("subagents").and_then(Value::as_array) {
            metadata.subagents = parse_work_items(items);
        }
        if let Some(items) = payload
            .get("background_tasks")
            .or_else(|| payload.get("backgroundTasks"))
            .and_then(Value::as_array)
        {
            metadata.background_tasks = parse_work_items(items);
        }
    }
    if metadata.total_tokens.is_none() {
        metadata.total_tokens = match (metadata.input_tokens, metadata.output_tokens) {
            (Some(input), Some(output)) => Some(input.saturating_add(output)),
            _ => None,
        };
    }
    finalize_context(&mut metadata);
    metadata
}

/// Keys that sit alongside the real quota windows but describe something else,
/// so they must not be walked as nested limit groups.
pub(super) const NON_QUOTA_LIMIT_KEYS: [&str; 12] = [
    "limit_id",
    "limit_name",
    "plan_type",
    "rate_limit_reached_type",
    "credits",
    "limitId",
    "limitName",
    "planType",
    "rateLimitReachedType",
    "individual_limit",
    "individualLimit",
    "rateLimitResetCredits",
];

/// Reset timestamps within this many seconds of the past are treated as current,
/// leaving room for clock skew against a remote host.
pub(super) const RESET_STALE_GRACE_SECONDS: u64 = 60;

pub(super) fn parse_rate_limits(value: Option<&Value>, now: u64) -> Vec<AgentRateLimitMetric> {
    let Some(Value::Object(limits)) = value else {
        return Vec::new();
    };
    let mut parsed = Vec::new();
    parse_rate_limit_entries(limits, None, now, &mut parsed);
    parsed
}

pub(super) fn parse_rate_limit_entries(
    limits: &serde_json::Map<String, Value>,
    group: Option<&str>,
    now: u64,
    parsed: &mut Vec<AgentRateLimitMetric>,
) {
    for (label, limit) in limits {
        let Value::Object(object) = limit else {
            continue;
        };
        if NON_QUOTA_LIMIT_KEYS.contains(&label.as_str()) {
            continue;
        }
        let remaining_fraction = value_f64(limit, "remaining_fraction")
            .or_else(|| value_f64(limit, "remainingFraction"));
        let explicit_remaining_percent = value_f64(limit, "remaining_percentage")
            .or_else(|| value_f64(limit, "remainingPercentage"))
            .or_else(|| value_f64(limit, "remaining_percent"))
            .or_else(|| value_f64(limit, "remainingPercent"));
        let used_percent = value_f64(limit, "used_percent")
            .or_else(|| value_f64(limit, "usedPercent"))
            .or_else(|| value_f64(limit, "used_percentage"))
            .or_else(|| value_f64(limit, "usedPercentage"))
            .or_else(|| {
                explicit_remaining_percent.map(|remaining| (100.0 - remaining).clamp(0.0, 100.0))
            })
            .or_else(|| {
                remaining_fraction.map(|remaining| (100.0 - remaining * 100.0).clamp(0.0, 100.0))
            });
        let remaining_percent = explicit_remaining_percent
            .or_else(|| remaining_fraction.map(|remaining| remaining * 100.0))
            .or_else(|| used_percent.map(|used| 100.0 - used))
            .map(|remaining| remaining.clamp(0.0, 100.0));
        let resets_at = value_u64(limit, "resets_at")
            .or_else(|| value_u64(limit, "resetsAt"))
            .or_else(|| value_u64(limit, "reset_at"))
            .or_else(|| value_u64(limit, "resetAt"))
            .or_else(|| value_u64(limit, "refreshes_at"))
            .or_else(|| value_u64(limit, "refreshesAt"))
            .map(epoch_seconds)
            // Some providers report the window relative to now instead.
            .or_else(|| {
                value_u64(limit, "resets_in_seconds")
                    .or_else(|| value_u64(limit, "resetsInSeconds"))
                    .or_else(|| value_u64(limit, "reset_after_seconds"))
                    .or_else(|| value_u64(limit, "resetAfterSeconds"))
                    .map(|seconds| now.saturating_add(seconds))
            });
        let window_minutes = value_u64(limit, "window_minutes")
            .or_else(|| value_u64(limit, "windowMinutes"))
            .or_else(|| value_u64(limit, "window_duration_mins"))
            .or_else(|| value_u64(limit, "windowDurationMins"))
            .or_else(|| infer_rate_limit_window(label));

        if used_percent.is_some() || resets_at.is_some() {
            // A window that already rolled over makes its recorded balance
            // meaningless; the UI reports the reset rather than a stale number.
            let stale = resets_at.is_some_and(|resets_at| {
                now > 0 && resets_at.saturating_add(RESET_STALE_GRACE_SECONDS) < now
            });
            parsed.push(AgentRateLimitMetric {
                label: label.clone(),
                group: group.map(str::to_owned),
                model_names: Vec::new(),
                remaining_percent,
                used_percent,
                window_minutes,
                resets_at,
                stale,
            });
            continue;
        }

        let nested_group = value_string(limit, "display_name")
            .or_else(|| value_string(limit, "displayName"))
            .or_else(|| value_string(limit, "name"))
            .unwrap_or_else(|| label.clone());
        parse_rate_limit_entries(object, Some(&nested_group), now, parsed);
    }
}

pub(super) fn now_epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// Normalizes a provider timestamp to epoch **seconds**.
///
/// Providers disagree about the unit, and everything downstream — the staleness
/// check here and both reset renderers in the webview — assumes seconds. Doing
/// the conversion once at the parse boundary keeps them from diverging. The
/// threshold is roughly the year 2286 in seconds, far past any real reset time
/// and far below any millisecond timestamp.
pub(super) fn epoch_seconds(value: u64) -> u64 {
    const MILLISECOND_THRESHOLD: u64 = 10_000_000_000;
    if value >= MILLISECOND_THRESHOLD {
        value / 1000
    } else {
        value
    }
}

pub(super) fn event_epoch_seconds(value: &Value) -> Option<u64> {
    let raw = value_string(value, "timestamp")
        .or_else(|| value_string(value, "captured_at"))
        .or_else(|| value_string(value, "capturedAt"))?;
    chrono::DateTime::parse_from_rfc3339(&raw)
        .ok()
        .and_then(|value| u64::try_from(value.timestamp()).ok())
}

pub(super) fn infer_rate_limit_window(label: &str) -> Option<u64> {
    let normalized = label
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    if normalized.contains("fivehour") || normalized.contains("5hour") {
        Some(5 * 60)
    } else if normalized.contains("sevenday")
        || normalized.contains("7day")
        || normalized.contains("weekly")
        || normalized == "week"
    {
        Some(7 * 24 * 60)
    } else {
        None
    }
}

pub(super) fn strip_terminal_control(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            0x1b if index + 1 < bytes.len() && bytes[index + 1] == b'[' => {
                index += 2;
                let mut final_byte = None;
                while index < bytes.len() {
                    let byte = bytes[index];
                    index += 1;
                    if (0x40..=0x7e).contains(&byte) {
                        final_byte = Some(byte);
                        break;
                    }
                }
                if final_byte.is_some_and(|byte| matches!(byte, b'A' | b'B' | b'E' | b'F' | b'H' | b'f'))
                    && !output.ends_with(b"\n")
                {
                    output.push(b'\n');
                }
            }
            0x1b if index + 1 < bytes.len() && bytes[index + 1] == b']' => {
                index += 2;
                while index < bytes.len() {
                    if bytes[index] == 0x07 {
                        index += 1;
                        break;
                    }
                    if bytes[index] == 0x1b && index + 1 < bytes.len() && bytes[index + 1] == b'\\'
                    {
                        index += 2;
                        break;
                    }
                    index += 1;
                }
            }
            0x1b => {
                index += if index + 1 < bytes.len() { 2 } else { 1 };
            }
            b'\r' => {
                output.push(b'\n');
                index += 1;
            }
            0x08 | 0x7f => {
                output.pop();
                index += 1;
            }
            byte if byte == b'\n' || byte == b'\t' || byte >= 0x20 => {
                output.push(byte);
                index += 1;
            }
            _ => index += 1,
        }
    }
    String::from_utf8_lossy(&output).into_owned()
}

pub(super) fn parse_agy_usage_output(input: &str, now: u64) -> Result<AgentQuotaSnapshot, String> {
    let cleaned = strip_terminal_control(input);
    let mut group = None::<String>;
    let mut collecting_models = None::<String>;
    let mut model_text_by_group = HashMap::<String, String>::new();
    let mut pending = None::<PendingAgyLimit>;
    let mut limits_by_key = HashMap::<(String, u64), AgentRateLimitMetric>::new();
    let mut last_limit_key = None::<(String, u64)>;

    for raw_line in cleaned.lines() {
        let line = raw_line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            continue;
        }
        let uppercase = line.to_ascii_uppercase();
        if uppercase.contains("GEMINI MODELS") {
            group = Some("Gemini models".to_string());
            collecting_models = None;
            pending = None;
            continue;
        }
        if uppercase.contains("CLAUDE AND GPT MODELS") || uppercase.contains("CLAUDE & GPT MODELS")
        {
            group = Some("Claude and GPT models".to_string());
            collecting_models = None;
            pending = None;
            continue;
        }
        if uppercase.contains("MODELS WITHIN THIS GROUP") {
            if let Some(group) = group.clone() {
                let model_text = line
                    .split_once(':')
                    .map(|(_, models)| models.trim())
                    .unwrap_or_default();
                model_text_by_group.insert(group.clone(), model_text.to_string());
                collecting_models = Some(group);
            }
            continue;
        }
        if uppercase.contains("WEEKLY LIMIT") {
            collecting_models = None;
            pending = group.clone().map(|group| PendingAgyLimit {
                label: "weekly_limit".to_string(),
                group,
                window_minutes: 7 * 24 * 60,
                precise_remaining_percent: None,
                resets_at: None,
            });
            continue;
        } else if uppercase.contains("FIVE HOUR LIMIT") || uppercase.contains("5 HOUR LIMIT") {
            collecting_models = None;
            pending = group.clone().map(|group| PendingAgyLimit {
                label: "five_hour_limit".to_string(),
                group,
                window_minutes: 5 * 60,
                precise_remaining_percent: None,
                resets_at: None,
            });
            continue;
        }

        if let Some(model_group) = collecting_models.as_ref() {
            let model_text = model_text_by_group.entry(model_group.clone()).or_default();
            if !model_text.is_empty() {
                model_text.push(' ');
            }
            model_text.push_str(&line);
            continue;
        }

        let Some(pending_limit) = pending.as_mut() else {
            if let Some(seconds) = parse_refresh_seconds(&line) {
                if let Some(last) = last_limit_key
                    .as_ref()
                    .and_then(|key| limits_by_key.get_mut(key))
                {
                    last.resets_at = Some(now.saturating_add(seconds));
                }
            }
            continue;
        };

        if let Some(seconds) = parse_refresh_seconds(&line) {
            pending_limit.resets_at = Some(now.saturating_add(seconds));
        }
        let rounded_remaining = extract_remaining_percent(&line);
        let quota_available = line.to_ascii_lowercase().contains("quota available");
        if let Some(precise) = extract_display_percent(&line) {
            pending_limit.precise_remaining_percent = Some(precise.clamp(0.0, 100.0));
        }
        if rounded_remaining.is_none() && !quota_available {
            continue;
        }

        let remaining_percent = pending_limit
            .precise_remaining_percent
            .or(rounded_remaining)
            .or(quota_available.then_some(100.0))
            .map(|remaining| remaining.clamp(0.0, 100.0));
        let Some(remaining_percent) = remaining_percent else {
            continue;
        };
        let key = (
            pending_limit.group.clone(),
            pending_limit.window_minutes,
        );
        let model_names = model_text_by_group
            .get(&pending_limit.group)
            .map(|models| parse_agy_model_names(models))
            .unwrap_or_default();
        limits_by_key.insert(
            key.clone(),
            AgentRateLimitMetric {
                label: pending_limit.label.clone(),
                group: Some(pending_limit.group.clone()),
                model_names,
                remaining_percent: Some(remaining_percent),
                used_percent: Some((100.0 - remaining_percent).clamp(0.0, 100.0)),
                window_minutes: Some(pending_limit.window_minutes),
                resets_at: pending_limit.resets_at,
                stale: false,
            },
        );
        last_limit_key = Some(key);
        pending = None;
    }

    let mut limits = limits_by_key.into_values().collect::<Vec<_>>();
    limits.sort_by_key(|limit| {
        let group_order = match limit.group.as_deref() {
            Some("Gemini models") => 0,
            Some("Claude and GPT models") => 1,
            _ => 2,
        };
        let window_order = match limit.window_minutes {
            Some(10_080) => 0,
            Some(300) => 1,
            _ => 2,
        };
        (group_order, window_order)
    });
    if !has_complete_agy_quota(&limits) {
        return Err(
            "the AGY /usage layout did not contain both model groups and all four quota windows"
                .to_string(),
        );
    }
    Ok(AgentQuotaSnapshot::available(
        "agy-usage-tui",
        Some(now),
        limits,
        now,
    ))
}

pub(super) fn has_complete_agy_quota(limits: &[AgentRateLimitMetric]) -> bool {
    ["Gemini models", "Claude and GPT models"]
        .iter()
        .all(|group| {
            [300, 10_080].iter().all(|window| {
                limits.iter().any(|limit| {
                    limit.group.as_deref() == Some(*group)
                        && limit.window_minutes == Some(*window)
                })
            })
        })
}

pub(super) struct PendingAgyLimit {
    pub(super) label: String,
    pub(super) group: String,
    pub(super) window_minutes: u64,
    pub(super) precise_remaining_percent: Option<f64>,
    pub(super) resets_at: Option<u64>,
}

pub(super) fn parse_agy_model_names(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string)
        .collect()
}

pub(super) fn extract_display_percent(line: &str) -> Option<f64> {
    let lowercase = line.to_ascii_lowercase();
    line.match_indices('%')
        .filter(|(percent, _)| {
            !lowercase[percent + 1..]
                .trim_start()
                .starts_with("remaining")
        })
        .filter_map(|(percent, _)| parse_percent_before(line, percent))
        .next_back()
}

pub(super) fn parse_percent_before(line: &str, percent: usize) -> Option<f64> {
    let prefix = line[..percent].trim_end();
    let bytes = prefix.as_bytes();
    let mut start = bytes.len();
    while start > 0
        && (bytes[start - 1].is_ascii_digit() || matches!(bytes[start - 1], b'.' | b','))
    {
        start -= 1;
    }
    (start < bytes.len())
        .then(|| prefix[start..].replace(',', ".").parse::<f64>().ok())
        .flatten()
}

pub(super) fn extract_remaining_percent(line: &str) -> Option<f64> {
    let lowercase = line.to_ascii_lowercase();
    let end = lowercase.find("% remaining")?;
    let prefix = &line[..end];
    let bytes = prefix.as_bytes();
    let mut start = bytes.len();
    while start > 0
        && (bytes[start - 1].is_ascii_digit() || matches!(bytes[start - 1], b'.' | b','))
    {
        start -= 1;
    }
    prefix[start..].replace(',', ".").parse::<f64>().ok()
}

pub(super) fn parse_refresh_seconds(line: &str) -> Option<u64> {
    let lowercase = line.to_ascii_lowercase();
    let marker = lowercase.find("refreshes in")?;
    let tail = &lowercase[marker + "refreshes in".len()..];
    let mut total = 0_u64;
    let mut found = false;
    for token in tail.split_whitespace() {
        let trimmed = token.trim_matches(|character: char| !character.is_ascii_alphanumeric());
        let (digits, multiplier) = if let Some(value) = trimmed.strip_suffix('d') {
            (value, 86_400)
        } else if let Some(value) = trimmed.strip_suffix('h') {
            (value, 3_600)
        } else if let Some(value) = trimmed.strip_suffix('m') {
            (value, 60)
        } else if let Some(value) = trimmed.strip_suffix('s') {
            (value, 1)
        } else {
            continue;
        };
        if let Ok(value) = digits.parse::<u64>() {
            total = total.saturating_add(value.saturating_mul(multiplier));
            found = true;
        }
    }
    found.then_some(total)
}

pub(super) fn finalize_context(metadata: &mut AgentSessionMetadata) {
    if metadata.context_used_percent.is_none() {
        metadata.context_used_percent =
            ratio_percent(metadata.context_used_tokens, metadata.context_window_tokens);
    }
    if metadata.context_remaining_tokens.is_none() {
        metadata.context_remaining_tokens =
            match (metadata.context_window_tokens, metadata.context_used_tokens) {
                (Some(window), Some(used)) => Some(window.saturating_sub(used)),
                _ => None,
            };
    }
    if metadata.context_remaining_percent.is_none() {
        metadata.context_remaining_percent = metadata
            .context_used_percent
            .map(|used| (100.0 - used).clamp(0.0, 100.0))
            .or_else(|| {
                ratio_percent(
                    metadata.context_remaining_tokens,
                    metadata.context_window_tokens,
                )
            });
    }
}

pub(super) fn parse_work_items(items: &[Value]) -> Vec<AgentWorkMetric> {
    items
        .iter()
        .filter_map(|item| {
            let name = value_string(item, "name")
                .or_else(|| value_string(item, "id"))
                .or_else(|| value_string(item, "title"))?;
            Some(AgentWorkMetric {
                name,
                role: value_string(item, "role"),
                status: value_string(item, "status"),
            })
        })
        .take(32)
        .collect()
}

pub(super) fn pointer_string(value: &Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .and_then(nonempty)
}

pub(super) fn value_string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).and_then(nonempty)
}

pub(super) fn value_u64(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(number_u64)
}

pub(super) fn value_f64(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(number_f64)
}

pub(super) fn number_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| {
            value
                .as_i64()
                .filter(|number| *number >= 0)
                .map(|number| number as u64)
        })
        .or_else(|| value.as_str()?.trim().parse().ok())
}

pub(super) fn number_f64(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.trim().parse().ok())
}

pub(super) fn nonempty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && value != "null").then(|| value.to_string())
}

pub(super) fn ratio_percent(used: Option<u64>, total: Option<u64>) -> Option<f64> {
    match (used, total) {
        (Some(used), Some(total)) if total > 0 => {
            Some((used as f64 / total as f64 * 100.0).clamp(0.0, 100.0))
        }
        _ => None,
    }
}

pub(super) fn parse_elapsed(value: &str) -> Option<u64> {
    let (days, clock) = if let Some((days, clock)) = value.split_once('-') {
        (days.parse::<u64>().ok()?, clock)
    } else {
        (0, value)
    };
    let fields = clock
        .split(':')
        .filter_map(|field| field.parse::<u64>().ok())
        .collect::<Vec<_>>();
    let seconds = match fields.as_slice() {
        [minutes, seconds] => minutes * 60 + seconds,
        [hours, minutes, seconds] => hours * 3600 + minutes * 60 + seconds,
        _ => return None,
    };
    Some(days * 86_400 + seconds)
}
