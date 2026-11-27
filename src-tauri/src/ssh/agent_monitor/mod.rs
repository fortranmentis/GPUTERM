//! Read-only monitoring for terminal coding agents.
//!
//! Process trees provide the reliable cross-platform baseline. Provider
//! session files are sampled conservatively for non-sensitive metadata only:
//! IDs, model names, token/context counters, cost/duration, rate-limit
//! snapshots, and AGY worker state. Prompt, response, tool input/output, and
//! authentication fields are never serialized into GpuTerm telemetry.

mod parsers;
mod quota;
mod statusline;
#[cfg(test)]
mod tests;
mod types;

#[allow(unused_imports)]
pub use types::{
    AgentMetric, AgentMonitorState, AgentQuotaHistories, AgentQuotaHistoryLimit,
    AgentQuotaHistoryPoint, AgentQuotaSnapshot, AgentRateLimitMetric, AgentWorkMetric,
    ClaudeQuotaSetupResult,
};

use crate::ssh::session::{target_for_active_session, with_ops_session, AppState, SshTarget};
use crate::ssh::system_monitor::{
    detect_remote_os, run_local_command_for, run_local_command_with_timeout,
    run_remote_command_for, run_remote_command_with_budget, RemoteOs,
};
use parsers::{
    build_agent_metrics, detected_providers, now_epoch_seconds, parse_metadata_output,
    parse_processes,
};
use quota::{
    merge_metadata_quota_fallbacks, refresh_provider_quotas_local, refresh_provider_quotas_remote,
    update_quota_snapshots,
};
use ssh2::Session;
use statusline::{
    claude_helper_for_os, configure_claude_quota_local, configure_claude_quota_remote,
};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::State;
use types::Provider;

const METADATA_REFRESH_INTERVAL: Duration = Duration::from_secs(5);
/// Time budget for one metadata scrape. Larger than the telemetry default
/// because the scrape walks provider session directories.
const METADATA_COMMAND_TIMEOUT_SECS: u64 = 10;

// `comm` is deliberately absent: macOS pads that column to sixteen characters,
// which turns an absolute executable path into an unusable fragment. `args` is
// the final column, so it is the one field that always arrives complete.
const POSIX_PROCESS_COMMAND: &str =
    "LC_ALL=C ps -axo pid=,ppid=,user=,%cpu=,rss=,etime=,args= 2>/dev/null || true";

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

const POSIX_METADATA_PRELUDE: &str = r#"emit_agent_tail() {
  provider="$1"
  file="$2"
  [ -r "$file" ] || return 0
  printf '__GPUTERM_AGENT_FILE__\t%s\t%s\n' "$provider" "$file"
  tail -c 131072 "$file" 2>/dev/null
  printf '\n__GPUTERM_AGENT_END__\n'
}
emit_agent_files() {
  provider="$1"
  root="$2"
  pattern="$3"
  depth="$4"
  [ -d "$root" ] || return 0
  # Subagent transcripts live under */subagents/ and describe a worker's own
  # context rather than the session the user started. Excluding them keeps a
  # worker's counters from overwriting the parent session's. The depth bound
  # keeps the walk proportional to the provider's layout instead of the whole
  # tree, which on a long-lived host is most of the scrape cost.
  find "$root" -maxdepth "$depth" -type f -name "$pattern" ! -path '*/subagents/*' -exec ls -t {} + 2>/dev/null | head -n 2 |
  while IFS= read -r file; do
    [ -r "$file" ] || continue
    printf '__GPUTERM_AGENT_FILE__\t%s\t%s\n' "$provider" "$file"
    head -n 1 "$file" 2>/dev/null
    tail -c 131072 "$file" 2>/dev/null
    printf '\n__GPUTERM_AGENT_END__\n'
  done
}
# Reports which setup step is incomplete when no quota has been published, so
# the card can say what to do instead of only that data is missing.
emit_claude_setup_state() {
  name="$1"
  dir="$HOME/.claude"
  helper=missing
  if [ -f "$dir/$name" ]; then
    if [ -s "$dir/$name" ]; then helper=ok; else helper=empty; fi
  fi
  line=none
  if [ -r "$dir/settings.json" ]; then
    if grep -q 'gputerm-claude-statusline' "$dir/settings.json" 2>/dev/null; then
      line=ours
    elif grep -q '"statusLine"' "$dir/settings.json" 2>/dev/null; then
      line=other
    fi
  fi
  printf '__GPUTERM_AGENT_FILE__\tclaude\tsetup-state\n'
  printf '{"scope":"setup","helper":"%s","status_line":"%s"}\n' "$helper" "$line"
  printf '__GPUTERM_AGENT_END__\n'
}
# Status-line snapshots are emitted after the transcripts so their richer
# fields win the per-session merge. One file per session id keeps concurrent
# sessions from overwriting each other.
emit_agent_snapshots() {
  provider="$1"
  dir="$HOME/.cache/gputerm/agent-status/$provider"
  if [ -d "$dir" ]; then
    # The account-wide quota is emitted by name. Selecting only the newest few
    # files used to miss it entirely: a provider publishes limits after its
    # first response, while short-lived sessions keep writing newer snapshots
    # that carry none.
    emit_agent_tail "$provider" "$dir/account.json"
    find "$dir" -type f -name '*.json' ! -name 'account.json' -exec ls -t {} + 2>/dev/null | head -n 4 |
    while IFS= read -r file; do
      emit_agent_tail "$provider" "$file"
    done
  fi
  emit_agent_tail "$provider" "$HOME/.cache/gputerm/agent-status/$provider.json"
}
"#;

