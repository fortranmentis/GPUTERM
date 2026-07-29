//! Read-only monitoring for terminal coding agents.
//!
//! Process trees provide the reliable cross-platform baseline. Provider
//! session files are sampled conservatively for non-sensitive metadata only:
//! IDs, model names, token/context counters, cost/duration, rate-limit
//! snapshots, and AGY worker state. Prompt, response, tool input/output, and
//! authentication fields are never serialized into GpuTerm telemetry.

use crate::ssh::session::{
    open_ssh_session, target_for_active_session, with_ops_session, AppState, SshTarget,
};
use crate::ssh::system_monitor::{
    detect_remote_os, local_os, run_local_command_for, run_local_command_with_timeout,
    run_remote_command_for, run_remote_command_with_budget, RemoteOs,
};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::Serialize;
use serde_json::Value;
use ssh2::{Channel, Session};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::State;

const METADATA_REFRESH_INTERVAL: Duration = Duration::from_secs(5);
/// Time budget for one metadata scrape. Larger than the telemetry default
/// because the scrape walks provider session directories.
const METADATA_COMMAND_TIMEOUT_SECS: u64 = 10;
const CODEX_QUOTA_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
const AGY_QUOTA_REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);
const CODEX_QUOTA_TIMEOUT: Duration = Duration::from_secs(5);
const AGY_STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const AGY_QUOTA_TIMEOUT: Duration = Duration::from_secs(15);
const AGY_USAGE_COMMAND: &[u8] = b"/usage\r";
const AGY_QUOTA_HISTORY_WINDOW_SECONDS: u64 = 24 * 60 * 60;
const AGY_QUOTA_HISTORY_BUCKET_SECONDS: u64 = 5 * 60;
const AGY_QUOTA_HISTORY_MAX_POINTS: usize = 288;

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

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRateLimitMetric {
    label: String,
    group: Option<String>,
    model_names: Vec<String>,
    remaining_percent: Option<f64>,
    used_percent: Option<f64>,
    window_minutes: Option<u64>,
    resets_at: Option<u64>,
    /// The window already rolled over, so `used_percent` describes a window
    /// that no longer exists. The UI reports a reset instead of a balance.
    stale: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentQuotaHistoryLimit {
    group: Option<String>,
    window_minutes: u64,
    remaining_percent: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentQuotaHistoryPoint {
    captured_at: u64,
    status: String,
    limits: Vec<AgentQuotaHistoryLimit>,
}

pub type AgentQuotaHistories = Arc<Mutex<HashMap<String, Vec<AgentQuotaHistoryPoint>>>>;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentQuotaSnapshot {
    status: String,
    source: String,
    captured_at: Option<u64>,
    snapshot_age_seconds: Option<u64>,
    message: Option<String>,
    limits: Vec<AgentRateLimitMetric>,
    history: Vec<AgentQuotaHistoryPoint>,
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
    fn unavailable(provider: Provider) -> Self {
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

    fn available(
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
    context_remaining_tokens: Option<u64>,
    context_remaining_percent: Option<f64>,
    last_request_input_tokens: Option<u64>,
    last_request_output_tokens: Option<u64>,
    last_request_cache_creation_tokens: Option<u64>,
    last_request_cache_read_tokens: Option<u64>,
    cost_usd: Option<f64>,
    session_duration_seconds: Option<f64>,
    /// Age of the status-line snapshot these numbers came from. Absent when the
    /// provider's own session records supplied them.
    snapshot_age_seconds: Option<u64>,
    quota: AgentQuotaSnapshot,
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
    executable_path: Option<String>,
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
    context_remaining_tokens: Option<u64>,
    context_remaining_percent: Option<f64>,
    last_request_input_tokens: Option<u64>,
    last_request_output_tokens: Option<u64>,
    last_request_cache_creation_tokens: Option<u64>,
    last_request_cache_read_tokens: Option<u64>,
    cost_usd: Option<f64>,
    session_duration_seconds: Option<f64>,
    snapshot_age_seconds: Option<u64>,
    captured_at: Option<u64>,
    /// Process id the status-line snapshot belongs to, used to attribute usage
    /// to the right session when several run at once.
    pid_hint: Option<u32>,
    /// Set for the account-wide quota record rather than a real session.
    account_scope: bool,
    /// Set for the setup-state record, which reports which install step is
    /// incomplete rather than describing a session.
    setup_scope: bool,
    /// `ok`, `empty`, or `missing`.
    setup_helper: Option<String>,
    /// `ours`, `other`, or `none`.
    setup_status_line: Option<String>,
    rate_limits: Vec<AgentRateLimitMetric>,
    subagents: Vec<AgentWorkMetric>,
    background_tasks: Vec<AgentWorkMetric>,
}

#[derive(Default)]
pub struct AgentMonitorState {
    last_metadata_scan: Option<Instant>,
    last_metadata_providers: HashSet<Provider>,
    metadata: HashMap<Provider, Vec<AgentSessionMetadata>>,
    quotas: HashMap<Provider, AgentQuotaSnapshot>,
    last_quota_refresh: HashMap<Provider, Instant>,
    forced_quota_refresh: HashSet<Provider>,
    agy_history_key: Option<String>,
    agy_histories: Option<AgentQuotaHistories>,
    agy_history: Vec<AgentQuotaHistoryPoint>,
    quota_probes: Arc<QuotaProbes>,
    windows_cpu: HashMap<u32, f64>,
    windows_cpu_sampled_at: Option<Instant>,
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

fn detected_providers(processes: &[ProcessSample]) -> HashSet<Provider> {
    processes.iter().filter_map(provider_for_process).collect()
}

/// Returns the native provider executable already observed in the Windows
/// process list. This is more reliable than the GUI application's PATH, which
/// may not contain CLI directories added by a terminal profile.
fn provider_executable_hint(processes: &[ProcessSample], provider: Provider) -> Option<String> {
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
    if let Ok(output) = run(&metadata_command(os, providers)) {
        let now = now_epoch_seconds();
        state.metadata = parse_metadata_output(&output, now);
        merge_metadata_quota_fallbacks(state, providers, now);
    }
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

fn quota_refresh_due(
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
fn unavailable_with_setup_hint(
    provider: Provider,
    setup: Option<&AgentSessionMetadata>,
) -> AgentQuotaSnapshot {
    let mut snapshot = AgentQuotaSnapshot::unavailable(provider);
    if provider != Provider::Claude {
        return snapshot;
    }
    let Some(setup) = setup else {
        return snapshot;
    };
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
    if let Some(message) = message {
        snapshot.message = Some(message);
    }
    snapshot
}

fn merge_metadata_quota_fallbacks(
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
            state
                .quotas
                .entry(*provider)
                .or_insert_with(|| unavailable_with_setup_hint(*provider, None));
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
            let snapshot = unavailable_with_setup_hint(*provider, hint);
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
struct QuotaProbes {
    finished: Mutex<Vec<(Provider, Result<AgentQuotaSnapshot, String>)>>,
    in_flight: Mutex<HashSet<Provider>>,
}

/// Starts a probe unless one for the same provider is still running.
fn spawn_quota_probe<F>(probes: &Arc<QuotaProbes>, provider: Provider, probe: F)
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
fn apply_finished_quota_probes(
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

fn refresh_provider_quotas_local(
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

fn refresh_provider_quotas_remote(
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
            let connection = open_ssh_session(&target)?;
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
            let connection = open_ssh_session(&target)?;
            query_agy_quota_remote(connection.session(), os, now_epoch_seconds())
        });
    }
}

fn update_quota_snapshots(state: &mut AgentMonitorState, now: u64) {
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

fn record_agy_history(
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

fn sync_agy_history(state: &mut AgentMonitorState, now: u64) {
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

fn upsert_agy_history_point(
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

fn trim_agy_history(history: &mut Vec<AgentQuotaHistoryPoint>, now: u64) {
    let cutoff = now.saturating_sub(AGY_QUOTA_HISTORY_WINDOW_SECONDS);
    history.retain(|point| point.captured_at >= cutoff);
    if history.len() > AGY_QUOTA_HISTORY_MAX_POINTS {
        history.drain(..history.len() - AGY_QUOTA_HISTORY_MAX_POINTS);
    }
}

fn codex_initialize_request() -> &'static str {
    r#"{"id":1,"method":"initialize","params":{"clientInfo":{"name":"gputerm-monitor","version":"1"},"capabilities":{"experimentalApi":true}}}"#
}

fn codex_rate_limit_request() -> &'static str {
    r#"{"id":2,"method":"account/rateLimits/read","params":null}"#
}

fn query_codex_quota_local(
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

fn wait_for_json_response(
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

fn query_codex_quota_remote(
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

fn read_channel_json_response(channel: &mut Channel, id: u64) -> Result<Value, String> {
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

fn parse_codex_quota_response(response: &Value, now: u64) -> Result<AgentQuotaSnapshot, String> {
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

fn query_agy_quota_local(
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

fn wait_for_agy_startup(
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

fn write_agy_usage_command(writer: &mut impl Write) -> std::io::Result<()> {
    writer.write_all(AGY_USAGE_COMMAND)?;
    writer.flush()
}

fn collect_pty_probe_output(receiver: &mpsc::Receiver<Vec<u8>>, timeout: Duration) -> Vec<u8> {
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

fn agy_usage_output_complete(output: &str) -> bool {
    parse_agy_usage_output(output, 0).is_ok()
}

fn query_agy_quota_remote(
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

fn wait_for_remote_agy_startup(channel: &mut Channel, timeout: Duration) -> Result<(), String> {
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
fn executable_candidates(process: &ProcessSample) -> Vec<String> {
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

fn provider_for_process(process: &ProcessSample) -> Option<Provider> {
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

fn build_agent_metrics(
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
fn assign_metadata(
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

fn parse_metadata_output(output: &str, now: u64) -> HashMap<Provider, Vec<AgentSessionMetadata>> {
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

fn insert_provider_metadata(
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

fn metadata_has_values(metadata: &AgentSessionMetadata) -> bool {
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

fn merge_metadata(base: &mut AgentSessionMetadata, newer: AgentSessionMetadata) {
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

fn parse_provider_metadata(provider: Provider, lines: &[String], now: u64) -> AgentSessionMetadata {
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

fn parse_codex_metadata(values: &[Value], now: u64) -> AgentSessionMetadata {
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
fn parse_claude_metadata(values: &[Value], now: u64) -> AgentSessionMetadata {
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
fn apply_last_request_usage(metadata: &mut AgentSessionMetadata, usage: &Value) {
    metadata.last_request_input_tokens =
        value_u64(usage, "input_tokens").or(metadata.last_request_input_tokens);
    metadata.last_request_output_tokens =
        value_u64(usage, "output_tokens").or(metadata.last_request_output_tokens);
    metadata.last_request_cache_creation_tokens = value_u64(usage, "cache_creation_input_tokens")
        .or(metadata.last_request_cache_creation_tokens);
    metadata.last_request_cache_read_tokens =
        value_u64(usage, "cache_read_input_tokens").or(metadata.last_request_cache_read_tokens);
}

fn parse_agy_metadata(values: &[Value], now: u64) -> AgentSessionMetadata {
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
const NON_QUOTA_LIMIT_KEYS: [&str; 12] = [
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
const RESET_STALE_GRACE_SECONDS: u64 = 60;

fn parse_rate_limits(value: Option<&Value>, now: u64) -> Vec<AgentRateLimitMetric> {
    let Some(Value::Object(limits)) = value else {
        return Vec::new();
    };
    let mut parsed = Vec::new();
    parse_rate_limit_entries(limits, None, now, &mut parsed);
    parsed
}

fn parse_rate_limit_entries(
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

fn now_epoch_seconds() -> u64 {
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
fn epoch_seconds(value: u64) -> u64 {
    const MILLISECOND_THRESHOLD: u64 = 10_000_000_000;
    if value >= MILLISECOND_THRESHOLD {
        value / 1000
    } else {
        value
    }
}

fn event_epoch_seconds(value: &Value) -> Option<u64> {
    let raw = value_string(value, "timestamp")
        .or_else(|| value_string(value, "captured_at"))
        .or_else(|| value_string(value, "capturedAt"))?;
    chrono::DateTime::parse_from_rfc3339(&raw)
        .ok()
        .and_then(|value| u64::try_from(value.timestamp()).ok())
}

fn infer_rate_limit_window(label: &str) -> Option<u64> {
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

fn strip_terminal_control(input: &str) -> String {
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

fn parse_agy_usage_output(input: &str, now: u64) -> Result<AgentQuotaSnapshot, String> {
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

fn has_complete_agy_quota(limits: &[AgentRateLimitMetric]) -> bool {
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

struct PendingAgyLimit {
    label: String,
    group: String,
    window_minutes: u64,
    precise_remaining_percent: Option<f64>,
    resets_at: Option<u64>,
}

fn parse_agy_model_names(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string)
        .collect()
}

fn extract_display_percent(line: &str) -> Option<f64> {
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

fn parse_percent_before(line: &str, percent: usize) -> Option<f64> {
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

fn extract_remaining_percent(line: &str) -> Option<f64> {
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

fn parse_refresh_seconds(line: &str) -> Option<u64> {
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeQuotaSetupResult {
    status: String,
    message: String,
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

fn configure_claude_quota_local() -> Result<ClaudeQuotaSetupResult, String> {
    let os = local_os();
    if os == RemoteOs::Linux && !local_claude_helper_runtime_available(os) {
        return Ok(ClaudeQuotaSetupResult {
            status: "unsupported".to_string(),
            message: "Claude quota setup needs Python 3 on this host. Install Python 3, then run Set up again.".to_string(),
        });
    }
    let Some(home) = std::env::var_os(if os == RemoteOs::Windows {
        "USERPROFILE"
    } else {
        "HOME"
    }) else {
        return Ok(ClaudeQuotaSetupResult {
            status: "unsupported".to_string(),
            message: "Claude quota setup cannot locate this user's home directory.".to_string(),
        });
    };
    let home = PathBuf::from(home);
    let claude_dir = home.join(".claude");
    let settings_path = claude_dir.join("settings.json");
    let (helper_name, helper_contents) = claude_helper_for_os(os);
    let helper_path = claude_dir.join(helper_name);
    let desired_command =
        claude_status_line_command_for(os, helper_path.to_string_lossy().as_ref());
    fs::create_dir_all(&claude_dir)
        .map_err(|error| format!("failed to create Claude config directory: {}", error))?;
    let (mut settings, existing) = read_claude_settings(&settings_path)?;
    if let Some(command) = existing.as_deref() {
        if !command.contains("gputerm-claude-statusline") {
            return Ok(ClaudeQuotaSetupResult {
                status: "conflict".to_string(),
                message: format!(
                    "Claude already uses a custom status line (`{}`). It was not overwritten. Add {} to that status-line pipeline manually.",
                    command, desired_command
                ),
            });
        }
    }
    write_claude_helper(&helper_path, helper_contents)?;
    set_claude_status_line(&mut settings, &desired_command);
    backup_and_write_json(&settings_path, &settings)?;
    remove_stale_claude_helpers(&claude_dir, helper_name);
    Ok(ClaudeQuotaSetupResult {
        status: if existing.is_some() {
            "alreadyConfigured"
        } else {
            "configured"
        }
        .to_string(),
        message: "Claude quota monitoring is configured. Restart Claude Code, accept workspace trust if prompted, then send one message to publish the 5-hour and weekly limits.".to_string(),
    })
}

fn configure_claude_quota_remote(
    session: &Session,
    os: RemoteOs,
) -> Result<ClaudeQuotaSetupResult, String> {
    if os == RemoteOs::Linux {
        let runtime_command = "if command -v python3 >/dev/null 2>&1 || command -v python >/dev/null 2>&1; then printf supported; else printf unsupported; fi";
        if run_remote_command_for(session, os, runtime_command)?.trim() != "supported" {
            return Ok(ClaudeQuotaSetupResult {
                status: "unsupported".to_string(),
                message: "Claude quota setup needs Python 3 on this host. Install Python 3, then run Set up again.".to_string(),
            });
        }
    }

    // Everything below moves over SFTP rather than through a shell command.
    // Embedding the helper in an exec request could not work:
    //
    // * macOS/BSD `base64` rejects a positional input file, so the POSIX
    //   decode failed and — because the shell truncates the target before
    //   running the decoder — destroyed any previously working helper.
    // * The Windows PowerShell form reached about 23,000 characters on the
    //   wire, far past cmd.exe's 8,191-character limit, and Windows OpenSSH
    //   runs exec requests through cmd.exe unless an admin changed
    //   `DefaultShell`.
    //
    // SFTP also avoids login-shell output contaminating the settings read.
    let sftp = session
        .sftp()
        .map_err(|error| format!("Claude quota setup needs SFTP on this host: {}", error))?;
    let home = sftp
        .realpath(Path::new("."))
        .map_err(|error| format!("failed to resolve the remote home directory: {}", error))?;
    let claude_dir = remote_join(&home, ".claude");
    if let Err(error) = sftp.mkdir(Path::new(&claude_dir), 0o700) {
        // Already present is the normal case; anything else is fatal.
        if sftp.stat(Path::new(&claude_dir)).is_err() {
            return Err(format!(
                "failed to create {} on the remote host: {}",
                claude_dir, error
            ));
        }
    }

    let settings_path = remote_join(&claude_dir, "settings.json");
    let (mut settings, existing) = read_remote_claude_settings(&sftp, &settings_path)?;
    let (helper_name, helper_contents) = claude_helper_for_os(os);
    let helper_path = remote_join(&claude_dir, helper_name);
    let desired_command = claude_status_line_command_for(os, &native_remote_path(&helper_path));
    if let Some(command) = existing.as_deref() {
        if !command.contains("gputerm-claude-statusline") {
            return Ok(ClaudeQuotaSetupResult {
                status: "conflict".to_string(),
                message: format!(
                    "Claude already uses a custom status line (`{}`). It was not overwritten. Add {} to that status-line pipeline manually.",
                    command, desired_command
                ),
            });
        }
    }
    set_claude_status_line(&mut settings, &desired_command);

    write_remote_file_atomically(&sftp, &helper_path, helper_contents.as_bytes())?;
    if os != RemoteOs::Windows {
        let stat = ssh2::FileStat {
            size: None,
            uid: None,
            gid: None,
            perm: Some(0o700),
            atime: None,
            mtime: None,
        };
        let _ = sftp.setstat(Path::new(&helper_path), stat);
    }

    if existing.is_some() {
        let backup = remote_join(&claude_dir, "settings.json.gputerm-backup");
        if let Ok(mut current) = sftp.open(Path::new(&settings_path)) {
            let mut bytes = Vec::new();
            if current.read_to_end(&mut bytes).is_ok() {
                let _ = write_remote_file_atomically(&sftp, &backup, &bytes);
            }
        }
    }
    let encoded = serde_json::to_vec_pretty(&settings)
        .map_err(|error| format!("failed to encode Claude settings: {}", error))?;
    write_remote_file_atomically(&sftp, &settings_path, &encoded)?;

    // A helper variant from an earlier GpuTerm release would otherwise sit
    // beside the current one, and the status line may still point at it.
    for (_, stale_name) in CLAUDE_HELPER_NAMES
        .iter()
        .filter(|(variant, _)| *variant != os)
    {
        if *stale_name != helper_name {
            let _ = sftp.unlink(Path::new(&remote_join(&claude_dir, stale_name)));
        }
    }

    Ok(ClaudeQuotaSetupResult {
        status: if existing.is_some() {
            "alreadyConfigured"
        } else {
            "configured"
        }
        .to_string(),
        message: "Claude quota monitoring is configured. Restart Claude Code, accept workspace trust if prompted, then send one message to publish the 5-hour and weekly limits.".to_string(),
    })
}

/// Every helper file name, so an install can remove the variants it replaces.
const CLAUDE_HELPER_NAMES: [(RemoteOs, &str); 3] = [
    (RemoteOs::Windows, "gputerm-claude-statusline.ps1"),
    (RemoteOs::MacOs, "gputerm-claude-statusline-macos.js"),
    (RemoteOs::Linux, "gputerm-claude-statusline.sh"),
];

/// Joins an SFTP path. SFTP always uses forward slashes, including against
/// Windows OpenSSH.
fn remote_join(base: impl AsRef<Path>, child: &str) -> String {
    let base = base.as_ref().to_string_lossy().replace('\\', "/");
    let base = base.trim_end_matches('/');
    format!("{}/{}", base, child)
}

/// Converts an SFTP path to the form the host's own tools expect. Windows
/// OpenSSH reports `/C:/Users/...`, but a `-File` argument needs `C:/Users/...`.
fn native_remote_path(path: &str) -> String {
    let bytes = path.as_bytes();
    if bytes.len() > 2 && bytes[0] == b'/' && bytes[2] == b':' {
        return path[1..].to_string();
    }
    path.to_string()
}

fn read_remote_claude_settings(
    sftp: &ssh2::Sftp,
    path: &str,
) -> Result<(Value, Option<String>), String> {
    let mut file = match sftp.open(Path::new(path)) {
        Ok(file) => file,
        // Absent settings are the first-run case, not an error.
        Err(_) => return Ok((Value::Object(serde_json::Map::new()), None)),
    };
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| format!("failed to read Claude settings: {}", error))?;
    let settings = parse_claude_settings(&bytes)?;
    let command = claude_status_line_command(&settings);
    Ok((settings, command))
}

/// Parses `settings.json`, tolerating an empty file and a UTF-8 BOM. Windows
/// editors and PowerShell redirection both write the BOM, and `serde_json`
/// treats it as a syntax error.
fn parse_claude_settings(bytes: &[u8]) -> Result<Value, String> {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim_start_matches('\u{feff}').trim();
    if text.is_empty() {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    serde_json::from_str::<Value>(text)
        .map_err(|error| format!("Claude settings.json is not valid JSON: {}", error))
}

/// Writes through a temporary sibling and renames over the target, so a failed
/// transfer cannot leave a truncated or empty file where a working one was.
fn write_remote_file_atomically(
    sftp: &ssh2::Sftp,
    path: &str,
    contents: &[u8],
) -> Result<(), String> {
    let temporary = format!("{}.gputerm-new", path);
    {
        let mut file = sftp
            .create(Path::new(&temporary))
            .map_err(|error| format!("failed to write {}: {}", temporary, error))?;
        file.write_all(contents)
            .map_err(|error| format!("failed to write {}: {}", temporary, error))?;
    }
    sftp.rename(Path::new(&temporary), Path::new(path), None)
        .map_err(|error| {
            let _ = sftp.unlink(Path::new(&temporary));
            format!("failed to replace {}: {}", path, error)
        })
}

fn local_claude_helper_runtime_available(os: RemoteOs) -> bool {
    if matches!(os, RemoteOs::Windows | RemoteOs::MacOs) {
        return true;
    }
    let candidates: &[&str] = &["python3", "python"];
    candidates.iter().any(|candidate| {
        let mut command = Command::new(candidate);
        command
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        command.status().is_ok_and(|status| status.success())
    })
}

fn read_claude_settings(path: &Path) -> Result<(Value, Option<String>), String> {
    let settings = match fs::read(path) {
        Ok(bytes) => parse_claude_settings(&bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Value::Object(serde_json::Map::new())
        }
        Err(error) => return Err(format!("failed to read Claude settings: {}", error)),
    };
    let command = claude_status_line_command(&settings);
    Ok((settings, command))
}

fn claude_status_line_command(settings: &Value) -> Option<String> {
    settings
        .pointer("/statusLine/command")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|command| !command.is_empty())
        .map(str::to_string)
}

fn set_claude_status_line(settings: &mut Value, command: &str) {
    if !settings.is_object() {
        *settings = Value::Object(serde_json::Map::new());
    }
    settings.as_object_mut().unwrap().insert(
        "statusLine".to_string(),
        serde_json::json!({
            "type": "command",
            "command": command,
            "padding": 0,
            "refreshInterval": 30
        }),
    );
}

fn claude_helper_for_os(os: RemoteOs) -> (&'static str, &'static str) {
    match os {
        RemoteOs::Windows => (
            "gputerm-claude-statusline.ps1",
            include_str!("../../../scripts/gputerm-claude-statusline.ps1"),
        ),
        RemoteOs::MacOs => (
            "gputerm-claude-statusline-macos.js",
            include_str!("../../../scripts/gputerm-claude-statusline-macos.js"),
        ),
        RemoteOs::Linux => (
            "gputerm-claude-statusline.sh",
            include_str!("../../../scripts/gputerm-claude-statusline.sh"),
        ),
    }
}

fn claude_status_line_command_for(os: RemoteOs, helper_path: &str) -> String {
    match os {
        RemoteOs::Windows => {
            let helper_path = helper_path.replace('\\', "/").replace('"', "\\\"");
            format!(
                "powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"{}\"",
                helper_path
            )
        }
        RemoteOs::MacOs => {
            "/usr/bin/osascript -l JavaScript \"$HOME/.claude/gputerm-claude-statusline-macos.js\""
                .to_string()
        }
        RemoteOs::Linux => "~/.claude/gputerm-claude-statusline.sh".to_string(),
    }
}

/// Installs the helper through a temporary sibling and a rename, so a failure
/// partway through cannot leave an empty file where a working helper was.
fn write_claude_helper(path: &Path, contents: &str) -> Result<(), String> {
    let temporary = path.with_extension("gputerm-new");
    fs::write(&temporary, contents)
        .map_err(|error| format!("failed to install Claude status-line helper: {}", error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("failed to make Claude helper executable: {}", error))?;
    }
    fs::rename(&temporary, path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        format!("failed to install Claude status-line helper: {}", error)
    })
}

/// Removes helper variants this OS no longer uses, so a stale file cannot sit
/// beside the current one with the status line still pointing at it.
fn remove_stale_claude_helpers(claude_dir: &Path, keep: &str) {
    for (_, name) in CLAUDE_HELPER_NAMES.iter().filter(|(_, name)| *name != keep) {
        let _ = fs::remove_file(claude_dir.join(name));
    }
}

fn backup_and_write_json(path: &Path, value: &Value) -> Result<(), String> {
    if path.exists() {
        let backup = path.with_file_name("settings.json.gputerm-backup");
        fs::copy(path, backup)
            .map_err(|error| format!("failed to back up Claude settings: {}", error))?;
    }
    let temporary = path.with_file_name("settings.json.gputerm-new");
    let encoded = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("failed to encode Claude settings: {}", error))?;
    fs::write(&temporary, encoded)
        .map_err(|error| format!("failed to write Claude settings: {}", error))?;
    fs::rename(&temporary, path)
        .map_err(|error| format!("failed to replace Claude settings: {}", error))
}

fn finalize_context(metadata: &mut AgentSessionMetadata) {
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

    /// Fixed clock for the parsers, well ahead of the fixture reset timestamps.
    const TEST_NOW: u64 = 1_799_000_000;

    fn parse_metadata(provider: Provider, lines: &[String]) -> AgentSessionMetadata {
        parse_provider_metadata(provider, lines, TEST_NOW)
    }

    #[test]
    fn detects_agents_and_aggregates_child_resources() {
        let processes = parse_posix_processes(
            "100 1 alice 2.0 100000 01:00 /usr/bin/codex\n\
             101 100 alice 4.0 50000 00:30 node worker.js\n\
             200 1 bob 1.0 70000 00:20 node /x/@anthropic-ai/claude-code/cli.js\n",
        );
        let metrics = build_agent_metrics(&processes, &HashMap::new(), &HashMap::new());
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
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1200,"output_tokens":300,"total_tokens":1500},"last_token_usage":{"total_tokens":500},"model_context_window":10000},"rate_limits":{"primary":{"used_percent":42,"window_minutes":10080,"resets_at":1800500000}}}}"#.to_string(),
        ];
        let metadata = parse_metadata(Provider::Codex, &lines);
        assert_eq!(metadata.session_id.as_deref(), Some("session-1"));
        assert_eq!(metadata.model.as_deref(), Some("gpt-5.4"));
        assert_eq!(metadata.total_tokens, Some(1500));
        assert_eq!(metadata.context_used_percent, Some(5.0));
        assert_eq!(metadata.rate_limits[0].used_percent, Some(42.0));
        assert_eq!(metadata.rate_limits[0].window_minutes, Some(10080));
        assert_eq!(metadata.rate_limits[0].group, None);
    }

    #[test]
    fn parses_agy_status_payload_without_prompt_content() {
        let lines = vec![r#"{"conversation_id":"agy-1","model":{"display_name":"Gemini"},"agent_state":"working","input_tokens":80,"output_tokens":20,"total_tokens":100,"context_window":{"context_used_tokens":100,"context_window_size":1000},"quota":{"premium":{"remaining_fraction":0.7}},"subagents":[{"name":"tests","role":"worker","status":"active"}],"background_tasks":[{"name":"npm test","status":"running"}],"prompt":"do not expose this prompt"}"#.to_string()];
        let metadata = parse_metadata(Provider::Agy, &lines);
        assert_eq!(metadata.status.as_deref(), Some("working"));
        assert_eq!(metadata.model.as_deref(), Some("Gemini"));
        assert_eq!(metadata.total_tokens, Some(100));
        assert_eq!(metadata.context_used_percent, Some(10.0));
        assert_eq!(metadata.context_remaining_tokens, Some(900));
        assert_eq!(metadata.context_remaining_percent, Some(90.0));
        assert_eq!(metadata.rate_limits[0].used_percent, Some(30.0));
        assert_eq!(metadata.subagents.len(), 1);
        assert_eq!(metadata.background_tasks.len(), 1);

        let processes =
            parse_posix_processes("100 1 alice 2.0 100000 01:00 agy --api-key do-not-expose\n");
        let mut sessions = HashMap::new();
        sessions.insert(Provider::Agy, vec![metadata]);
        let serialized =
            serde_json::to_string(&build_agent_metrics(&processes, &sessions, &HashMap::new()))
                .unwrap();
        assert!(!serialized.contains("do not expose this prompt"));
        assert!(!serialized.contains("do-not-expose"));
    }

    #[test]
    fn parses_agy_grouped_weekly_and_five_hour_limits() {
        let lines = vec![r#"{
            "quota": {
                "gemini_models": {
                    "display_name": "Gemini models",
                    "weekly_limit": {"remaining_percentage": 99.9, "refreshes_at": 1800000000},
                    "five_hour_limit": {"remaining_percentage": 99.4, "refreshes_at": 1799990000}
                },
                "claude_and_gpt_models": {
                    "display_name": "Claude and GPT models",
                    "weekly_limit": {"remaining_percentage": 100},
                    "five_hour_limit": {"remaining_percentage": 100}
                }
            }
        }"#
        .to_string()];
        let metadata = parse_metadata(Provider::Agy, &lines);
        assert_eq!(metadata.rate_limits.len(), 4);
        let gemini_weekly = metadata
            .rate_limits
            .iter()
            .find(|limit| {
                limit.group.as_deref() == Some("Gemini models")
                    && limit.window_minutes == Some(10080)
            })
            .unwrap();
        assert_eq!(gemini_weekly.group.as_deref(), Some("Gemini models"));
        assert_eq!(gemini_weekly.window_minutes, Some(10080));
        assert!((gemini_weekly.used_percent.unwrap() - 0.1).abs() < 1e-9);
        assert!(metadata.rate_limits.iter().any(|limit| {
            limit.group.as_deref() == Some("Gemini models") && limit.window_minutes == Some(300)
        }));
        assert!(metadata.rate_limits.iter().any(|limit| {
            limit.group.as_deref() == Some("Claude and GPT models")
                && limit.window_minutes == Some(10080)
        }));
    }

    #[test]
    fn parses_claude_context_and_rate_limits_without_faking_session_totals() {
        let lines = vec![
            r#"{"sessionId":"claude-1","message":{"id":"msg-1","model":"claude-sonnet","usage":{"input_tokens":100,"cache_read_input_tokens":50,"output_tokens":20}}}"#.to_string(),
            r#"{"sessionId":"claude-1","message":{"id":"msg-1","model":"claude-sonnet","usage":{"input_tokens":100,"cache_read_input_tokens":50,"output_tokens":20}}}"#.to_string(),
            r#"{"total_cost_usd":0.42,"total_duration_ms":5000,"context_window":{"total_input_tokens":150,"total_output_tokens":20,"context_window_size":1000,"used_percentage":15,"remaining_percentage":85},"rate_limits":{"five_hour":{"used_percentage":23.5,"resets_at":1800300000},"seven_day":{"used_percentage":41.2,"resets_at":1800900000}}}"#.to_string(),
        ];
        let metadata = parse_metadata(Provider::Claude, &lines);
        // `context_window.*` is the live context, so it must not be reported as
        // a cumulative session total.
        assert_eq!(metadata.input_tokens, None);
        assert_eq!(metadata.output_tokens, None);
        assert_eq!(metadata.total_tokens, None);
        assert_eq!(metadata.context_used_tokens, Some(150));
        assert_eq!(metadata.last_request_input_tokens, Some(100));
        assert_eq!(metadata.last_request_cache_read_tokens, Some(50));
        assert_eq!(metadata.last_request_output_tokens, Some(20));
        assert_eq!(metadata.cost_usd, Some(0.42));
        assert_eq!(metadata.session_duration_seconds, Some(5.0));
        assert_eq!(metadata.context_remaining_tokens, Some(850));
        assert_eq!(metadata.context_remaining_percent, Some(85.0));
        assert_eq!(metadata.rate_limits.len(), 2);
        assert_eq!(metadata.rate_limits[0].window_minutes, Some(300));
        assert_eq!(metadata.rate_limits[1].window_minutes, Some(10080));
    }

    #[test]
    fn merges_status_snapshot_into_matching_agent_session() {
        let output = concat!(
            "__GPUTERM_AGENT_FILE__\tagy\tconversation.db\n",
            "{\"conversation_id\":\"same\",\"input_tokens\":100,\"output_tokens\":20,\"context_window\":{\"context_used_tokens\":80,\"context_window_size\":1000}}\n",
            "__GPUTERM_AGENT_END__\n",
            "__GPUTERM_AGENT_FILE__\tagy\tstatus.json\n",
            "{\"conversation_id\":\"same\",\"agent_state\":\"working\",\"quota\":{\"premium\":{\"remaining_percentage\":75}}}\n",
            "__GPUTERM_AGENT_END__\n",
        );
        let grouped = parse_metadata_output(output, TEST_NOW);
        let sessions = grouped.get(&Provider::Agy).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].input_tokens, Some(100));
        assert_eq!(sessions[0].status.as_deref(), Some("working"));
        assert_eq!(sessions[0].rate_limits[0].used_percent, Some(25.0));
    }

    #[test]
    fn detects_agents_launched_from_long_absolute_paths() {
        // macOS truncates the `comm` column, so detection has to work from the
        // command line alone.
        let processes = parse_posix_processes(concat!(
            "100 1 alice 1.0 1000 01:00 /Users/a/Library/Application Support/Claude/claude-code/2.1.219/claude.app/Contents/MacOS/claude --output-format stream-json\n",
            "101 1 alice 1.0 1000 01:00 /Applications/ChatGPT.app/Contents/Resources/codex -c features.code_mode_host=true app-server\n",
            "102 1 alice 1.0 1000 01:00 codex exec --sandbox read-only\n",
            "103 1 alice 1.0 1000 01:00 /Applications/Claude.app/Contents/MacOS/Claude\n",
            "104 1 alice 1.0 1000 01:00 /Applications/ChatGPT.app/Contents/Frameworks/Codex Framework.framework/Helpers/Codex (Renderer).app/Contents/MacOS/Codex (Renderer) --type=renderer\n",
        ));
        let detected = processes
            .iter()
            .filter_map(|process| {
                provider_for_process(process).map(|provider| (process.pid, provider))
            })
            .collect::<HashMap<_, _>>();
        assert_eq!(detected.get(&100), Some(&Provider::Claude));
        assert_eq!(detected.get(&101), Some(&Provider::Codex));
        assert_eq!(detected.get(&102), Some(&Provider::Codex));
        // The desktop shell and its renderer helpers are not agent sessions.
        assert_eq!(detected.get(&103), None);
        assert_eq!(detected.get(&104), None);
    }

    #[test]
    fn ignores_subagent_records_so_they_cannot_overwrite_the_session() {
        // The collector can surface a session transcript and a worker
        // transcript that share a session id; only the session's own records
        // may describe its context.
        let output = concat!(
            "__GPUTERM_AGENT_FILE__\tclaude\tsession.jsonl\n",
            "{\"sessionId\":\"claude-1\",\"cwd\":\"/work\",\"message\":{\"id\":\"msg-1\",\"usage\":{\"input_tokens\":90000,\"cache_read_input_tokens\":10000,\"output_tokens\":500}}}\n",
            "__GPUTERM_AGENT_END__\n",
            "__GPUTERM_AGENT_FILE__\tclaude\tsubagents/agent-1.jsonl\n",
            "{\"sessionId\":\"claude-1\",\"isSidechain\":true,\"agentId\":\"agent-1\",\"message\":{\"id\":\"msg-2\",\"usage\":{\"input_tokens\":10,\"cache_read_input_tokens\":0,\"output_tokens\":5}}}\n",
            "__GPUTERM_AGENT_END__\n",
        );
        let grouped = parse_metadata_output(output, TEST_NOW);
        let sessions = grouped.get(&Provider::Claude).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].context_used_tokens, Some(100_000));
        assert_eq!(sessions[0].last_request_output_tokens, Some(500));
    }

    #[test]
    fn attributes_snapshots_to_their_own_process() {
        // Pid 201 is a launcher wrapping the agent at 200, as the Claude desktop
        // app does, so the snapshot names a pid below the tree root.
        let processes = parse_posix_processes(
            "201 1 alice 0.1 1000 00:21 launcher /x/@anthropic-ai/claude-code/cli.js\n\
             200 201 alice 1.0 70000 00:20 node /x/@anthropic-ai/claude-code/cli.js\n\
             100 1 alice 1.0 70000 00:40 node /x/@anthropic-ai/claude-code/cli.js\n",
        );
        let mut sessions = HashMap::new();
        sessions.insert(
            Provider::Claude,
            vec![
                AgentSessionMetadata {
                    session_id: Some("newest".to_string()),
                    pid_hint: Some(200),
                    context_used_tokens: Some(200),
                    ..Default::default()
                },
                AgentSessionMetadata {
                    session_id: Some("older".to_string()),
                    pid_hint: Some(100),
                    context_used_tokens: Some(100),
                    ..Default::default()
                },
            ],
        );
        let metrics = build_agent_metrics(&processes, &sessions, &HashMap::new());
        let by_pid = metrics
            .iter()
            .map(|metric| (metric.root_pid, metric))
            .collect::<HashMap<_, _>>();
        assert_eq!(by_pid[&100].session_id.as_deref(), Some("older"));
        assert_eq!(by_pid[&100].context_used_tokens, Some(100));
        assert_eq!(by_pid[&201].session_id.as_deref(), Some("newest"));
        assert_eq!(by_pid[&201].context_used_tokens, Some(200));
        assert!(
            !by_pid.contains_key(&200),
            "the launcher tree owns the agent"
        );
    }

    #[test]
    fn keeps_codex_quota_when_a_later_event_omits_it() {
        let lines = vec![
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":1000},"model_context_window":10000},"rate_limits":{"limit_id":"codex","plan_type":"plus","credits":{"has_credits":false,"balance":"0"},"primary":{"used_percent":2,"window_minutes":300,"resets_at":1799500000},"secondary":{"used_percent":17,"window_minutes":10080,"resets_at":1799900000}}}}"#.to_string(),
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":2000},"model_context_window":10000}}}"#.to_string(),
        ];
        let metadata = parse_metadata(Provider::Codex, &lines);
        assert_eq!(metadata.total_tokens, Some(2000));
        assert_eq!(metadata.rate_limits.len(), 2);
        assert!(metadata
            .rate_limits
            .iter()
            .all(|limit| !limit.stale && limit.used_percent.is_some()));
    }

    #[test]
    fn marks_rolled_over_windows_as_stale_and_accepts_relative_resets() {
        let lines = vec![r#"{"rate_limits":{"five_hour":{"used_percentage":80,"resets_at":1798000000},"seven_day":{"used_percentage":40,"resets_in_seconds":3600}}}"#.to_string()];
        let metadata = parse_metadata(Provider::Claude, &lines);
        let five_hour = metadata
            .rate_limits
            .iter()
            .find(|limit| limit.window_minutes == Some(300))
            .unwrap();
        assert!(five_hour.stale);
        let weekly = metadata
            .rate_limits
            .iter()
            .find(|limit| limit.window_minutes == Some(10080))
            .unwrap();
        assert!(!weekly.stale);
        assert_eq!(weekly.resets_at, Some(TEST_NOW + 3600));
    }

    #[test]
    fn normalizes_millisecond_reset_timestamps_to_seconds() {
        // Both reset renderers in the webview and the staleness check here read
        // seconds, so a millisecond-emitting provider must be converted once at
        // the parse boundary rather than guessed at each display site.
        let lines = vec![
            r#"{"rate_limits":{"five_hour":{"used_percentage":10,"resets_at":1799500000000}}}"#
                .to_string(),
        ];
        let metadata = parse_metadata(Provider::Claude, &lines);
        let limit = &metadata.rate_limits[0];
        assert_eq!(limit.resets_at, Some(1_799_500_000));
        assert!(!limit.stale);
    }

    #[test]
    fn reports_snapshot_age_from_capture_time() {
        let lines = vec![r#"{"session_id":"claude-1","captured_at":1798999880,"pid":4242,"rate_limits":{"five_hour":{"used_percentage":10,"resets_at":1799500000}}}"#.to_string()];
        let metadata = parse_metadata(Provider::Claude, &lines);
        assert_eq!(metadata.snapshot_age_seconds, Some(120));
        assert_eq!(metadata.pid_hint, Some(4242));
    }

    #[test]
    fn builds_metadata_command_only_for_running_providers() {
        let claude_only = metadata_command(RemoteOs::Linux, &HashSet::from([Provider::Claude]));
        assert!(claude_only.contains("emit_agent_files claude"));
        assert!(claude_only.contains("emit_agent_snapshots claude"));
        assert!(!claude_only.contains("emit_agent_files codex"));
        assert!(!claude_only.contains("sqlite3"));

        let agy_only = metadata_command(RemoteOs::Linux, &HashSet::from([Provider::Agy]));
        assert!(agy_only.contains("sqlite3"));
        assert!(agy_only.contains("emit_agent_snapshots agy"));
        assert!(!agy_only.contains("emit_agent_files claude"));

        let windows = metadata_command(RemoteOs::Windows, &HashSet::from([Provider::Claude]));
        assert!(windows.contains("Emit-AgentFiles 'claude'"));
        assert!(windows.contains("Emit-AgentSnapshots 'claude'"));
        assert!(windows.trim_end().ends_with("exit 0"));
    }

    #[test]
    fn codex_live_account_response_normalizes_used_to_remaining() {
        let response = serde_json::json!({
            "id": 2,
            "result": {
                "rateLimits": {
                    "limitId": "codex",
                    "primary": {
                        "usedPercent": 3,
                        "windowDurationMins": 10080,
                        "resetsAt": TEST_NOW + 3600
                    },
                    "secondary": null
                },
                "rateLimitsByLimitId": {
                    "codex": {
                        "limitId": "codex",
                        "primary": {
                            "usedPercent": 3,
                            "windowDurationMins": 10080,
                            "resetsAt": TEST_NOW + 3600
                        },
                        "secondary": null
                    }
                }
            }
        });
        let quota = parse_codex_quota_response(&response, TEST_NOW).unwrap();
        assert_eq!(quota.source, "codex-app-server");
        assert_eq!(quota.limits.len(), 1);
        assert_eq!(quota.limits[0].window_minutes, Some(10080));
        assert_eq!(quota.limits[0].used_percent, Some(3.0));
        assert_eq!(quota.limits[0].remaining_percent, Some(97.0));
    }

    #[test]
    fn reports_the_blocking_setup_step_instead_of_a_generic_absence() {
        let cases = [
            ("missing", "ours", "not installed"),
            ("empty", "ours", "present but empty"),
            ("ok", "none", "no status line configured"),
            ("ok", "other", "different status line"),
            ("ok", "ours", "Send one message"),
        ];
        for (helper, status_line, expected) in cases {
            let output = format!(
                "__GPUTERM_AGENT_FILE__\tclaude\tsetup-state\n\
                 {{\"scope\":\"setup\",\"helper\":\"{helper}\",\"status_line\":\"{status_line}\"}}\n\
                 __GPUTERM_AGENT_END__\n"
            );
            let mut state = AgentMonitorState {
                metadata: parse_metadata_output(&output, TEST_NOW),
                ..Default::default()
            };
            merge_metadata_quota_fallbacks(
                &mut state,
                &HashSet::from([Provider::Claude]),
                TEST_NOW,
            );
            let quota = state.quotas.get(&Provider::Claude).unwrap();
            assert_eq!(quota.status, "setup-required");
            let message = quota.message.as_deref().unwrap_or_default();
            assert!(
                message.contains(expected),
                "helper={helper} status_line={status_line} produced {message:?}"
            );
        }
    }

    #[test]
    fn setup_state_record_is_not_treated_as_a_session() {
        let output = concat!(
            "__GPUTERM_AGENT_FILE__\tclaude\tsetup-state\n",
            "{\"scope\":\"setup\",\"helper\":\"ok\",\"status_line\":\"ours\"}\n",
            "__GPUTERM_AGENT_END__\n",
        );
        let grouped = parse_metadata_output(output, TEST_NOW);
        let sessions = grouped.get(&Provider::Claude).unwrap();
        assert!(sessions.iter().all(|entry| entry.setup_scope));
        // It names no session, so it must never become an agent card's metadata.
        assert!(sessions.iter().all(|entry| entry.session_id.is_none()));
    }

    #[test]
    fn remote_paths_use_sftp_separators_and_native_windows_drives() {
        assert_eq!(
            remote_join(Path::new("/home/alice"), ".claude"),
            "/home/alice/.claude"
        );
        assert_eq!(remote_join(Path::new("/home/alice/"), ".claude"), "/home/alice/.claude");
        // Windows OpenSSH reports the home directory in this form.
        assert_eq!(
            remote_join(Path::new("/C:/Users/Test User"), ".claude"),
            "/C:/Users/Test User/.claude"
        );
        // A `-File` argument needs the drive without the SFTP root slash.
        assert_eq!(
            native_remote_path("/C:/Users/Test User/.claude/x.ps1"),
            "C:/Users/Test User/.claude/x.ps1"
        );
        assert_eq!(native_remote_path("/home/alice/.claude/x.sh"), "/home/alice/.claude/x.sh");
    }

    #[test]
    fn claude_settings_parsing_tolerates_a_bom_and_an_empty_file() {
        // PowerShell redirection and Windows editors both write a UTF-8 BOM,
        // which `serde_json` rejects outright.
        let with_bom = b"\xef\xbb\xbf{\"statusLine\":{\"command\":\"x\"}}";
        let parsed = parse_claude_settings(with_bom).expect("BOM is tolerated");
        assert_eq!(claude_status_line_command(&parsed).as_deref(), Some("x"));

        assert!(parse_claude_settings(b"").unwrap().is_object());
        assert!(parse_claude_settings(b"   \n").unwrap().is_object());
        assert!(parse_claude_settings(b"not json").is_err());
    }

    #[test]
    fn account_snapshot_supplies_the_quota_when_every_session_lacks_limits() {
        // The situation observed on a real machine: Claude only publishes
        // rate limits after a session's first API response, so short-lived
        // sessions kept writing newer, quota-less snapshots and pushed the only
        // useful reading out of the collector's newest-files window.
        let mut output = String::from(concat!(
            "__GPUTERM_AGENT_FILE__\tclaude\taccount.json\n",
            "{\"scope\":\"account\",\"captured_at\":1798999900,\"session_id\":\"published\",\"rate_limits\":{\"five_hour\":{\"used_percentage\":20,\"resets_at\":1799500000},\"seven_day\":{\"used_percentage\":40,\"resets_at\":1799900000}}}\n",
            "__GPUTERM_AGENT_END__\n",
        ));
        for index in 0..4 {
            output.push_str(&format!(
                "__GPUTERM_AGENT_FILE__\tclaude\tshort-{index}.json\n\
                 {{\"session_id\":\"short-{index}\",\"captured_at\":1798999990,\"cwd\":\"/w\",\"cost\":{{\"total_cost_usd\":0.0,\"total_duration_ms\":236}}}}\n\
                 __GPUTERM_AGENT_END__\n"
            ));
        }

        let mut state = AgentMonitorState {
            metadata: parse_metadata_output(&output, TEST_NOW),
            ..Default::default()
        };
        merge_metadata_quota_fallbacks(&mut state, &HashSet::from([Provider::Claude]), TEST_NOW);

        let quota = state.quotas.get(&Provider::Claude).unwrap();
        assert_eq!(quota.status, "available");
        assert_eq!(quota.source, "claude-statusline");
        let five_hour = quota
            .limits
            .iter()
            .find(|limit| limit.window_minutes == Some(300))
            .expect("five hour window");
        assert_eq!(five_hour.remaining_percent, Some(80.0));
    }

    #[test]
    fn account_snapshot_does_not_overwrite_its_session_context() {
        // The account record names the session that published it, so merging by
        // session id would let it wipe that session's own cost and context.
        let output = concat!(
            "__GPUTERM_AGENT_FILE__\tclaude\tpublished.json\n",
            "{\"session_id\":\"published\",\"captured_at\":1798999900,\"cwd\":\"/work\",\"cost\":{\"total_cost_usd\":1.25,\"total_duration_ms\":165296},\"context_window\":{\"total_input_tokens\":150,\"context_window_size\":1000}}\n",
            "__GPUTERM_AGENT_END__\n",
            "__GPUTERM_AGENT_FILE__\tclaude\taccount.json\n",
            "{\"scope\":\"account\",\"captured_at\":1798999950,\"session_id\":\"published\",\"rate_limits\":{\"five_hour\":{\"used_percentage\":20,\"resets_at\":1799500000}}}\n",
            "__GPUTERM_AGENT_END__\n",
        );
        let grouped = parse_metadata_output(output, TEST_NOW);
        let sessions = grouped.get(&Provider::Claude).unwrap();
        let session = sessions
            .iter()
            .find(|entry| !entry.account_scope)
            .expect("session record");
        assert_eq!(session.cwd.as_deref(), Some("/work"));
        assert_eq!(session.cost_usd, Some(1.25));
        assert_eq!(session.context_used_tokens, Some(150));
        assert!(sessions.iter().any(|entry| entry.account_scope));
    }

    #[test]
    fn collector_emits_the_account_snapshot_by_name() {
        let posix = metadata_command(RemoteOs::MacOs, &HashSet::from([Provider::Claude]));
        assert!(posix.contains("emit_agent_tail \"$provider\" \"$dir/account.json\""));
        // The recency-limited scan must skip it, or it would occupy one of the
        // four session slots.
        assert!(posix.contains("! -name 'account.json'"));

        let windows = metadata_command(RemoteOs::Windows, &HashSet::from([Provider::Claude]));
        assert!(windows.contains("Emit-AgentTail $provider (Join-Path $dir 'account.json')"));
        assert!(windows.contains("$_.Name -ne 'account.json'"));
    }

    #[test]
    fn newest_account_quota_is_shared_instead_of_assigned_by_session() {
        let older = AgentSessionMetadata {
            session_id: Some("older".to_string()),
            captured_at: Some(TEST_NOW - 100),
            rate_limits: vec![AgentRateLimitMetric {
                label: "primary".to_string(),
                remaining_percent: Some(12.0),
                used_percent: Some(88.0),
                window_minutes: Some(10080),
                ..Default::default()
            }],
            ..Default::default()
        };
        let newest = AgentSessionMetadata {
            session_id: Some("newest".to_string()),
            captured_at: Some(TEST_NOW - 5),
            rate_limits: vec![AgentRateLimitMetric {
                label: "primary".to_string(),
                remaining_percent: Some(97.0),
                used_percent: Some(3.0),
                window_minutes: Some(10080),
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut state = AgentMonitorState::default();
        state.metadata.insert(Provider::Codex, vec![older, newest]);
        merge_metadata_quota_fallbacks(&mut state, &HashSet::from([Provider::Codex]), TEST_NOW);
        let quota = state.quotas.get(&Provider::Codex).unwrap();
        assert_eq!(quota.source, "codex-session-log");
        assert_eq!(quota.limits[0].remaining_percent, Some(97.0));

        let processes = parse_posix_processes(
            "100 1 alice 1.0 1000 01:00 codex exec first\n\
             200 1 alice 1.0 1000 01:00 codex exec second\n",
        );
        let metrics = build_agent_metrics(&processes, &state.metadata, &state.quotas);
        assert_eq!(metrics.len(), 2);
        assert!(metrics
            .iter()
            .all(|metric| metric.quota.limits[0].remaining_percent == Some(97.0)));
    }

    #[test]
    fn advances_quota_age_and_marks_windows_expired() {
        let mut state = AgentMonitorState::default();
        state.quotas.insert(
            Provider::Codex,
            AgentQuotaSnapshot::available(
                "codex-app-server",
                Some(TEST_NOW - 180),
                vec![AgentRateLimitMetric {
                    label: "primary".to_string(),
                    remaining_percent: Some(97.0),
                    resets_at: Some(TEST_NOW + 30),
                    ..Default::default()
                }],
                TEST_NOW - 180,
            ),
        );

        update_quota_snapshots(&mut state, TEST_NOW + 120);
        let quota = state.quotas.get(&Provider::Codex).unwrap();
        assert_eq!(quota.snapshot_age_seconds, Some(300));
        assert_eq!(quota.status, "stale");
        assert!(quota.limits[0].stale);
    }

    #[test]
    fn parses_agy_usage_tui_groups_remaining_and_refresh_times() {
        let output = concat!(
            "\u{1b}[1mGEMINI MODELS\u{1b}[0m\r\n",
            "Models within this group: Gemini Flash, Gemini Pro\r\n",
            "Weekly Limit\r\n",
            "[||||] 99.90%\r\n",
            "100% remaining · Refreshes in 95h 58m\r\n",
            "Five Hour Limit\r\n",
            "[||||] 100.00%\r\n",
            "Quota available\r\n",
            "CLAUDE AND GPT MODELS\r\n",
            "Models within this group: Claude Opus, Claude Sonnet, GPT-OSS\r\n",
            "Weekly Limit\r\n",
            "[||||] 100.00%\r\n",
            "Quota available\r\n",
            "Five Hour Limit\r\n",
            "[||||] 100.00%\r\n",
            "Quota available\r\n",
        );
        let quota = parse_agy_usage_output(output, TEST_NOW).unwrap();
        assert_eq!(quota.source, "agy-usage-tui");
        assert_eq!(quota.limits.len(), 4);
        assert_eq!(quota.limits[0].group.as_deref(), Some("Gemini models"));
        assert_eq!(quota.limits[0].remaining_percent, Some(99.9));
        assert_eq!(
            quota.limits[0].resets_at,
            Some(TEST_NOW + 95 * 3600 + 58 * 60)
        );
        assert_eq!(
            quota.limits[0].model_names,
            vec!["Gemini Flash", "Gemini Pro"]
        );
        assert_eq!(quota.limits[1].remaining_percent, Some(100.0));
        assert_eq!(
            quota.limits[2].group.as_deref(),
            Some("Claude and GPT models")
        );
        assert_eq!(quota.limits[2].remaining_percent, Some(100.0));
        assert_eq!(
            quota.limits[2].model_names,
            vec!["Claude Opus", "Claude Sonnet", "GPT-OSS"]
        );
    }

    #[test]
    fn agy_usage_redraws_keep_the_latest_precise_value_and_wrapped_model_names() {
        let first = concat!(
            "GEMINI MODELS\n",
            "Models within this group: Gemini Flash, Gemini Pro\n",
            "Weekly Limit\n[||||] 80.25%\n80% remaining\n",
            "Five Hour Limit\n[||||] 90.50%\n91% remaining\n",
            "CLAUDE AND GPT MODELS\n",
            "Models within this group: Claude Opus, Claude\n",
            "Sonnet, GPT-OSS\n",
            "Weekly Limit\n[||||] 70.75%\n71% remaining\n",
            "Five Hour Limit\n[||||] 60.25%\n60% remaining\n",
        );
        let redraw = concat!(
            "GEMINI MODELS\n",
            "Models within this group: Gemini Flash, Gemini Pro\n",
            "Weekly Limit\n[||||] 79.95%\n80% remaining\n",
            "Five Hour Limit\n[||||] 89.75%\n90% remaining\n",
            "CLAUDE AND GPT MODELS\n",
            "Models within this group: Claude Opus, Claude Sonnet, GPT-OSS\n",
            "Weekly Limit\n[||||] 69.50%\n70% remaining\n",
            "Five Hour Limit\n[||||] 59.95%\n60% remaining\n",
        );
        let quota = parse_agy_usage_output(&format!("{}{}", first, redraw), TEST_NOW).unwrap();
        assert_eq!(quota.limits.len(), 4);
        assert_eq!(quota.limits[0].remaining_percent, Some(79.95));
        assert_eq!(quota.limits[1].remaining_percent, Some(89.75));
        assert_eq!(quota.limits[2].remaining_percent, Some(69.5));
        assert_eq!(quota.limits[3].remaining_percent, Some(59.95));
        assert_eq!(
            quota.limits[2].model_names,
            vec!["Claude Opus", "Claude Sonnet", "GPT-OSS"]
        );
    }

    #[test]
    fn parses_cursor_positioned_agy_output_and_joined_bar_status_lines() {
        let output = concat!(
            "\u{1b}[1;1HGEMINI MODELS",
            "\u{1b}[2;1HModels within this group: Gemini Flash, Gemini Pro",
            "\u{1b}[3;1HWeekly Limit",
            "\u{1b}[4;1H[||||] 99.90% 100% remaining · Refreshes in 95h 58m",
            "\u{1b}[5;1HFive Hour Limit",
            "\u{1b}[6;1H[||||] 100.00% Quota available",
            "\u{1b}[7;1HCLAUDE AND GPT MODELS",
            "\u{1b}[8;1HModels within this group: Claude Opus, Claude Sonnet, GPT-OSS",
            "\u{1b}[9;1HWeekly Limit",
            "\u{1b}[10;1H[||||] 100.00% Quota available",
            "\u{1b}[11;1HFive Hour Limit",
            "\u{1b}[12;1H[||||] 100.00% Quota available",
        );
        let quota = parse_agy_usage_output(output, TEST_NOW).unwrap();
        assert_eq!(quota.limits.len(), 4);
        assert_eq!(quota.limits[0].remaining_percent, Some(99.9));
        assert_eq!(
            quota.limits[0].resets_at,
            Some(TEST_NOW + 95 * 3600 + 58 * 60)
        );
        assert!(quota
            .limits
            .iter()
            .all(|limit| !limit.model_names.is_empty()));
    }

    #[test]
    fn waits_for_both_agy_model_groups_before_finishing_the_probe() {
        let gemini_only = concat!(
            "GEMINI MODELS\n",
            "Weekly Limit\n100% remaining\n",
            "Five Hour Limit\n99% remaining\n",
        );
        assert!(!agy_usage_output_complete(gemini_only));

        let complete = concat!(
            "GEMINI MODELS\n",
            "Weekly Limit\n100% remaining\n",
            "Five Hour Limit\n99% remaining\n",
            "CLAUDE AND GPT MODELS\n",
            "Weekly Limit\nQuota available\n",
            "Five Hour Limit\nQuota available\n",
        );
        assert!(agy_usage_output_complete(complete));
    }

    #[test]
    fn agy_startup_requires_visible_output_and_writes_only_slash_usage_once() {
        let (control_sender, control_receiver) = mpsc::channel();
        control_sender.send(b"\x1b[?1049h".to_vec()).unwrap();
        assert!(wait_for_agy_startup(&control_receiver, Duration::from_millis(2)).is_err());

        let (ready_sender, ready_receiver) = mpsc::channel();
        ready_sender
            .send(b"\x1b[?1049hAGY ready".to_vec())
            .unwrap();
        assert!(wait_for_agy_startup(&ready_receiver, Duration::from_millis(2)).is_ok());

        let mut written = Vec::new();
        write_agy_usage_command(&mut written).unwrap();
        assert_eq!(written, b"/usage\r");
    }

    #[test]
    fn agy_probe_output_collection_stops_at_timeout_without_inventing_data() {
        let (_sender, receiver) = mpsc::channel::<Vec<u8>>();
        let output = collect_pty_probe_output(&receiver, Duration::from_millis(2));
        assert!(output.is_empty());
        assert!(parse_agy_usage_output("", TEST_NOW).is_err());
    }

    #[test]
    fn agy_history_replaces_five_minute_buckets_and_keeps_failure_gaps() {
        let base = TEST_NOW / AGY_QUOTA_HISTORY_BUCKET_SECONDS * AGY_QUOTA_HISTORY_BUCKET_SECONDS;
        let mut history = Vec::new();
        let available = |captured_at, remaining_percent| AgentQuotaHistoryPoint {
            captured_at,
            status: "available".to_string(),
            limits: vec![AgentQuotaHistoryLimit {
                group: Some("Gemini models".to_string()),
                window_minutes: 300,
                remaining_percent: Some(remaining_percent),
            }],
        };
        let unavailable = |captured_at| AgentQuotaHistoryPoint {
            captured_at,
            status: "unavailable".to_string(),
            limits: Vec::new(),
        };

        upsert_agy_history_point(&mut history, available(base + 1, 90.0), base + 1);
        upsert_agy_history_point(&mut history, unavailable(base + 20), base + 20);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].status, "unavailable");
        assert!(history[0].limits.is_empty());

        upsert_agy_history_point(&mut history, available(base + 40, 88.0), base + 40);
        upsert_agy_history_point(
            &mut history,
            unavailable(base + AGY_QUOTA_HISTORY_BUCKET_SECONDS + 1),
            base + AGY_QUOTA_HISTORY_BUCKET_SECONDS + 1,
        );
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].limits[0].remaining_percent, Some(88.0));
        assert_eq!(history[1].status, "unavailable");
    }

    #[test]
    fn agy_history_is_memory_only_bounded_and_shared_across_reconnects() {
        let histories = AgentQuotaHistories::default();
        let key = "ssh:alice@example.test:22".to_string();
        let base = TEST_NOW / AGY_QUOTA_HISTORY_BUCKET_SECONDS * AGY_QUOTA_HISTORY_BUCKET_SECONDS;
        let quota = AgentQuotaSnapshot::available(
            "agy-usage-tui",
            Some(base),
            vec![
                AgentRateLimitMetric {
                    label: "five_hour".to_string(),
                    group: Some("Gemini models".to_string()),
                    remaining_percent: Some(91.0),
                    window_minutes: Some(300),
                    ..Default::default()
                },
                AgentRateLimitMetric {
                    label: "weekly".to_string(),
                    group: Some("Gemini models".to_string()),
                    remaining_percent: Some(84.0),
                    window_minutes: Some(10_080),
                    ..Default::default()
                },
            ],
            base,
        );

        let mut first_connection = AgentMonitorState::default();
        first_connection.configure_agy_history(key.clone(), histories.clone());
        record_agy_history(&mut first_connection, base, Some(&quota));

        let mut reconnected = AgentMonitorState::default();
        reconnected.configure_agy_history(key, histories.clone());
        assert_eq!(reconnected.agy_history.len(), 1);
        assert_eq!(reconnected.agy_history[0].limits.len(), 2);

        let mut stored = histories.lock().unwrap();
        let history = stored.values_mut().next().unwrap();
        for index in 1..=300 {
            let captured_at = base + index * AGY_QUOTA_HISTORY_BUCKET_SECONDS;
            upsert_agy_history_point(
                history,
                AgentQuotaHistoryPoint {
                    captured_at,
                    status: "unavailable".to_string(),
                    limits: Vec::new(),
                },
                captured_at,
            );
        }
        assert_eq!(history.len(), AGY_QUOTA_HISTORY_MAX_POINTS);
        assert!(
            history.last().unwrap().captured_at - history.first().unwrap().captured_at
                < AGY_QUOTA_HISTORY_WINDOW_SECONDS
        );
        assert!(history
            .iter()
            .all(|point| point.status == "available" || point.status == "unavailable"));
    }

    #[test]
    fn detects_custom_claude_status_line_without_overwriting_it() {
        let settings = serde_json::json!({
            "statusLine": {
                "type": "command",
                "command": "~/.claude/my-status.sh"
            }
        });
        assert_eq!(
            claude_status_line_command(&settings).as_deref(),
            Some("~/.claude/my-status.sh")
        );
    }

    #[test]
    fn windows_claude_setup_uses_a_shell_independent_powershell_helper() {
        let (helper_name, helper) = claude_helper_for_os(RemoteOs::Windows);
        assert_eq!(helper_name, "gputerm-claude-statusline.ps1");
        assert!(helper.contains("[Console]::In.ReadToEnd()"));
        assert!(helper.contains(".cache\\gputerm\\agent-status\\claude"));
        assert!(!helper.contains("gputerm-claude-statusline.py"));

        let command = claude_status_line_command_for(
            RemoteOs::Windows,
            r"C:\Users\Test User\.claude\gputerm-claude-statusline.ps1",
        );
        assert_eq!(
            command,
            "powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"C:/Users/Test User/.claude/gputerm-claude-statusline.ps1\""
        );
        assert!(!command.contains("%USERPROFILE%"));
        assert!(!command.contains("python"));

        let mut settings = serde_json::json!({});
        set_claude_status_line(&mut settings, &command);
        assert_eq!(
            settings.pointer("/statusLine/refreshInterval"),
            Some(&serde_json::json!(30))
        );
        assert_eq!(
            claude_status_line_command(&settings).as_deref(),
            Some(command.as_str())
        );
    }

    #[test]
    fn windows_provider_probe_prefers_the_observed_native_executable() {
        let processes = vec![
            ProcessSample {
                name: "node.exe".to_string(),
                command: r#""C:\Program Files\nodejs\node.exe" C:\tools\codex\cli.js"#
                    .to_string(),
                executable_path: Some(r"C:\Program Files\nodejs\node.exe".to_string()),
                ..Default::default()
            },
            ProcessSample {
                name: "codex.exe".to_string(),
                command: r#""C:\Users\Test User\AppData\Roaming\npm\node_modules\@openai\codex\vendor\codex.exe""#
                    .to_string(),
                executable_path: Some(
                    r"C:\Users\Test User\AppData\Roaming\npm\node_modules\@openai\codex\vendor\codex.exe"
                        .to_string(),
                ),
                ..Default::default()
            },
            ProcessSample {
                name: "agy.exe".to_string(),
                command: r#""C:\Users\Test User\bin\agy.exe""#.to_string(),
                executable_path: Some(r"C:\Users\Test User\bin\agy.exe".to_string()),
                ..Default::default()
            },
        ];

        assert_eq!(
            provider_executable_hint(&processes, Provider::Codex).as_deref(),
            Some(
                r"C:\Users\Test User\AppData\Roaming\npm\node_modules\@openai\codex\vendor\codex.exe"
            )
        );
        assert_eq!(
            provider_executable_hint(&processes, Provider::Agy).as_deref(),
            Some(r"C:\Users\Test User\bin\agy.exe")
        );
    }

    #[test]
    fn macos_claude_setup_uses_the_builtin_jxa_runtime() {
        let (helper_name, helper) = claude_helper_for_os(RemoteOs::MacOs);
        assert_eq!(helper_name, "gputerm-claude-statusline-macos.js");
        assert!(helper.starts_with("#!/usr/bin/osascript -l JavaScript"));
        assert!(helper.contains("NSFileHandle.fileHandleWithStandardInput"));
        assert!(helper.contains("rate_limits"));
        assert!(!helper.contains("python"));
        assert!(local_claude_helper_runtime_available(RemoteOs::MacOs));
        assert_eq!(
            claude_status_line_command_for(RemoteOs::MacOs, "/ignored"),
            "/usr/bin/osascript -l JavaScript \"$HOME/.claude/gputerm-claude-statusline-macos.js\""
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn executes_macos_claude_status_line_and_writes_a_quota_snapshot() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "gputerm-claude-statusline-macos-{}-{}",
            std::process::id(),
            unique
        ));
        let session_id = format!("gputerm-macos-test-{}-{}", std::process::id(), unique);
        let script_path = root.join("gputerm-claude-statusline-macos.js");
        let snapshot_path = root
            .join(".cache")
            .join("gputerm")
            .join("agent-status")
            .join("claude")
            .join(format!("{}.json", session_id));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            &script_path,
            include_str!("../../../scripts/gputerm-claude-statusline-macos.js"),
        )
        .unwrap();

        let mut child = Command::new("/usr/bin/osascript")
            .args(["-l", "JavaScript"])
            .arg(&script_path)
            .env("HOME", &root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let payload = serde_json::json!({
            "session_id": session_id,
            "model": { "display_name": "Opus" },
            "context_window": { "used_percentage": 25 },
            "rate_limits": {
                "five_hour": { "used_percentage": 20, "resets_at": 2_000_000_000_u64 },
                "seven_day": { "used_percentage": 40, "resets_at": 2_000_100_000_u64 }
            },
            "prompt": "must not be copied"
        })
        .to_string();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        let snapshot = fs::read_to_string(&snapshot_path).unwrap();
        let _ = fs::remove_dir_all(&root);

        assert!(
            output.status.success(),
            "JXA helper failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Opus"));
        assert!(stdout.contains("5h 80%"));
        assert!(stdout.contains("wk 60%"));
        let snapshot: Value = serde_json::from_str(&snapshot).unwrap();
        assert_eq!(
            snapshot
                .pointer("/rate_limits/five_hour/used_percentage")
                .and_then(Value::as_f64),
            Some(20.0)
        );
        assert!(snapshot.get("prompt").is_none());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn executes_windows_claude_status_line_and_writes_a_quota_snapshot() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "gputerm-claude-statusline-{}-{}",
            std::process::id(),
            unique
        ));
        let session_id = format!("gputerm-windows-test-{}-{}", std::process::id(), unique);
        let snapshot_path = PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join(".cache")
            .join("gputerm")
            .join("agent-status")
            .join("claude")
            .join(format!("{}.json", session_id));
        let script_path = root.join("gputerm-claude-statusline.ps1");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            &script_path,
            include_str!("../../../scripts/gputerm-claude-statusline.ps1"),
        )
        .unwrap();

        let mut child = Command::new("powershell.exe")
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(&script_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let payload = serde_json::json!({
            "session_id": session_id,
            "model": { "display_name": "Opus" },
            "context_window": { "used_percentage": 25 },
            "rate_limits": {
                "five_hour": { "used_percentage": 20, "resets_at": 2_000_000_000_u64 },
                "seven_day": { "used_percentage": 40, "resets_at": 2_000_100_000_u64 }
            }
        })
        .to_string();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        let snapshot = fs::read_to_string(&snapshot_path).unwrap();
        let _ = fs::remove_file(&snapshot_path);
        let _ = fs::remove_dir_all(&root);

        assert!(
            output.status.success(),
            "PowerShell helper failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Opus"));
        assert!(stdout.contains("5h 80%"));
        assert!(stdout.contains("wk 60%"));
        let snapshot: Value = serde_json::from_str(&snapshot).unwrap();
        assert_eq!(
            snapshot
                .pointer("/rate_limits/five_hour/used_percentage")
                .and_then(Value::as_f64),
            Some(20.0)
        );
        assert_eq!(
            snapshot
                .pointer("/rate_limits/seven_day/used_percentage")
                .and_then(Value::as_f64),
            Some(40.0)
        );
    }

    #[test]
    fn posix_claude_setup_keeps_the_executable_status_line_helper() {
        let (helper_name, helper) = claude_helper_for_os(RemoteOs::Linux);
        assert_eq!(helper_name, "gputerm-claude-statusline.sh");
        assert!(helper.starts_with("#!/bin/sh"));
        assert_eq!(
            claude_status_line_command_for(RemoteOs::Linux, "/ignored"),
            "~/.claude/gputerm-claude-statusline.sh"
        );
    }

    #[test]
    fn local_install_is_repeatable_and_never_leaves_an_empty_helper() {
        // Reproduces the reported macOS failure: a second Set up used to
        // truncate the working helper to zero bytes, and a BOM-prefixed
        // settings.json aborted the whole install.
        let root = std::env::temp_dir().join(format!("gputerm-install-{}", uuid::Uuid::new_v4()));
        let claude_dir = root.join(".claude");
        fs::create_dir_all(&claude_dir).expect("create scratch home");
        fs::write(claude_dir.join("gputerm-claude-statusline.sh"), "").expect("stale variant");
        fs::write(
            claude_dir.join("settings.json"),
            b"\xef\xbb\xbf{\"theme\":\"dark\"}",
        )
        .expect("BOM settings");

        let os = local_os();
        let (helper_name, helper_contents) = claude_helper_for_os(os);
        let helper_path = claude_dir.join(helper_name);
        let settings_path = claude_dir.join("settings.json");

        for round in 0..2 {
            let (mut settings, existing) =
                read_claude_settings(&settings_path).expect("settings readable");
            if round == 1 {
                assert!(existing.is_some(), "the second run must see our own command");
            }
            let desired =
                claude_status_line_command_for(os, helper_path.to_string_lossy().as_ref());
            write_claude_helper(&helper_path, helper_contents).expect("helper installed");
            set_claude_status_line(&mut settings, &desired);
            backup_and_write_json(&settings_path, &settings).expect("settings written");
            remove_stale_claude_helpers(&claude_dir, helper_name);

            let written = fs::read_to_string(&helper_path).expect("helper readable");
            assert_eq!(written.len(), helper_contents.len(), "round {round}");
        }

        let settings = fs::read_to_string(&settings_path).expect("settings readable");
        assert!(settings.contains("gputerm-claude-statusline"));
        assert!(settings.contains("dark"), "unrelated keys survive");
        assert!(
            !claude_dir.join(format!("{helper_name}.gputerm-new")).exists(),
            "no temporary file is left behind"
        );
        if helper_name != "gputerm-claude-statusline.sh" {
            assert!(
                !claude_dir.join("gputerm-claude-statusline.sh").exists(),
                "the superseded variant is removed"
            );
        }

        fs::remove_dir_all(root).expect("remove scratch home");
    }

    #[test]
    fn parses_elapsed_time_formats() {
        assert_eq!(parse_elapsed("01:30"), Some(90));
        assert_eq!(parse_elapsed("02:01:30"), Some(7290));
        assert_eq!(parse_elapsed("3-02:01:30"), Some(266_490));
    }
}
