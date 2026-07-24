//! Read-only monitoring for terminal coding agents.
//!
//! Process trees provide the reliable cross-platform baseline. Provider
//! session files are sampled conservatively for non-sensitive metadata only:
//! IDs, model names, token/context counters, cost/duration, rate-limit
//! snapshots, and AGY worker state. Prompt, response, tool input/output, and
//! authentication fields are never serialized into GpuTerm telemetry.

use crate::ssh::system_monitor::{run_local_command_for, run_remote_command_for, RemoteOs};
use serde::Serialize;
use serde_json::Value;
use ssh2::Session;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

const METADATA_REFRESH_INTERVAL: Duration = Duration::from_secs(5);

const POSIX_PROCESS_COMMAND: &str =
    "LC_ALL=C ps -axo pid=,ppid=,user=,%cpu=,rss=,etime=,comm=,args= 2>/dev/null || true";

const WINDOWS_PROCESS_COMMAND: &str = r#"$ErrorActionPreference='SilentlyContinue'
$logical = [Math]::Max(1, [int](Get-CimInstance Win32_ComputerSystem).NumberOfLogicalProcessors)
$byId = @{}
Get-Process | ForEach-Object { $byId[[int]$_.Id] = $_ }
$rows = Get-CimInstance Win32_Process | ForEach-Object {
  $p = $byId[[int]$_.ProcessId]
  $elapsed = $null
  if ($_.CreationDate) { $elapsed = [Math]::Max(0, [int64]((Get-Date) - $_.CreationDate).TotalSeconds) }
  [pscustomobject]@{
    pid = [int]$_.ProcessId
    ppid = [int]$_.ParentProcessId
    name = [string]$_.Name
    commandLine = [string]$_.CommandLine
    executablePath = [string]$_.ExecutablePath
    cpuSeconds = if ($p) { [double]$p.CPU } else { $null }
    rssBytes = if ($p) { [int64]$p.WorkingSet64 } else { $null }
    elapsedSeconds = $elapsed
  }
}
[pscustomobject]@{ logicalCores = $logical; processes = @($rows) } | ConvertTo-Json -Depth 4 -Compress
exit 0"#;

const POSIX_METADATA_COMMAND: &str = r#"emit_agent_files() {
  provider="$1"
  root="$2"
  pattern="$3"
  [ -d "$root" ] || return 0
  find "$root" -type f -name "$pattern" -exec ls -t {} + 2>/dev/null | head -n 2 |
  while IFS= read -r file; do
    [ -r "$file" ] || continue
    printf '__GPUTERM_AGENT_FILE__\t%s\t%s\n' "$provider" "$file"
    head -n 1 "$file" 2>/dev/null
    tail -c 131072 "$file" 2>/dev/null
    printf '\n__GPUTERM_AGENT_END__\n'
  done
}
emit_agent_files codex "$HOME/.codex/sessions" 'rollout-*.jsonl'
emit_agent_files claude "$HOME/.claude/projects" '*.jsonl'
emit_agent_files agy "$HOME/.gemini/antigravity-cli/brain" 'transcript.jsonl'
for provider in agy claude; do
  snapshot="$HOME/.cache/gputerm/agent-status/$provider.json"
  if [ -r "$snapshot" ]; then
    printf '__GPUTERM_AGENT_FILE__\t%s\t%s\n' "$provider" "$snapshot"
    tail -c 131072 "$snapshot" 2>/dev/null
    printf '\n__GPUTERM_AGENT_END__\n'
  fi
done
true"#;