/// AGY 1.0 stores token metadata in SQLite/protobuf conversation records. Read
/// only the small generator-metadata blobs: no step payloads, prompts, model
/// responses, tool arguments, credentials, or environment values are selected.
const AGY_USAGE_PYTHON: &str = r#"import glob, json, os, sqlite3

def read_varint(data, pos):
    value = 0
    shift = 0
    while pos < len(data) and shift < 70:
        byte = data[pos]
        pos += 1
        value |= (byte & 0x7f) << shift
        if byte < 0x80:
            return value, pos
        shift += 7
    raise ValueError("invalid protobuf varint")

def fields(data):
    result = {}
    pos = 0
    while pos < len(data):
        tag, pos = read_varint(data, pos)
        number, wire = tag >> 3, tag & 7
        if number == 0:
            break
        if wire == 0:
            value, pos = read_varint(data, pos)
        elif wire == 1:
            value, pos = data[pos:pos + 8], pos + 8
        elif wire == 2:
            size, pos = read_varint(data, pos)
            value, pos = data[pos:pos + size], pos + size
        elif wire == 5:
            value, pos = data[pos:pos + 4], pos + 4
        else:
            raise ValueError("unsupported protobuf wire type")
        result.setdefault(number, []).append(value)
    return result

def child(parent, number):
    for value in reversed(parent.get(number, [])):
        if isinstance(value, (bytes, bytearray)):
            try:
                return fields(value)
            except Exception:
                pass
    return {}

def integer(parent, number):
    for value in reversed(parent.get(number, [])):
        if isinstance(value, int):
            return value
    return 0

def text(parent, number):
    for value in reversed(parent.get(number, [])):
        if isinstance(value, (bytes, bytearray)):
            try:
                decoded = value.decode("utf-8").strip()
            except Exception:
                continue
            if decoded:
                return decoded
    return None

root = os.path.expanduser("~/.gemini/antigravity-cli/conversations")
paths = sorted(glob.glob(os.path.join(root, "*.db")), key=os.path.getmtime, reverse=True)[:2]
for path in paths:
    total_input = total_output = 0
    context_used = context_window = 0
    model = None
    try:
        uri = "file:" + path.replace("?", "%3f") + "?mode=ro&immutable=1"
        connection = sqlite3.connect(uri, uri=True)
        rows = connection.execute("SELECT data FROM gen_metadata ORDER BY idx").fetchall()
        connection.close()
        for (blob,) in rows:
            outer = fields(bytes(blob))
            generation = child(outer, 1)
            usage = child(generation, 4)
            if not usage:
                continue
            # AGY generator token metadata: input, output, cached input, and
            # tool-use input. Output already includes visible + thought tokens.
            current_input = integer(usage, 2) + integer(usage, 5) + integer(usage, 6)
            current_output = integer(usage, 3)
            total_input += current_input
            total_output += current_output
            context_used = current_input
            config = child(generation, 15)
            context_window = integer(config, 2) or context_window
            model = text(generation, 21) or model
    except Exception:
        continue
    if not (total_input or total_output or context_used or model):
        continue
    payload = {
        "conversation_id": os.path.splitext(os.path.basename(path))[0],
        "model": model,
        "input_tokens": total_input or None,
        "output_tokens": total_output or None,
        "total_tokens": (total_input + total_output) or None,
        "context_window": {
            "context_used_tokens": context_used or None,
            "context_window_size": context_window or None,
        },
    }
    print("__GPUTERM_AGENT_FILE__\tagy\t" + path)
    print(json.dumps(payload, separators=(",", ":")))
    print("__GPUTERM_AGENT_END__")"#;

const WINDOWS_METADATA_PRELUDE: &str = r#"$ErrorActionPreference='SilentlyContinue'
$GpuTermHome = [Environment]::GetFolderPath('UserProfile')
if ([string]::IsNullOrWhiteSpace($GpuTermHome)) { $GpuTermHome = $HOME }
function Emit-AgentTail([string]$provider, [string]$path) {
  if (-not (Test-Path -LiteralPath $path)) { return }
  Write-Output ("__GPUTERM_AGENT_FILE__`t{0}`t{1}" -f $provider, $path)
  Get-Content -LiteralPath $path -Tail 300 -ErrorAction SilentlyContinue
  Write-Output '__GPUTERM_AGENT_END__'
}
function Emit-AgentFiles([string]$provider, [string]$root, [string]$filter, [int]$depth) {
  if (-not (Test-Path -LiteralPath $root)) { return }
  Get-ChildItem -LiteralPath $root -Recurse -Depth $depth -File -Filter $filter |
    Where-Object { $_.FullName -notmatch '\\subagents\\' } |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 2 |
    ForEach-Object {
      Write-Output ("__GPUTERM_AGENT_FILE__`t{0}`t{1}" -f $provider, $_.FullName)
      Get-Content -LiteralPath $_.FullName -TotalCount 1 -ErrorAction SilentlyContinue
      Get-Content -LiteralPath $_.FullName -Tail 300 -ErrorAction SilentlyContinue
      Write-Output '__GPUTERM_AGENT_END__'
    }
}
function Emit-ClaudeSetupState([string]$name) {
  $dir = Join-Path $GpuTermHome '.claude'
  $helperPath = Join-Path $dir $name
  $helper = 'missing'
  if (Test-Path -LiteralPath $helperPath) {
    if ((Get-Item -LiteralPath $helperPath).Length -gt 0) { $helper = 'ok' } else { $helper = 'empty' }
  }
  $line = 'none'
  $settings = Join-Path $dir 'settings.json'
  if (Test-Path -LiteralPath $settings) {
    $text = Get-Content -LiteralPath $settings -Raw -ErrorAction SilentlyContinue
    if ($text -like '*gputerm-claude-statusline*') { $line = 'ours' }
    elseif ($text -like '*statusLine*') { $line = 'other' }
  }
  Write-Output ("__GPUTERM_AGENT_FILE__`tclaude`tsetup-state")
  Write-Output ("{{`"scope`":`"setup`",`"helper`":`"{0}`",`"status_line`":`"{1}`"}}" -f $helper, $line)
  Write-Output '__GPUTERM_AGENT_END__'
}
function Emit-AgentSnapshots([string]$provider) {
  $dir = Join-Path $GpuTermHome ".cache\gputerm\agent-status\$provider"
  if (Test-Path -LiteralPath $dir) {
    # Emitted by name for the same reason as the POSIX branch: recency alone
    # misses the account-wide quota, because a provider only publishes limits
    # after its first response while short-lived sessions keep writing newer
    # snapshots that carry none.
    Emit-AgentTail $provider (Join-Path $dir 'account.json')
    Get-ChildItem -LiteralPath $dir -File -Filter '*.json' |
      Where-Object { $_.Name -ne 'account.json' } |
      Sort-Object LastWriteTime -Descending |
      Select-Object -First 4 |
      ForEach-Object { Emit-AgentTail $provider $_.FullName }
  }
  Emit-AgentTail $provider (Join-Path $GpuTermHome ".cache\gputerm\agent-status\$provider.json")
}
"#;