const WINDOWS_METADATA_COMMAND: &str = r#"$ErrorActionPreference='SilentlyContinue'
function Emit-AgentFiles([string]$provider, [string]$root, [string]$filter) {
  if (-not (Test-Path -LiteralPath $root)) { return }
  Get-ChildItem -LiteralPath $root -Recurse -File -Filter $filter |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 2 |
    ForEach-Object {
      Write-Output ("__GPUTERM_AGENT_FILE__`t{0}`t{1}" -f $provider, $_.FullName)
      Get-Content -LiteralPath $_.FullName -TotalCount 1 -ErrorAction SilentlyContinue
      Get-Content -LiteralPath $_.FullName -Tail 300 -ErrorAction SilentlyContinue
      Write-Output '__GPUTERM_AGENT_END__'
    }
}
Emit-AgentFiles 'codex' (Join-Path $HOME '.codex\sessions') 'rollout-*.jsonl'
Emit-AgentFiles 'claude' (Join-Path $HOME '.claude\projects') '*.jsonl'
Emit-AgentFiles 'agy' (Join-Path $HOME '.gemini\antigravity-cli\brain') 'transcript.jsonl'
foreach ($provider in @('agy', 'claude')) {
  $snapshot = Join-Path $HOME ".cache\gputerm\agent-status\$provider.json"
  if (Test-Path -LiteralPath $snapshot) {
    Write-Output ("__GPUTERM_AGENT_FILE__`t{0}`t{1}" -f $provider, $snapshot)
    Get-Content -LiteralPath $snapshot -Tail 300 -ErrorAction SilentlyContinue
    Write-Output '__GPUTERM_AGENT_END__'
  }
}
exit 0"#;

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRateLimitMetric {
    label: String,
    used_percent: Option<f64>,
    window_minutes: Option<u64>,
    resets_at: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentWorkMetric {
    name: String,
    role: Option<String>,
    status: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMetric {
    provider: String,
    display_name: String,
    status: String,
    root_pid: u32,
    process_count: u32,
    user: Option<String>,
    cpu_percent: Option<f64>,
    memory_bytes: Option<u64>,
    elapsed_seconds: Option<u64>,
    session_id: Option<String>,
    cwd: Option<String>,
    model: Option<String>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    total_tokens: Option<u64>,
    context_used_tokens: Option<u64>,
    context_window_tokens: Option<u64>,
    context_used_percent: Option<f64>,
    cost_usd: Option<f64>,
    session_duration_seconds: Option<f64>,
    rate_limits: Vec<AgentRateLimitMetric>,
    subagents: Vec<AgentWorkMetric>,
    background_tasks: Vec<AgentWorkMetric>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Provider {
    Agy,
    Codex,
    Claude,
}

impl Provider {
    fn key(self) -> &'static str {
        match self {
            Self::Agy => "agy",
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }

    fn display_name(self) -> &'static str {
        match self {
            Self::Agy => "Antigravity",
            Self::Codex => "Codex",
            Self::Claude => "Claude Code",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "agy" | "antigravity" => Some(Self::Agy),
            "codex" => Some(Self::Codex),
            "claude" | "claude-code" => Some(Self::Claude),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct ProcessSample {
    pid: u32,
    ppid: u32,
    user: Option<String>,
    cpu_percent: Option<f64>,
    rss_bytes: Option<u64>,
    elapsed_seconds: Option<u64>,
    name: String,
    command: String,
}

#[derive(Debug, Clone, Default)]
struct AgentSessionMetadata {
    session_id: Option<String>,
    cwd: Option<String>,
    model: Option<String>,
    status: Option<String>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    total_tokens: Option<u64>,
    context_used_tokens: Option<u64>,
    context_window_tokens: Option<u64>,
    context_used_percent: Option<f64>,
    cost_usd: Option<f64>,
    session_duration_seconds: Option<f64>,
    rate_limits: Vec<AgentRateLimitMetric>,
    subagents: Vec<AgentWorkMetric>,
    background_tasks: Vec<AgentWorkMetric>,
}

#[derive(Default)]
pub struct AgentMonitorState {
    last_metadata_scan: Option<Instant>,
    metadata: HashMap<Provider, Vec<AgentSessionMetadata>>,
    windows_cpu: HashMap<u32, f64>,
    windows_cpu_sampled_at: Option<Instant>,
}

pub fn collect_remote_agents(
    session: &Session,
    os: RemoteOs,
    state: &mut AgentMonitorState,
) -> Result<Vec<AgentMetric>, String> {
    let command = if os == RemoteOs::Windows {
        WINDOWS_PROCESS_COMMAND
    } else {
        POSIX_PROCESS_COMMAND
    };
    let output = run_remote_command_for(session, os, command)?;
    let processes = parse_processes(os, &output, state)?;
    if !processes
        .iter()
        .any(|process| provider_for_process(process).is_some())
    {
        return Ok(Vec::new());
    }
    refresh_metadata_if_due(state, os, |command| {
        run_remote_command_for(session, os, command)
    });
    Ok(build_agent_metrics(&processes, &state.metadata))
}

pub fn collect_local_agents(
    os: RemoteOs,
    state: &mut AgentMonitorState,
) -> Result<Vec<AgentMetric>, String> {
    let command = if os == RemoteOs::Windows {
        WINDOWS_PROCESS_COMMAND
    } else {
        POSIX_PROCESS_COMMAND
    };
    let output = run_local_command_for(os, command)?;
    let processes = parse_processes(os, &output, state)?;
    if !processes
        .iter()
        .any(|process| provider_for_process(process).is_some())
    {
        return Ok(Vec::new());
    }
    refresh_metadata_if_due(state, os, |command| run_local_command_for(os, command));
    Ok(build_agent_metrics(&processes, &state.metadata))
}

fn refresh_metadata_if_due<F>(state: &mut AgentMonitorState, os: RemoteOs, run: F)
where
    F: FnOnce(&str) -> Result<String, String>,
{
    let due = state
        .last_metadata_scan
        .map(|last| last.elapsed() >= METADATA_REFRESH_INTERVAL)
        .unwrap_or(true);
    if !due {
        return;
    }
    state.last_metadata_scan = Some(Instant::now());
    let command = if os == RemoteOs::Windows {
        WINDOWS_METADATA_COMMAND
    } else {
        POSIX_METADATA_COMMAND
    };
    if let Ok(output) = run(command) {
        state.metadata = parse_metadata_output(&output);
    }
}

fn parse_processes(
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

fn parse_posix_processes(output: &str) -> Vec<ProcessSample> {
    output
        .lines()
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 8 {
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
                name: fields[6].to_string(),
                command: fields[7..].join(" "),
            })
        })
        .collect()
}

fn parse_windows_processes(
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
            Some(ProcessSample {
                pid,
                ppid: value_u64(row, "ppid").unwrap_or(0) as u32,
                cpu_percent,
                rss_bytes: value_u64(row, "rssBytes"),
                elapsed_seconds: value_u64(row, "elapsedSeconds"),
                name,
                command,
                ..Default::default()
            })
        })
        .collect();
    state.windows_cpu = next_cpu;
    state.windows_cpu_sampled_at = Some(now);
    Ok(samples)
}

fn provider_for_process(process: &ProcessSample) -> Option<Provider> {
    let executable = process
        .name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(&process.name)
        .trim_end_matches(".exe")
        .to_ascii_lowercase();
    let command = process.command.to_ascii_lowercase();

    if executable == "agy" {
        return Some(Provider::Agy);
    }
    if executable == "codex"
        || command.contains("@openai/codex")
        || command.contains("/codex/bin/codex")
        || command.contains("\\codex\\bin\\codex")
    {
        return Some(Provider::Codex);
    }
    if executable == "claude"
        || command.contains("@anthropic-ai/claude-code")
        || command.contains("/claude-code/cli.js")
        || command.contains("\\claude-code\\cli.js")
    {
        return Some(Provider::Claude);
    }
    None
}

fn build_agent_metrics(
    processes: &[ProcessSample],
    metadata: &HashMap<Provider, Vec<AgentSessionMetadata>>,
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
    let mut provider_offsets = HashMap::<Provider, usize>::new();
    roots
        .into_iter()
        .filter_map(|(root_pid, provider)| {
            let root = by_pid.get(&root_pid)?;
            let mut stack = vec![root_pid];
            let mut included = Vec::new();
            while let Some(pid) = stack.pop() {
                included.push(pid);
                if let Some(child_pids) = children.get(&pid) {
                    for child in child_pids {
                        if *child != root_pid && root_pids.contains(child) {
                            continue;
                        }
                        stack.push(*child);
                    }
                }
            }
            let cpu_values = included
                .iter()
                .filter_map(|pid| by_pid.get(pid).and_then(|process| process.cpu_percent))
                .collect::<Vec<_>>();
            let memory_values = included
                .iter()
                .filter_map(|pid| by_pid.get(pid).and_then(|process| process.rss_bytes))
                .collect::<Vec<_>>();
            let offset = provider_offsets.entry(provider).or_default();
            let provider_metadata = metadata
                .get(&provider)
                .and_then(|sessions| sessions.get(*offset))
                .cloned()
                .unwrap_or_default();
            *offset += 1;
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
                cost_usd: provider_metadata.cost_usd,
                // Claude status-line snapshots expose an API/session duration.
                // The process elapsed time remains a useful read-only fallback.
                session_duration_seconds: provider_metadata
                    .session_duration_seconds
                    .or_else(|| root.elapsed_seconds.map(|seconds| seconds as f64)),
                rate_limits: provider_metadata.rate_limits,
                subagents: provider_metadata.subagents,
                background_tasks: provider_metadata.background_tasks,
            })
        })
        .collect()
}

fn parse_metadata_output(output: &str) -> HashMap<Provider, Vec<AgentSessionMetadata>> {
    let mut grouped = HashMap::<Provider, Vec<AgentSessionMetadata>>::new();
    let mut provider = None;
    let mut lines = Vec::<String>::new();
    for line in output.lines() {
        if let Some(marker) = line.strip_prefix("__GPUTERM_AGENT_FILE__\t") {
            if let Some(current) = provider.take() {
                grouped
                    .entry(current)
                    .or_default()
                    .push(parse_provider_metadata(current, &lines));
            }
            let key = marker.split('\t').next().unwrap_or("");
            provider = Provider::parse(key);
            lines.clear();
        } else if line.trim() == "__GPUTERM_AGENT_END__" {
            if let Some(current) = provider.take() {
                grouped
                    .entry(current)
                    .or_default()
                    .push(parse_provider_metadata(current, &lines));
            }
            lines.clear();
        } else if provider.is_some() {
            lines.push(line.to_string());
        }
    }
    if let Some(current) = provider {
        grouped
            .entry(current)
            .or_default()
            .push(parse_provider_metadata(current, &lines));
    }
    grouped
}

fn parse_provider_metadata(provider: Provider, lines: &[String]) -> AgentSessionMetadata {
    let values = lines
        .iter()
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .collect::<Vec<_>>();
    match provider {
        Provider::Codex => parse_codex_metadata(&values),
        Provider::Claude => parse_claude_metadata(&values),
        Provider::Agy => parse_agy_metadata(&values),
    }
}