/// Builds the metadata scrape for the providers that actually have a process
/// running. Skipping the other blocks keeps the five-second poll from running
/// an unnecessary `find` or Python interpreter on the remote host.
fn metadata_command(os: RemoteOs, providers: &HashSet<Provider>) -> String {
    let mut script = String::new();
    if os == RemoteOs::Windows {
        script.push_str(WINDOWS_METADATA_PRELUDE);
        if providers.contains(&Provider::Codex) {
            script.push_str(
                "Emit-AgentFiles 'codex' (Join-Path $GpuTermHome '.codex\\sessions') 'rollout-*.jsonl' 3\n",
            );
        }
        if providers.contains(&Provider::Claude) {
            script.push_str(
                "Emit-AgentFiles 'claude' (Join-Path $GpuTermHome '.claude\\projects') '*.jsonl' 1\n",
            );
        }
        if providers.contains(&Provider::Agy) {
            script.push_str("$agyPython = @'\n");
            script.push_str(AGY_USAGE_PYTHON);
            script.push_str("\n'@\n$python = Get-Command python3.exe -ErrorAction SilentlyContinue\nif (-not $python) { $python = Get-Command python.exe -ErrorAction SilentlyContinue }\nif ($python) { & $python.Source -c $agyPython 2>$null }\n");
            script.push_str("Emit-AgentSnapshots 'agy'\n");
        }
        if providers.contains(&Provider::Claude) {
            script.push_str("Emit-AgentSnapshots 'claude'\n");
            script.push_str(&format!(
                "Emit-ClaudeSetupState '{}'\n",
                claude_helper_for_os(RemoteOs::Windows).0
            ));
        }
        script.push_str("exit 0");
        return script;
    }
    script.push_str(POSIX_METADATA_PRELUDE);
    if providers.contains(&Provider::Codex) {
        // ~/.codex/sessions/<year>/<month>/<day>/rollout-*.jsonl
        script.push_str("emit_agent_files codex \"$HOME/.codex/sessions\" 'rollout-*.jsonl' 4\n");
    }
    if providers.contains(&Provider::Claude) {
        // ~/.claude/projects/<project>/<session>.jsonl
        script.push_str("emit_agent_files claude \"$HOME/.claude/projects\" '*.jsonl' 2\n");
    }
    if providers.contains(&Provider::Agy) {
        script.push_str("if command -v python3 >/dev/null 2>&1; then\npython3 - <<'GPUTERM_AGY_USAGE' 2>/dev/null\n");
        script.push_str(AGY_USAGE_PYTHON);
        script.push_str("\nGPUTERM_AGY_USAGE\nfi\n");
        script.push_str("emit_agent_snapshots agy\n");
    }
    if providers.contains(&Provider::Claude) {
        script.push_str("emit_agent_snapshots claude\n");
        script.push_str(&format!(
            "emit_claude_setup_state '{}'\n",
            claude_helper_for_os(os).0
        ));
    }
    script.push_str("true\n");
    script
}

pub fn collect_remote_agents(
    session: &Session,
    target: &SshTarget,
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
    let providers = detected_providers(&processes);
    if providers.is_empty() {
        return Ok(Vec::new());
    }
    refresh_metadata_if_due(state, os, &providers, |command| {
        // The scrape walks provider session directories, which takes longer than
        // a single-value telemetry probe. Sharing the 3 s budget made a large
        // ~/.claude/projects tree fail every time, silently leaving the card
        // without metadata.
        if os == RemoteOs::Windows {
            run_remote_command_for(session, os, command)
        } else {
            run_remote_command_with_budget(session, command, METADATA_COMMAND_TIMEOUT_SECS)
        }
    });
    refresh_provider_quotas_remote(target, os, &providers, state);
    update_quota_snapshots(state, now_epoch_seconds());
    Ok(build_agent_metrics(
        &processes,
        &state.metadata,
        &state.quotas,
    ))
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
    let providers = detected_providers(&processes);
    if providers.is_empty() {
        return Ok(Vec::new());
    }
    refresh_metadata_if_due(state, os, &providers, |command| {
        run_local_command_with_timeout(
            os,
            command,
            Duration::from_secs(METADATA_COMMAND_TIMEOUT_SECS),
        )
    });
    refresh_provider_quotas_local(os, &providers, &processes, state);
    update_quota_snapshots(state, now_epoch_seconds());
    Ok(build_agent_metrics(
        &processes,
        &state.metadata,
        &state.quotas,
    ))
}