fn parse_codex_metadata(values: &[Value]) -> AgentSessionMetadata {
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
            metadata.rate_limits = parse_rate_limits(payload.get("rate_limits"));
        }
        if event_type == "turn_context" {
            metadata.model = value_string(payload, "model").or(metadata.model);
        }
    }
    metadata
}

fn parse_claude_metadata(values: &[Value]) -> AgentSessionMetadata {
    let mut metadata = AgentSessionMetadata::default();
    let mut seen_messages = HashSet::new();
    let mut input = 0_u64;
    let mut output = 0_u64;
    let mut saw_usage = false;
    for value in values {
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
        let message = value.get("message").unwrap_or(value);
        let message_id = value_string(message, "id");
        if message_id
            .as_ref()
            .is_some_and(|id| !seen_messages.insert(id.clone()))
        {
            continue;
        }
        if let Some(usage) = message.get("usage") {
            input = input.saturating_add(value_u64(usage, "input_tokens").unwrap_or(0));
            input =
                input.saturating_add(value_u64(usage, "cache_creation_input_tokens").unwrap_or(0));
            input = input.saturating_add(value_u64(usage, "cache_read_input_tokens").unwrap_or(0));
            output = output.saturating_add(value_u64(usage, "output_tokens").unwrap_or(0));
            saw_usage = true;
            metadata.context_used_tokens = value_u64(usage, "input_tokens")
                .and_then(|base| {
                    base.checked_add(value_u64(usage, "cache_creation_input_tokens").unwrap_or(0))
                })
                .and_then(|base| {
                    base.checked_add(value_u64(usage, "cache_read_input_tokens").unwrap_or(0))
                });
        }
        let context = value
            .get("context_window")
            .or_else(|| value.get("contextWindow"))
            .unwrap_or(value);
        metadata.input_tokens = value_u64(context, "total_input_tokens")
            .or_else(|| value_u64(context, "totalInputTokens"))
            .or(metadata.input_tokens);
        metadata.output_tokens = value_u64(context, "total_output_tokens")
            .or_else(|| value_u64(context, "totalOutputTokens"))
            .or(metadata.output_tokens);
        metadata.context_window_tokens = value_u64(context, "context_window_size")
            .or_else(|| value_u64(context, "contextWindowSize"))
            .or(metadata.context_window_tokens);
        metadata.context_used_tokens = value_u64(context, "total_input_tokens")
            .or_else(|| value_u64(context, "totalInputTokens"))
            .or(metadata.context_used_tokens);
        metadata.context_used_percent = value_f64(context, "used_percentage")
            .or_else(|| value_f64(context, "usedPercentage"))
            .or(metadata.context_used_percent);
    }
    if saw_usage {
        metadata.input_tokens = Some(input);
        metadata.output_tokens = Some(output);
    }
    metadata.total_tokens = match (metadata.input_tokens, metadata.output_tokens) {
        (Some(input), Some(output)) => Some(input.saturating_add(output)),
        _ => None,
    };
    if metadata.context_used_percent.is_none() {
        metadata.context_used_percent =
            ratio_percent(metadata.context_used_tokens, metadata.context_window_tokens);
    }
    metadata
}