fn refresh_metadata_if_due<F>(
    state: &mut AgentMonitorState,
    os: RemoteOs,
    providers: &HashSet<Provider>,
    run: F,
) where
    F: FnOnce(&str) -> Result<String, String>,
{
    // A newly started agent should not wait out the throttle before its usage
    // appears, so a changed provider set forces an immediate scan.
    let due = *providers != state.last_metadata_providers
        || state
            .last_metadata_scan
            .map(|last| last.elapsed() >= METADATA_REFRESH_INTERVAL)
            .unwrap_or(true);
    if !due {
        return;
    }
    state.last_metadata_scan = Some(Instant::now());
    state.last_metadata_providers = providers.clone();
    match run(&metadata_command(os, providers)) {
        Ok(output) => {
            state.last_metadata_error = None;
            let now = now_epoch_seconds();
            state.metadata = parse_metadata_output(&output, now);
            merge_metadata_quota_fallbacks(state, providers, now);
        }
        Err(error) => {
            // Kept rather than discarded: the process card stays valid, but the
            // reason no session or quota data arrived has to reach the user.
            state.last_metadata_error = Some(error);
            let now = now_epoch_seconds();
            merge_metadata_quota_fallbacks(state, providers, now);
        }
    }
}

#[tauri::command]
pub fn refresh_agent_quota(
    state: State<'_, AppState>,
    session_id: String,
    provider: String,
) -> Result<(), String> {
    let provider = Provider::parse(&provider)
        .ok_or_else(|| format!("Unsupported coding-agent provider: {}", provider))?;
    let active = state
        .active_connections
        .lock()
        .map_err(|_| "Active session state is unavailable".to_string())?
        .contains_key(&session_id);
    if !active {
        return Err("No active terminal session is available".to_string());
    }
    let mut requests = state
        .agent_quota_refreshes
        .lock()
        .map_err(|_| "Agent quota refresh state is unavailable".to_string())?;
    requests
        .entry(session_id)
        .or_default()
        .insert(provider.key().to_string());
    Ok(())
}

#[tauri::command]
pub async fn configure_claude_quota_monitor(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<ClaudeQuotaSetupResult, String> {
    let is_local = state
        .active_connections
        .lock()
        .map_err(|_| "Active session state is unavailable".to_string())?
        .get(&session_id)
        .map(|connection| connection.profile.is_local)
        .ok_or_else(|| "No active terminal session is available".to_string())?;
    let refreshes = Arc::clone(&state.agent_quota_refreshes);
    let result = if is_local {
        tauri::async_runtime::spawn_blocking(configure_claude_quota_local)
            .await
            .map_err(|error| format!("Claude monitor setup task failed: {}", error))??
    } else {
        let target = target_for_active_session(&state, &session_id)?;
        let ops = Arc::clone(&state.ops_sessions);
        let os_cache = Arc::clone(&state.remote_os_cache);
        tauri::async_runtime::spawn_blocking(move || {
            with_ops_session(&ops, &target, 10_000, |session| {
                let os = os_cache
                    .lock()
                    .ok()
                    .and_then(|cache| cache.get(&target.session_id).copied())
                    .or_else(|| detect_remote_os(session))
                    .unwrap_or(RemoteOs::Linux);
                configure_claude_quota_remote(session, os)
            })
        })
        .await
        .map_err(|error| format!("Remote Claude monitor setup task failed: {}", error))??
    };
    if matches!(result.status.as_str(), "configured" | "alreadyConfigured") {
        if let Ok(mut requests) = refreshes.lock() {
            requests
                .entry(session_id)
                .or_default()
                .insert(Provider::Claude.key().to_string());
        }
    }
    Ok(result)
}