fn parse_agy_metadata(values: &[Value]) -> AgentSessionMetadata {
    let mut metadata = AgentSessionMetadata::default();
    for value in values {
        let payload = value.get("payload").unwrap_or(value);
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
        let context = payload
            .get("context_window")
            .or_else(|| payload.get("contextWindow"))
            .unwrap_or(payload);
        metadata.input_tokens = value_u64(context, "total_input_tokens")
            .or_else(|| value_u64(context, "totalInputTokens"))
            .or(metadata.input_tokens);
        metadata.output_tokens = value_u64(context, "total_output_tokens")
            .or_else(|| value_u64(context, "totalOutputTokens"))
            .or(metadata.output_tokens);
        metadata.context_window_tokens = value_u64(context, "context_window_size")
            .or_else(|| value_u64(context, "contextWindowSize"))
            .or(metadata.context_window_tokens);
        metadata.context_used_percent = value_f64(context, "used_percentage")
            .or_else(|| value_f64(context, "usedPercentage"))
            .or(metadata.context_used_percent);
        metadata.context_used_tokens = value_u64(context, "context_used_tokens")
            .or_else(|| value_u64(context, "contextUsedTokens"))
            .or_else(|| value_u64(context, "input_tokens"))
            .or(metadata.context_used_tokens);
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
    metadata.total_tokens = match (metadata.input_tokens, metadata.output_tokens) {
        (Some(input), Some(output)) => Some(input.saturating_add(output)),
        _ => None,
    };
    if metadata.context_used_percent.is_none() {
        metadata.context_used_percent =
            ratio_percent(metadata.context_used_tokens, metadata.context_window_tokens);
    }
    metadata
}

fn parse_rate_limits(value: Option<&Value>) -> Vec<AgentRateLimitMetric> {
    let Some(Value::Object(limits)) = value else {
        return Vec::new();
    };
    ["primary", "secondary"]
        .into_iter()
        .filter_map(|label| {
            let limit = limits.get(label)?;
            Some(AgentRateLimitMetric {
                label: label.to_string(),
                used_percent: value_f64(limit, "used_percent")
                    .or_else(|| value_f64(limit, "usedPercent")),
                window_minutes: value_u64(limit, "window_minutes")
                    .or_else(|| value_u64(limit, "windowMinutes")),
                resets_at: value_u64(limit, "resets_at").or_else(|| value_u64(limit, "resetsAt")),
            })
        })
        .collect()
}

fn parse_work_items(items: &[Value]) -> Vec<AgentWorkMetric> {
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

fn pointer_string(value: &Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .and_then(nonempty)
}

fn value_string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).and_then(nonempty)
}

fn value_u64(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(number_u64)
}

fn value_f64(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(number_f64)
}

fn number_u64(value: &Value) -> Option<u64> {
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

fn number_f64(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.trim().parse().ok())
}

fn nonempty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && value != "null").then(|| value.to_string())
}

fn ratio_percent(used: Option<u64>, total: Option<u64>) -> Option<f64> {
    match (used, total) {
        (Some(used), Some(total)) if total > 0 => {
            Some((used as f64 / total as f64 * 100.0).clamp(0.0, 100.0))
        }
        _ => None,
    }
}

fn parse_elapsed(value: &str) -> Option<u64> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_agents_and_aggregates_child_resources() {
        let processes = parse_posix_processes(
            "100 1 alice 2.0 100000 01:00 codex /usr/bin/codex\n\
             101 100 alice 4.0 50000 00:30 node node worker.js\n\
             200 1 bob 1.0 70000 00:20 node node /x/@anthropic-ai/claude-code/cli.js\n",
        );
        let metrics = build_agent_metrics(&processes, &HashMap::new());
        assert_eq!(metrics.len(), 2);
        let codex = metrics
            .iter()
            .find(|metric| metric.provider == "codex")
            .unwrap();
        assert_eq!(codex.process_count, 2);
        assert_eq!(codex.cpu_percent, Some(6.0));
        assert_eq!(codex.memory_bytes, Some(150_000 * 1024));
    }

    #[test]
    fn parses_codex_token_and_rate_limit_snapshot() {
        let lines = vec![
            r#"{"type":"session_meta","payload":{"id":"session-1","cwd":"/work","model_provider":"openai"}}"#.to_string(),
            r#"{"type":"turn_context","payload":{"model":"gpt-5.4"}}"#.to_string(),
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1200,"output_tokens":300,"total_tokens":1500},"last_token_usage":{"total_tokens":500},"model_context_window":10000},"rate_limits":{"primary":{"used_percent":42,"window_minutes":300,"resets_at":1234}}}}"#.to_string(),
        ];
        let metadata = parse_provider_metadata(Provider::Codex, &lines);
        assert_eq!(metadata.session_id.as_deref(), Some("session-1"));
        assert_eq!(metadata.model.as_deref(), Some("gpt-5.4"));
        assert_eq!(metadata.total_tokens, Some(1500));
        assert_eq!(metadata.context_used_percent, Some(5.0));
        assert_eq!(metadata.rate_limits[0].used_percent, Some(42.0));
    }

    #[test]
    fn parses_agy_status_payload_without_prompt_content() {
        let lines = vec![r#"{"conversation_id":"agy-1","model":{"display_name":"Gemini"},"agent_state":"working","context_window":{"total_input_tokens":80,"total_output_tokens":20,"context_window_size":1000,"used_percentage":10},"subagents":[{"name":"tests","role":"worker","status":"active"}],"background_tasks":[{"name":"npm test","status":"running"}],"prompt":"do not expose this prompt"}"#.to_string()];
        let metadata = parse_provider_metadata(Provider::Agy, &lines);
        assert_eq!(metadata.status.as_deref(), Some("working"));
        assert_eq!(metadata.model.as_deref(), Some("Gemini"));
        assert_eq!(metadata.total_tokens, Some(100));
        assert_eq!(metadata.subagents.len(), 1);
        assert_eq!(metadata.background_tasks.len(), 1);

        let processes =
            parse_posix_processes("100 1 alice 2.0 100000 01:00 agy agy --api-key do-not-expose\n");
        let mut sessions = HashMap::new();
        sessions.insert(Provider::Agy, vec![metadata]);
        let serialized =
            serde_json::to_string(&build_agent_metrics(&processes, &sessions)).unwrap();
        assert!(!serialized.contains("do not expose this prompt"));
        assert!(!serialized.contains("do-not-expose"));
    }

    #[test]
    fn parses_claude_usage_without_double_counting_duplicate_messages() {
        let lines = vec![
            r#"{"sessionId":"claude-1","message":{"id":"msg-1","model":"claude-sonnet","usage":{"input_tokens":100,"cache_read_input_tokens":50,"output_tokens":20}}}"#.to_string(),
            r#"{"sessionId":"claude-1","message":{"id":"msg-1","model":"claude-sonnet","usage":{"input_tokens":100,"cache_read_input_tokens":50,"output_tokens":20}}}"#.to_string(),
            r#"{"total_cost_usd":0.42,"total_duration_ms":5000}"#.to_string(),
        ];
        let metadata = parse_provider_metadata(Provider::Claude, &lines);
        assert_eq!(metadata.input_tokens, Some(150));
        assert_eq!(metadata.output_tokens, Some(20));
        assert_eq!(metadata.cost_usd, Some(0.42));
        assert_eq!(metadata.session_duration_seconds, Some(5.0));
    }

    #[test]
    fn parses_elapsed_time_formats() {
        assert_eq!(parse_elapsed("01:30"), Some(90));
        assert_eq!(parse_elapsed("02:01:30"), Some(7290));
        assert_eq!(parse_elapsed("3-02:01:30"), Some(266_490));
    }
}
