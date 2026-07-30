use crate::ssh::agent_monitor::{self, AgentMetric, AgentMonitorState, AgentQuotaHistories};
use crate::ssh::gpu_monitor::{
    append_uncovered_linux_drm, parse_gpu_probe, parse_intel_gpu_top_stream, parse_linux_drm_gpus,
    parse_nvidia_smi_csv, parse_rocm_smi_json, parse_xpu_discovery, parse_xpu_stats,
    xpu_stats_command, GpuMetric, GpuProbe, GPU_PROBE_COMMAND, INTEL_GPU_TOP_COMMAND,
    LINUX_DRM_GPU_COMMAND, NVIDIA_SMI_QUERY, XPU_DISCOVERY_COMMAND,
};
use crate::ssh::macos_monitor;
use crate::ssh::parse_util::{
    kib_to_mib, parse_average_clock, parse_cpu_model, parse_first_u64, parse_loadavg,
    parse_lscpu_value, parse_meminfo_values, required_section, split_sections,
};
use crate::ssh::session::{open_ssh_session, AgentQuotaRefreshes, AppState, SshTarget};
use crate::ssh::windows_monitor;
use base64::Engine as _;
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use ssh2::Session;
use std::collections::hash_map::DefaultHasher;
use std::ffi::OsString;
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, State};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
#[cfg(target_os = "windows")]
use std::path::PathBuf;

const COMMAND_TIMEOUT_SECS: u64 = 3;
const COMMAND_TIMEOUT_MS: u32 = 3_000;
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Windows PowerShell 5.1 cold-starts in 0.5–2 s and the batched telemetry
/// script samples counters for 500 ms, so Windows remotes get a longer
/// libssh2 session timeout than the Unix paths.
pub(crate) const WINDOWS_COMMAND_TIMEOUT_MS: u32 = 10_000;
/// Upper bound for one local collector run. Generous enough for a slow
/// PowerShell start-up, small enough that a wedged command cannot stall the
/// telemetry thread indefinitely.
const LOCAL_COMMAND_TIMEOUT: Duration = Duration::from_secs(15);
const LOCAL_COMMAND_POLL_INTERVAL: Duration = Duration::from_millis(25);
/// Head-room so the remote `timeout` wrapper, not libssh2's read timeout,
/// is what bounds a command.
const SESSION_TIMEOUT_SLACK_MS: u64 = 3_000;
const DEFAULT_INTERVAL_SECS: u64 = 2;
const DEFAULT_IGNORED_FS_TYPES: &[&str] = &[
    "tmpfs", "devtmpfs", "squashfs", "proc", "sysfs", "cgroup", "cgroup2", "overlay", "devfs",
    "autofs",
];

const CPU_COMMAND: &str = "printf '__PROC_STAT__\\n'; cat /proc/stat 2>/dev/null; printf '\\n__LOADAVG__\\n'; cat /proc/loadavg 2>/dev/null; printf '\\n__CPUINFO__\\n'; cat /proc/cpuinfo 2>/dev/null; printf '\\n__NPROC_ALL__\\n'; nproc --all 2>/dev/null || true; printf '\\n__NPROC_ONLINE__\\n'; nproc 2>/dev/null || true; printf '\\n__LSCPU__\\n'; lscpu 2>/dev/null || true";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemMonitorSettings {
    pub telemetry_interval_secs: u64,
    pub display_mode: String,
    pub disk_ignore_fs_types: Vec<String>,
}

impl Default for SystemMonitorSettings {
    fn default() -> Self {
        Self {
            telemetry_interval_secs: DEFAULT_INTERVAL_SECS,
            display_mode: "gpu-system".to_string(),
            disk_ignore_fs_types: DEFAULT_IGNORED_FS_TYPES
                .iter()
                .map(|item| item.to_string())
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CpuMetric {
    pub(crate) model_name: Option<String>,
    pub(crate) usage_percent: Option<f64>,
    pub(crate) load_avg1: Option<f64>,
    pub(crate) load_avg5: Option<f64>,
    pub(crate) load_avg15: Option<f64>,
    pub(crate) total_cores: Option<u64>,
    pub(crate) online_cores: Option<u64>,
    pub(crate) avg_clock_ghz: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryMetric {
    pub(crate) total_mi_b: Option<u64>,
    pub(crate) used_mi_b: Option<u64>,
    pub(crate) available_mi_b: Option<u64>,
    pub(crate) free_mi_b: Option<u64>,
    pub(crate) usage_percent: Option<f64>,
    pub(crate) swap_total_mi_b: Option<u64>,
    pub(crate) swap_used_mi_b: Option<u64>,
    pub(crate) swap_free_mi_b: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskMetric {
    pub(crate) filesystem: String,
    pub(crate) fs_type: Option<String>,
    pub(crate) mount_point: String,
    pub(crate) total_bytes: Option<u64>,
    pub(crate) used_bytes: Option<u64>,
    pub(crate) available_bytes: Option<u64>,
    pub(crate) usage_percent: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteUserSession {
    pub(crate) user: String,
    pub(crate) tty: String,
    pub(crate) login_time: String,
    pub(crate) from: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TelemetryErrors {
    #[serde(skip_serializing_if = "Option::is_none")]
    cpu: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    memory: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    disk: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gpu: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    users: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agents: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteTelemetry {
    session_id: String,
    timestamp: String,
    hostname: Option<String>,
    cpu: Option<CpuMetric>,
    memory: Option<MemoryMetric>,
    disks: Vec<DiskMetric>,
    gpu: Vec<GpuMetric>,
    users: Vec<RemoteUserSession>,
    agents: Vec<AgentMetric>,
    errors: TelemetryErrors,
}

/// One CPU time sample in a monotonically increasing unit. Linux fills it from
/// /proc/stat jiffies; Windows from raw perf counters (idle 100 ns ticks vs.
/// the 100 ns wall clock). Usage is the two-poll delta either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuStatSample {
    pub(crate) idle: u64,
    pub(crate) total: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoteOs {
    Linux,
    MacOs,
    Windows,
}

pub(crate) fn local_os() -> RemoteOs {
    if cfg!(target_os = "windows") {
        RemoteOs::Windows
    } else if cfg!(target_os = "macos") {
        RemoteOs::MacOs
    } else {
        RemoteOs::Linux
    }
}

/// Two-stage probe run without the POSIX wrapper, since the remote shell
/// dialect is unknown until the OS is known.
pub(crate) fn detect_remote_os(session: &Session) -> Option<RemoteOs> {
    // Stage 1: uname answers for Linux, macOS, and MSYS/Cygwin environments.
    let (output, status) = run_raw_remote_command(session, "uname -s").ok()?;
    let name = output.trim();
    let uname_answered = status == 0 && !name.is_empty();
    if uname_answered {
        if let Some(os) = classify_uname(name) {
            return Some(os);
        }
    }
    // Stage 2: a Windows host whose default shell (cmd.exe or PowerShell) has
    // no uname — or whose uname port printed something stage 1 didn't
    // recognize. `cmd.exe /c ver` needs no quoting in either shell, and the
    // "Microsoft Windows" brand token survives localization.
    let (output, status) = run_raw_remote_command(session, "cmd.exe /c ver").ok()?;
    if status == 0 && output.contains("Windows") {
        return Some(RemoteOs::Windows);
    }
    // uname answered with an unknown Unix flavour and ver ruled out Windows.
    uname_answered.then_some(RemoteOs::Linux)
}

/// MSYS/Cygwin ports report `MINGW64_NT-…`/`CYGWIN_NT-…`; the physical host is
/// Windows and PowerShell yields correct disks/users/GPU data where the POSIX
/// emulation layer would not. Returns None for names it cannot place — the
/// caller then falls back to the `ver` probe rather than assuming Linux,
/// because standalone Windows uname ports (Gow prints "windows32", others
/// "Windows_NT") would otherwise route a Windows host to the POSIX commands.
fn classify_uname(name: &str) -> Option<RemoteOs> {
    if name == "Darwin" {
        return Some(RemoteOs::MacOs);
    }
    if name.starts_with("MINGW")
        || name.starts_with("MSYS")
        || name.starts_with("CYGWIN")
        || name.to_lowercase().contains("windows")
    {
        return Some(RemoteOs::Windows);
    }
    matches!(
        name,
        "Linux" | "GNU/Linux" | "FreeBSD" | "OpenBSD" | "NetBSD" | "DragonFly" | "SunOS" | "AIX"
    )
    .then_some(RemoteOs::Linux)
}

#[tauri::command]
pub fn get_telemetry_settings(state: State<AppState>) -> Result<SystemMonitorSettings, String> {
    state
        .telemetry_settings
        .lock()
        .map(|settings| settings.clone())
        .map_err(|_| "Telemetry settings are unavailable".to_string())
}

#[tauri::command]
pub fn update_telemetry_settings(
    state: State<AppState>,
    settings: SystemMonitorSettings,
) -> Result<SystemMonitorSettings, String> {
    let settings = sanitize_settings(settings);
    let mut stored = state
        .telemetry_settings
        .lock()
        .map_err(|_| "Telemetry settings are unavailable".to_string())?;
    *stored = settings.clone();
    Ok(settings)
}

const RECONNECT_BACKOFF_INITIAL: Duration = Duration::from_secs(2);
const RECONNECT_BACKOFF_MAX: Duration = Duration::from_secs(60);
/// Consecutive polls where every section fails before the session is
/// considered dead and reopened.
const MAX_TOTAL_FAILURES: u32 = 2;

pub fn start(
    app: AppHandle,
    target: SshTarget,
    stop: Arc<AtomicBool>,
    settings: Arc<Mutex<SystemMonitorSettings>>,
    quota_refreshes: AgentQuotaRefreshes,
    quota_histories: AgentQuotaHistories,
) {
    thread::spawn(move || {
        let mut backoff = RECONNECT_BACKOFF_INITIAL;
        let history_key = format!(
            "ssh:{}@{}:{}",
            target.username,
            target.host.to_ascii_lowercase(),
            target.port
        );
        let mut agent_state = AgentMonitorState::default();
        agent_state.configure_agy_history(history_key, quota_histories);
        while !stop.load(Ordering::SeqCst) {
            // `connection` is bound per reconnect iteration, so its jump-host
            // tunnel (if any) is torn down before the next attempt.
            let connection = match open_ssh_session(&target) {
                Ok(connection) => connection,
                Err(error) => {
                    emit_connection_error_telemetry(&app, &target.session_id, &error);
                    sleep_with_stop_duration(backoff, &stop);
                    backoff = (backoff * 2).min(RECONNECT_BACKOFF_MAX);
                    continue;
                }
            };
            backoff = RECONNECT_BACKOFF_INITIAL;
            let session = connection.session();
            session.set_timeout(COMMAND_TIMEOUT_MS);

            let mut previous_cpu = None;
            let mut gpu_probe = None;
            let mut host_os: Option<RemoteOs> = None;
            let mut consecutive_total_failures = 0_u32;
            while !stop.load(Ordering::SeqCst) {
                let settings_snapshot = settings
                    .lock()
                    .map(|settings| settings.clone())
                    .unwrap_or_default();
                if host_os.is_none() {
                    // Not cached on failure so a dead transport keeps looking
                    // like total failure to the reconnect heuristic.
                    host_os = detect_remote_os(session);
                    if host_os == Some(RemoteOs::MacOs) && gpu_probe.is_none() {
                        gpu_probe = Some(GpuProbe {
                            apple: true,
                            ..GpuProbe::default()
                        });
                    }
                    if host_os == Some(RemoteOs::Windows) {
                        session.set_timeout(WINDOWS_COMMAND_TIMEOUT_MS);
                    }
                }
                apply_agent_quota_refreshes(&target.session_id, &quota_refreshes, &mut agent_state);
                let telemetry = collect_remote_telemetry(
                    &target,
                    session,
                    host_os.unwrap_or(RemoteOs::Linux),
                    &mut previous_cpu,
                    &mut gpu_probe,
                    &mut agent_state,
                );
                consecutive_total_failures = if telemetry_all_failed(&telemetry) {
                    consecutive_total_failures + 1
                } else {
                    0
                };
                emit_telemetry(&app, telemetry);
                if consecutive_total_failures >= MAX_TOTAL_FAILURES {
                    // Every section failed repeatedly — the transport is
                    // likely dead. Drop the session and reconnect.
                    break;
                }
                sleep_with_stop(settings_snapshot.telemetry_interval_secs, &stop);
            }
        }
    });
}

/// Starts telemetry for a native local PTY. Unlike remote sessions this path
/// executes collectors directly on the host and never attempts an SSH
/// connection to localhost.
pub fn start_local(
    app: AppHandle,
    session_id: String,
    stop: Arc<AtomicBool>,
    settings: Arc<Mutex<SystemMonitorSettings>>,
    quota_refreshes: AgentQuotaRefreshes,
    quota_histories: AgentQuotaHistories,
) {
    thread::spawn(move || {
        let os = local_os();
        let mut previous_cpu = None;
        let mut agent_state = AgentMonitorState::default();
        agent_state.configure_agy_history(local_agent_history_key(), quota_histories);
        let mut gpu_probe = (os == RemoteOs::MacOs).then(|| GpuProbe {
            apple: true,
            ..GpuProbe::default()
        });

        while !stop.load(Ordering::SeqCst) {
            let settings_snapshot = settings
                .lock()
                .map(|settings| settings.clone())
                .unwrap_or_default();
            apply_agent_quota_refreshes(&session_id, &quota_refreshes, &mut agent_state);
            let telemetry = collect_local_telemetry(
                &session_id,
                os,
                &mut previous_cpu,
                &mut gpu_probe,
                &mut agent_state,
            );
            emit_telemetry(&app, telemetry);
            sleep_with_stop(settings_snapshot.telemetry_interval_secs, &stop);
        }
    });
}

fn local_agent_history_key() -> String {
    let username = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".to_string());
    let hostname = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "localhost".to_string());
    format!(
        "local:{}@{}",
        username.to_ascii_lowercase(),
        hostname.to_ascii_lowercase()
    )
}

fn apply_agent_quota_refreshes(
    session_id: &str,
    requests: &AgentQuotaRefreshes,
    state: &mut AgentMonitorState,
) {
    let providers = requests
        .lock()
        .ok()
        .and_then(|mut requests| requests.remove(session_id));
    if let Some(providers) = providers {
        for provider in providers {
            state.force_quota_refresh(&provider);
        }
    }
}

fn emit_connection_error_telemetry(app: &AppHandle, session_id: &str, error: &str) {
    emit_telemetry(
        app,
        RemoteTelemetry {
            session_id: session_id.to_string(),
            timestamp: timestamp(),
            hostname: None,
            cpu: None,
            memory: None,
            disks: Vec::new(),
            gpu: Vec::new(),
            users: Vec::new(),
            agents: Vec::new(),
            errors: TelemetryErrors {
                cpu: Some(format!("Telemetry SSH connection failed: {}", error)),
                memory: Some("Telemetry SSH connection failed".to_string()),
                disk: Some("Telemetry SSH connection failed".to_string()),
                gpu: Some("Telemetry SSH connection failed".to_string()),
                users: Some("Telemetry SSH connection failed".to_string()),
                agents: Some("Telemetry SSH connection failed".to_string()),
            },
        },
    );
}

fn telemetry_all_failed(telemetry: &RemoteTelemetry) -> bool {
    telemetry.hostname.is_none()
        && telemetry.cpu.is_none()
        && telemetry.memory.is_none()
        && telemetry.disks.is_empty()
        && telemetry.gpu.is_empty()
        && telemetry.users.is_empty()
        && telemetry.agents.is_empty()
        && telemetry.errors.cpu.is_some()
        && telemetry.errors.memory.is_some()
        && telemetry.errors.disk.is_some()
        && telemetry.errors.agents.is_some()
}

fn emit_telemetry(app: &AppHandle, telemetry: RemoteTelemetry) {
    let _ = app.emit("remote-telemetry", telemetry);
}

fn collect_remote_telemetry(
    target: &SshTarget,
    session: &Session,
    os: RemoteOs,
    previous_cpu: &mut Option<CpuStatSample>,
    gpu_probe: &mut Option<GpuProbe>,
    agent_state: &mut AgentMonitorState,
) -> RemoteTelemetry {
    if os == RemoteOs::Windows {
        return collect_windows_telemetry(
            target,
            session,
            previous_cpu,
            gpu_probe,
            agent_state,
        );
    }
    let mut errors = TelemetryErrors::default();

    let hostname = run_remote_command(
        session,
        "hostname 2>/dev/null || uname -n 2>/dev/null || true",
    )
    .ok()
    .map(|hostname| hostname.trim().to_string())
    .filter(|hostname| !hostname.is_empty());

    let cpu_result = match os {
        RemoteOs::Linux => run_remote_command(session, CPU_COMMAND)
            .and_then(|output| parse_cpu_command_output(&output, previous_cpu)),
        RemoteOs::MacOs => run_remote_command(session, macos_monitor::MACOS_CPU_COMMAND)
            .and_then(|output| macos_monitor::parse_macos_cpu_output(&output)),
        RemoteOs::Windows => unreachable!("Windows telemetry is collected above"),
    };
    let cpu = match cpu_result {
        Ok(metric) => Some(metric),
        Err(error) => {
            errors.cpu = Some(error);
            None
        }
    };

    let memory_result = match os {
        RemoteOs::Linux => run_remote_command(session, "cat /proc/meminfo 2>/dev/null")
            .and_then(|output| parse_meminfo(&output)),
        RemoteOs::MacOs => run_remote_command(session, macos_monitor::MACOS_MEMORY_COMMAND)
            .and_then(|output| macos_monitor::parse_macos_memory_output(&output)),
        RemoteOs::Windows => unreachable!("Windows telemetry is collected above"),
    };
    let memory = match memory_result {
        Ok(metric) => Some(metric),
        Err(error) => {
            errors.memory = Some(error);
            None
        }
    };

    let disks_result = match os {
        RemoteOs::Linux => run_remote_command(session, "df -P -T -B1 2>/dev/null")
            .and_then(|output| parse_df_output(&output)),
        RemoteOs::MacOs => run_remote_command(session, macos_monitor::MACOS_DISK_COMMAND)
            .and_then(|output| macos_monitor::parse_macos_disk_output(&output)),
        RemoteOs::Windows => unreachable!("Windows telemetry is collected above"),
    };
    let disks = match disks_result {
        // The frontend filters by the user's disk_ignore_fs_types setting so the
        // "show hidden filesystems" toggle can reveal them; the backend only sorts.
        Ok(disks) => sort_disks(disks),
        Err(error) => {
            errors.disk = Some(error);
            Vec::new()
        }
    };

    let gpu = match collect_gpu_metrics(session, os, gpu_probe) {
        Ok(metrics) => metrics,
        Err(error) => {
            errors.gpu = Some(error);
            Vec::new()
        }
    };

    let users = match run_remote_command(session, "LC_ALL=C who 2>/dev/null || true") {
        Ok(output) => parse_who_output(&output),
        Err(error) => {
            errors.users = Some(error);
            Vec::new()
        }
    };
    let agents = match agent_monitor::collect_remote_agents(session, target, os, agent_state) {
        Ok(metrics) => metrics,
        Err(error) => {
            errors.agents = Some(error);
            Vec::new()
        }
    };

    RemoteTelemetry {
        session_id: target.session_id.clone(),
        timestamp: timestamp(),
        hostname,
        cpu,
        memory,
        disks,
        gpu,
        users,
        agents,
        errors,
    }
}

fn collect_local_telemetry(
    session_id: &str,
    os: RemoteOs,
    previous_cpu: &mut Option<CpuStatSample>,
    gpu_probe: &mut Option<GpuProbe>,
    agent_state: &mut AgentMonitorState,
) -> RemoteTelemetry {
    if os == RemoteOs::Windows {
        return collect_local_windows_telemetry(
            session_id,
            previous_cpu,
            gpu_probe,
            agent_state,
        );
    }

    let mut errors = TelemetryErrors::default();
    let hostname =
        run_local_command_for(os, "hostname 2>/dev/null || uname -n 2>/dev/null || true")
            .ok()
            .map(|hostname| hostname.trim().to_string())
            .filter(|hostname| !hostname.is_empty());

    let cpu_result = match os {
        RemoteOs::Linux => run_local_command_for(os, CPU_COMMAND)
            .and_then(|output| parse_cpu_command_output(&output, previous_cpu)),
        RemoteOs::MacOs => run_local_command_for(os, macos_monitor::MACOS_CPU_COMMAND)
            .and_then(|output| macos_monitor::parse_macos_cpu_output(&output)),
        RemoteOs::Windows => unreachable!("Windows telemetry is collected above"),
    };
    let cpu = cpu_result.map_err(|error| errors.cpu = Some(error)).ok();

    let memory_result = match os {
        RemoteOs::Linux => run_local_command_for(os, "cat /proc/meminfo 2>/dev/null")
            .and_then(|output| parse_meminfo(&output)),
        RemoteOs::MacOs => run_local_command_for(os, macos_monitor::MACOS_MEMORY_COMMAND)
            .and_then(|output| macos_monitor::parse_macos_memory_output(&output)),
        RemoteOs::Windows => unreachable!("Windows telemetry is collected above"),
    };
    let memory = memory_result
        .map_err(|error| errors.memory = Some(error))
        .ok();

    let disks_result = match os {
        RemoteOs::Linux => run_local_command_for(os, "df -P -T -B1 2>/dev/null")
            .and_then(|output| parse_df_output(&output)),
        RemoteOs::MacOs => run_local_command_for(os, macos_monitor::MACOS_DISK_COMMAND)
            .and_then(|output| macos_monitor::parse_macos_disk_output(&output)),
        RemoteOs::Windows => unreachable!("Windows telemetry is collected above"),
    };
    let disks = disks_result
        .map(sort_disks)
        .map_err(|error| errors.disk = Some(error))
        .unwrap_or_default();

    let gpu = collect_local_gpu_metrics(os, gpu_probe)
        .map_err(|error| errors.gpu = Some(error))
        .unwrap_or_default();
    let users = run_local_command_for(os, "LC_ALL=C who 2>/dev/null || true")
        .map(|output| parse_who_output(&output))
        .map_err(|error| errors.users = Some(error))
        .unwrap_or_default();
    let agents = agent_monitor::collect_local_agents(os, agent_state)
        .map_err(|error| errors.agents = Some(error))
        .unwrap_or_default();

    RemoteTelemetry {
        session_id: session_id.to_string(),
        timestamp: timestamp(),
        hostname,
        cpu,
        memory,
        disks,
        gpu,
        users,
        agents,
        errors,
    }
}

fn collect_local_windows_telemetry(
    session_id: &str,
    previous_cpu: &mut Option<CpuStatSample>,
    gpu_probe: &mut Option<GpuProbe>,
    agent_state: &mut AgentMonitorState,
) -> RemoteTelemetry {
    let mut errors = TelemetryErrors::default();
    let mut hostname = None;
    let mut cpu = None;
    let mut memory = None;
    let mut disks = Vec::new();
    let mut users = Vec::new();

    match run_local_command_for(
        RemoteOs::Windows,
        windows_monitor::WINDOWS_TELEMETRY_COMMAND,
    ) {
        Ok(output) => {
            let sections = split_sections(&output);
            hostname = sections
                .get("HOSTNAME")
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
            match windows_monitor::parse_windows_cpu_output(&sections, previous_cpu) {
                Ok(metric) => cpu = Some(metric),
                Err(error) => errors.cpu = Some(error),
            }
            match windows_monitor::parse_windows_memory_output(&sections) {
                Ok(metric) => memory = Some(metric),
                Err(error) => errors.memory = Some(error),
            }
            match windows_monitor::parse_windows_disk_output(&sections) {
                Ok(found) => disks = sort_disks(found),
                Err(error) => errors.disk = Some(error),
            }
            users = sections
                .get("USERS")
                .map(|value| windows_monitor::parse_quser_output(value))
                .unwrap_or_default();
        }
        Err(error) => {
            errors.cpu = Some(error.clone());
            errors.memory = Some(error.clone());
            errors.disk = Some(error.clone());
            errors.users = Some(error);
        }
    }

    let gpu = collect_local_gpu_metrics(RemoteOs::Windows, gpu_probe)
        .map_err(|error| errors.gpu = Some(error))
        .unwrap_or_default();
    let agents = agent_monitor::collect_local_agents(RemoteOs::Windows, agent_state)
        .map_err(|error| errors.agents = Some(error))
        .unwrap_or_default();
    RemoteTelemetry {
        session_id: session_id.to_string(),
        timestamp: timestamp(),
        hostname,
        cpu,
        memory,
        disks,
        gpu,
        users,
        agents,
        errors,
    }
}

/// Detects available GPU tools once per connection (cached in `probe`), then
/// collects metrics from every detected vendor and concatenates them.
pub(crate) fn collect_gpu_metrics(
    session: &Session,
    os: RemoteOs,
    probe: &mut Option<GpuProbe>,
) -> Result<Vec<GpuMetric>, String> {
    if probe.is_none() {
        let detected = if os == RemoteOs::Windows {
            run_remote_command_for(session, os, windows_monitor::WINDOWS_GPU_PROBE_COMMAND)
                .map(|output| windows_monitor::parse_windows_gpu_probe(&output))
                .unwrap_or_default()
        } else {
            let mut detected = run_remote_command(session, GPU_PROBE_COMMAND)
                .map(|output| parse_gpu_probe(&output))
                .unwrap_or_default();
            if detected.xpu_smi {
                detected.xpu_devices = run_remote_command(session, XPU_DISCOVERY_COMMAND)
                    .map(|output| parse_xpu_discovery(&output))
                    .unwrap_or_default();
            }
            detected
        };
        *probe = Some(detected);
    }
    let probe = probe.as_ref().expect("probe is populated above");

    if !probe.any() {
        return Err(
            "No GPU monitoring source found (nvidia-smi / rocm-smi / xpu-smi / intel_gpu_top / macOS ioreg / Windows GPU counters)"
                .to_string(),
        );
    }

    if os == RemoteOs::Windows {
        return collect_windows_gpu_metrics(session, probe);
    }

    let mut metrics = Vec::new();
    let mut errors = Vec::new();

    if probe.apple {
        match run_remote_command(session, macos_monitor::MACOS_GPU_COMMAND)
            .and_then(|output| macos_monitor::parse_macos_gpu_output(&output))
        {
            Ok(mut found) => metrics.append(&mut found),
            Err(error) => errors.push(error),
        }
    }

    if probe.nvidia {
        match run_remote_command(session, NVIDIA_SMI_QUERY)
            .and_then(|output| parse_nvidia_smi_csv(&output))
        {
            Ok(mut found) => metrics.append(&mut found),
            Err(error) => errors.push(error),
        }
    }
    // rocm-smi is the stable AMD target; amd-smi's JSON schema is still in flux.
    if probe.rocm_smi {
        let command = "rocm-smi --showproductname --showuniqueid --showuse --showmemuse --showmeminfo vram --showtemp --showpower --json 2>/dev/null";
        match run_remote_command(session, command).and_then(|output| parse_rocm_smi_json(&output)) {
            Ok(mut found) => metrics.append(&mut found),
            Err(error) => errors.push(error),
        }
    }
    if probe.xpu_smi && !probe.xpu_devices.is_empty() {
        let command = xpu_stats_command(&probe.xpu_devices);
        match run_remote_command(session, &command) {
            Ok(output) => {
                let sections = split_sections(&output);
                for device in &probe.xpu_devices {
                    if let Some(stats) = sections
                        .get(&format!("XPU_{}", device.id))
                        .and_then(|section| parse_xpu_stats(section, device))
                    {
                        metrics.push(stats);
                    }
                }
            }
            Err(error) => errors.push(error),
        }
    }
    if probe.intel_gpu_top {
        let next_index = metrics.iter().map(|m| m.index + 1).max().unwrap_or(0);
        match run_remote_command(session, INTEL_GPU_TOP_COMMAND)
            .and_then(|output| parse_intel_gpu_top_stream(&output, next_index))
        {
            Ok(igpu) => metrics.push(igpu),
            Err(error) => errors.push(error),
        }
    }
    if os == RemoteOs::Linux && probe.linux_drm {
        let next_index = metrics.iter().map(|m| m.index + 1).max().unwrap_or(0);
        match run_remote_command(session, LINUX_DRM_GPU_COMMAND) {
            Ok(output) => {
                let found = parse_linux_drm_gpus(&output, next_index);
                append_uncovered_linux_drm(&mut metrics, found);
            }
            Err(error) => errors.push(error),
        }
    }

    if metrics.is_empty() && !errors.is_empty() {
        return Err(errors.join("; "));
    }
    Ok(metrics)
}

pub(crate) fn collect_local_gpu_metrics(
    os: RemoteOs,
    probe: &mut Option<GpuProbe>,
) -> Result<Vec<GpuMetric>, String> {
    if probe.is_none() {
        let detected = if os == RemoteOs::Windows {
            run_local_command_for(os, windows_monitor::WINDOWS_GPU_PROBE_COMMAND)
                .map(|output| windows_monitor::parse_windows_gpu_probe(&output))
                .unwrap_or_default()
        } else {
            let mut detected = run_local_command_for(os, GPU_PROBE_COMMAND)
                .map(|output| parse_gpu_probe(&output))
                .unwrap_or_default();
            if detected.xpu_smi {
                detected.xpu_devices = run_local_command_for(os, XPU_DISCOVERY_COMMAND)
                    .map(|output| parse_xpu_discovery(&output))
                    .unwrap_or_default();
            }
            detected
        };
        *probe = Some(detected);
    }
    let probe = probe.as_ref().expect("probe is populated above");
    if !probe.any() {
        return Err(
            "No GPU monitoring source found (nvidia-smi / rocm-smi / xpu-smi / intel_gpu_top / macOS ioreg / Windows GPU counters)"
                .to_string(),
        );
    }

    let mut metrics = Vec::new();
    let mut errors = Vec::new();
    if probe.apple {
        match run_local_command_for(os, macos_monitor::MACOS_GPU_COMMAND)
            .and_then(|output| macos_monitor::parse_macos_gpu_output(&output))
        {
            Ok(mut found) => metrics.append(&mut found),
            Err(error) => errors.push(error),
        }
    }
    if probe.nvidia {
        match run_local_command_for(os, NVIDIA_SMI_QUERY)
            .and_then(|output| parse_nvidia_smi_csv(&output))
        {
            Ok(mut found) => metrics.append(&mut found),
            Err(error) => errors.push(error),
        }
    }
    if probe.rocm_smi {
        let command = "rocm-smi --showproductname --showuniqueid --showuse --showmemuse --showmeminfo vram --showtemp --showpower --json 2>/dev/null";
        match run_local_command_for(os, command).and_then(|output| parse_rocm_smi_json(&output)) {
            Ok(mut found) => metrics.append(&mut found),
            Err(error) => errors.push(error),
        }
    }
    if probe.xpu_smi && !probe.xpu_devices.is_empty() {
        match run_local_command_for(os, &xpu_stats_command(&probe.xpu_devices)) {
            Ok(output) => {
                let sections = split_sections(&output);
                for device in &probe.xpu_devices {
                    if let Some(stats) = sections
                        .get(&format!("XPU_{}", device.id))
                        .and_then(|section| parse_xpu_stats(section, device))
                    {
                        metrics.push(stats);
                    }
                }
            }
            Err(error) => errors.push(error),
        }
    }
    if probe.intel_gpu_top {
        let next_index = metrics
            .iter()
            .map(|metric| metric.index + 1)
            .max()
            .unwrap_or(0);
        match run_local_command_for(os, INTEL_GPU_TOP_COMMAND)
            .and_then(|output| parse_intel_gpu_top_stream(&output, next_index))
        {
            Ok(metric) => metrics.push(metric),
            Err(error) => errors.push(error),
        }
    }
    if os == RemoteOs::Linux && probe.linux_drm {
        let next_index = metrics
            .iter()
            .map(|metric| metric.index + 1)
            .max()
            .unwrap_or(0);
        match run_local_command_for(os, LINUX_DRM_GPU_COMMAND) {
            Ok(output) => {
                let found = parse_linux_drm_gpus(&output, next_index);
                append_uncovered_linux_drm(&mut metrics, found);
            }
            Err(error) => errors.push(error),
        }
    }
    if os == RemoteOs::Windows {
        let needs_counters = probe
            .windows_adapters
            .iter()
            .any(|adapter| !(probe.nvidia && adapter.vendor == "nvidia"));
        if needs_counters {
            let next_index = metrics
                .iter()
                .map(|metric| metric.index + 1)
                .max()
                .unwrap_or(0);
            match run_local_command_for(os, windows_monitor::WINDOWS_GPU_COUNTERS_COMMAND).and_then(
                |output| {
                    windows_monitor::parse_windows_gpu_counters(
                        &output,
                        &probe.windows_adapters,
                        probe.nvidia,
                        next_index,
                    )
                },
            ) {
                Ok(mut found) => metrics.append(&mut found),
                Err(error) => errors.push(error),
            }
        }
    }

    if metrics.is_empty() && !errors.is_empty() {
        return Err(errors.join("; "));
    }
    Ok(metrics)
}

/// Windows GPU collection: nvidia-smi works identically to Linux (full
/// fields); adapters it does not cover fall back to the WDDM performance
/// counters, which expose utilization and dedicated VRAM only.
fn collect_windows_gpu_metrics(
    session: &Session,
    probe: &GpuProbe,
) -> Result<Vec<GpuMetric>, String> {
    let mut metrics = Vec::new();
    let mut errors = Vec::new();

    if probe.nvidia {
        match run_remote_command_for(session, RemoteOs::Windows, NVIDIA_SMI_QUERY)
            .and_then(|output| parse_nvidia_smi_csv(&output))
        {
            Ok(mut found) => metrics.append(&mut found),
            Err(error) => errors.push(error),
        }
    }

    let needs_counters = probe
        .windows_adapters
        .iter()
        .any(|adapter| !(probe.nvidia && adapter.vendor == "nvidia"));
    if needs_counters {
        let next_index = metrics
            .iter()
            .map(|metric| metric.index + 1)
            .max()
            .unwrap_or(0);
        match run_remote_command_for(
            session,
            RemoteOs::Windows,
            windows_monitor::WINDOWS_GPU_COUNTERS_COMMAND,
        )
        .and_then(|output| {
            windows_monitor::parse_windows_gpu_counters(
                &output,
                &probe.windows_adapters,
                probe.nvidia,
                next_index,
            )
        }) {
            Ok(mut found) => metrics.append(&mut found),
            Err(error) => errors.push(error),
        }
    }

    if metrics.is_empty() && !errors.is_empty() {
        return Err(errors.join("; "));
    }
    Ok(metrics)
}

/// Windows telemetry: one batched PowerShell invocation covers hostname, CPU,
/// memory, disks, and users (PowerShell start-up is too slow for one command
/// per section); GPUs keep their own commands like the Unix path.
fn collect_windows_telemetry(
    target: &SshTarget,
    session: &Session,
    previous_cpu: &mut Option<CpuStatSample>,
    gpu_probe: &mut Option<GpuProbe>,
    agent_state: &mut AgentMonitorState,
) -> RemoteTelemetry {
    let mut errors = TelemetryErrors::default();
    let mut hostname = None;
    let mut cpu = None;
    let mut memory = None;
    let mut disks = Vec::new();
    let mut users = Vec::new();

    match run_remote_command_for(
        session,
        RemoteOs::Windows,
        windows_monitor::WINDOWS_TELEMETRY_COMMAND,
    ) {
        Ok(output) => {
            let sections = split_sections(&output);
            hostname = sections
                .get("HOSTNAME")
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
            match windows_monitor::parse_windows_cpu_output(&sections, previous_cpu) {
                Ok(metric) => cpu = Some(metric),
                Err(error) => errors.cpu = Some(error),
            }
            match windows_monitor::parse_windows_memory_output(&sections) {
                Ok(metric) => memory = Some(metric),
                Err(error) => errors.memory = Some(error),
            }
            match windows_monitor::parse_windows_disk_output(&sections) {
                Ok(found) => disks = sort_disks(found),
                Err(error) => errors.disk = Some(error),
            }
            users = sections
                .get("USERS")
                .map(|value| windows_monitor::parse_quser_output(value))
                .unwrap_or_default();
        }
        Err(error) => {
            // The one batched command failing fails every section, so a dead
            // transport looks like total failure to the reconnect heuristic
            // exactly as it does on the Unix paths.
            errors.cpu = Some(error.clone());
            errors.memory = Some(error.clone());
            errors.disk = Some(error.clone());
            errors.users = Some(error);
        }
    }

    let gpu = match collect_gpu_metrics(session, RemoteOs::Windows, gpu_probe) {
        Ok(metrics) => metrics,
        Err(error) => {
            errors.gpu = Some(error);
            Vec::new()
        }
    };
    let agents =
        match agent_monitor::collect_remote_agents(session, target, RemoteOs::Windows, agent_state)
        {
            Ok(metrics) => metrics,
            Err(error) => {
                errors.agents = Some(error);
                Vec::new()
            }
        };

    RemoteTelemetry {
        session_id: target.session_id.clone(),
        timestamp: timestamp(),
        hostname,
        cpu,
        memory,
        disks,
        gpu,
        users,
        agents,
        errors,
    }
}

/// Parses `LC_ALL=C who` output. The login time column set varies between
/// GNU ("2026-07-15 09:12") and BSD ("Jul 15 09:12"), so it is kept as an
/// opaque string; the trailing "(host)" field becomes `from` when present.
pub fn parse_who_output(output: &str) -> Vec<RemoteUserSession> {
    output
        .lines()
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 3 {
                return None;
            }
            let from = fields
                .last()
                .filter(|value| value.starts_with('(') && value.ends_with(')'))
                .map(|value| value[1..value.len() - 1].to_string());
            let time_end = if from.is_some() {
                fields.len() - 1
            } else {
                fields.len()
            };
            Some(RemoteUserSession {
                user: fields[0].to_string(),
                tty: fields[1].to_string(),
                login_time: fields[2..time_end].join(" "),
                from,
            })
        })
        .collect()
}

pub(crate) fn run_remote_command(session: &Session, command: &str) -> Result<String, String> {
    run_remote_command_with_budget(session, command, COMMAND_TIMEOUT_SECS)
}

/// Runs one POSIX collector with an explicit time budget.
///
/// The remote `timeout` wrapper and libssh2's own read timeout are raised
/// together: if the session timeout were left at the default it would fire
/// first, abandoning the channel mid-read and reporting a transport error
/// instead of the intended timeout. The session timeout is always restored, so
/// later calls keep the short default.
pub(crate) fn run_remote_command_with_budget(
    session: &Session,
    command: &str,
    budget_secs: u64,
) -> Result<String, String> {
    // `timeout` is missing on macOS <= 12, so fall back to a bare shell there;
    // libssh2's session timeout still bounds the fallback branch.
    let quoted = shell_quote(command);
    let wrapped = format!(
        "command -v timeout >/dev/null 2>&1 && exec timeout {secs}s sh -lc {quoted}; exec sh -lc {quoted}",
        secs = budget_secs,
        quoted = quoted
    );

    let previous_timeout = session.timeout();
    let session_timeout = budget_secs
        .saturating_mul(1000)
        .saturating_add(SESSION_TIMEOUT_SLACK_MS)
        .min(u32::MAX as u64) as u32;
    if session_timeout != previous_timeout {
        session.set_timeout(session_timeout);
    }
    let result = exec_remote(session, &wrapped);
    if session_timeout != previous_timeout {
        session.set_timeout(previous_timeout);
    }
    let (stdout, stderr, exit_status) = result?;

    if exit_status == 124 {
        return Err(format!(
            "telemetry command timed out after {}s",
            budget_secs
        ));
    }

    if exit_status != 0 {
        return Err(non_zero_exit_error(&stderr, exit_status));
    }

    Ok(stdout)
}

/// Dispatches by remote OS: the POSIX wrapper for Linux/macOS, PowerShell for
/// Windows.
pub(crate) fn run_remote_command_for(
    session: &Session,
    os: RemoteOs,
    command: &str,
) -> Result<String, String> {
    match os {
        RemoteOs::Linux | RemoteOs::MacOs => run_remote_command(session, command),
        RemoteOs::Windows => run_windows_remote_command(session, command),
    }
}

/// Executes one telemetry collector directly on the machine running GpuTerm.
/// Only stdout crosses into the parser and no shell output is exposed to the
/// terminal session itself.
pub(crate) fn run_local_command_for(os: RemoteOs, command: &str) -> Result<String, String> {
    run_local_command_with_timeout(os, command, LOCAL_COMMAND_TIMEOUT)
}

/// Runs a local collector with an upper bound on how long it may take.
///
/// `Command::output` waits forever, so one wedged collector (a hung mount, a
/// stuck PowerShell) would stall the telemetry thread for the life of the
/// session. The child is killed once the budget is gone.
pub(crate) fn run_local_command_with_timeout(
    os: RemoteOs,
    command: &str,
    timeout: Duration,
) -> Result<String, String> {
    let mut child = local_collector_command(os, command)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to start local telemetry command: {}", error))?;

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "local telemetry command timed out after {}s",
                        timeout.as_secs()
                    ));
                }
                thread::sleep(LOCAL_COMMAND_POLL_INTERVAL);
            }
            Err(error) => {
                let _ = child.kill();
                return Err(format!("local telemetry command failed: {}", error));
            }
        }
    }

    let output = child
        .wait_with_output()
        .map_err(|error| format!("local telemetry command failed: {}", error))?;
    if !output.status.success() {
        return Err(non_zero_exit_error(
            &String::from_utf8_lossy(&output.stderr),
            output.status.code().unwrap_or(-1),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn local_collector_command(os: RemoteOs, command: &str) -> Command {
    let process = match os {
        RemoteOs::Windows => {
            let mut process = Command::new(windows_powershell_executable());
            process
                .args([
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-OutputFormat",
                    "Text",
                    "-EncodedCommand",
                ])
                .arg(encode_powershell_script(&windows_local_script(command)));
            process
        }
        RemoteOs::Linux | RemoteOs::MacOs => {
            let mut process = Command::new("sh");
            process.args(["-lc", command]);
            process
        }
    };

    // GpuTerm is a Windows GUI-subsystem application. Without this flag every
    // local telemetry poll allocates a new console for powershell.exe, causing
    // one or more terminal windows to flash on screen every few seconds.
    #[cfg(target_os = "windows")]
    {
        let mut process = process;
        process.creation_flags(CREATE_NO_WINDOW);
        process
    }

    #[cfg(not(target_os = "windows"))]
    {
        process
    }
}

fn windows_powershell_executable() -> OsString {
    #[cfg(target_os = "windows")]
    {
        if let Some(system_root) = std::env::var_os("SystemRoot") {
            let executable = PathBuf::from(system_root)
                .join("System32")
                .join("WindowsPowerShell")
                .join("v1.0")
                .join("powershell.exe");
            if executable.is_file() {
                return executable.into_os_string();
            }
        }
    }
    OsString::from("powershell.exe")
}

fn windows_local_script(script: &str) -> String {
    // PowerShell 5.1 otherwise writes redirected console output using the
    // active OEM code page. JSON containing a localized CPU, volume, user, or
    // process name can then become invalid when Rust decodes it. Force UTF-8
    // before any collector output is produced.
    format!(
        "$utf8 = New-Object System.Text.UTF8Encoding($false)\n\
         [Console]::OutputEncoding = $utf8\n\
         $OutputEncoding = $utf8\n\
         {}",
        script
    )
}

/// Runs a PowerShell script on a Windows remote. `-EncodedCommand` (base64 of
/// UTF-16LE) keeps the wire command inert in both cmd.exe and PowerShell, so
/// it survives whichever default shell the OpenSSH server is configured with.
/// Windows has no `timeout`/`exit 124` equivalent; the libssh2 session
/// timeout bounds the call like the macOS no-`timeout` fallback branch.
/// Joins an SFTP path. SFTP always uses forward slashes, including against
/// Windows OpenSSH.
pub(crate) fn remote_join(base: impl AsRef<Path>, child: &str) -> String {
    let base = base.as_ref().to_string_lossy().replace('\\', "/");
    let base = base.trim_end_matches('/');
    format!("{}/{}", base, child)
}

/// Converts an SFTP path to the form the host's own tools expect. Windows
/// OpenSSH reports `/C:/Users/...`, but a `-File` argument needs `C:/Users/...`.
pub(crate) fn native_remote_path(path: &str) -> String {
    let bytes = path.as_bytes();
    if bytes.len() > 2 && bytes[0] == b'/' && bytes[2] == b':' {
        return path[1..].to_string();
    }
    path.to_string()
}

/// Writes a remote file through a temporary sibling, so a failed transfer cannot
/// leave a truncated or empty file where a working one was.
///
/// The rename needs a fallback because `ssh2::Sftp::rename` can never overwrite:
/// libssh2 pins SFTP to version 3 (`libssh2_sftp.h`, `LIBSSH2_SFTP_VERSION 3`)
/// and only sends the rename flags field at version 5 or later (`sftp.c`), so
/// `RenameFlags::OVERWRITE` is never transmitted to any server and a rename onto
/// an existing path fails with `SSH_FX_FAILURE`. OpenSSH's atomic
/// `posix-rename@openssh.com` extension exists in libssh2 but is not bound by
/// `libssh2-sys`, and `ssh2::Sftp` keeps its raw handle private, so unlinking
/// first is the only route. The complete contents are already on the remote
/// before the destination is touched, which is the property that matters.
pub(crate) fn write_remote_file_atomically(
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

    if sftp
        .rename(Path::new(&temporary), Path::new(path), None)
        .is_ok()
    {
        return Ok(());
    }
    // Absent is fine; this only clears the way for the retry.
    let _ = sftp.unlink(Path::new(path));
    sftp.rename(Path::new(&temporary), Path::new(path), None)
        .map_err(|error| {
            // The temporary file is deliberately left in place: it holds the
            // complete contents, so naming it lets the user recover.
            format!(
                "failed to replace {}: {}. The new file is at {}",
                path, error, temporary
            )
        })
}

/// Ceiling for an `-EncodedCommand` wire command.
///
/// Windows OpenSSH runs exec requests through `cmd.exe` unless an administrator
/// changed `DefaultShell`, and cmd.exe's command line stops at 8,191 characters.
/// Base64 of UTF-16LE inflates a script by roughly 2.7x, so the all-providers
/// agent metadata scrape reached about 18,000 characters and failed outright,
/// while the Claude-only form sat at 98% of the limit. Scripts above this bound
/// are uploaded and run by path instead, which keeps the command near 120
/// characters no matter how large the script grows.
/// cmd.exe's hard command-line ceiling, which bounds anything sent inline.
pub(crate) const WINDOWS_CMD_EXE_LIMIT: usize = 8_191;

/// Inline commands stay well inside the ceiling so that growing a script does
/// not silently walk up to it again.
const WINDOWS_ENCODED_COMMAND_LIMIT: usize = WINDOWS_CMD_EXE_LIMIT * 3 / 4;

fn run_windows_remote_command(session: &Session, script: &str) -> Result<String, String> {
    let wrapped = match windows_remote_invocation(session, script) {
        WindowsInvocation::Encoded(command) => command,
        WindowsInvocation::ScriptFile(path) => format!(
            "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"{}\"",
            path
        ),
    };
    let (stdout, stderr, exit_status) = exec_remote(session, &wrapped)?;
    if exit_status != 0 {
        return Err(non_zero_exit_error(&stderr, exit_status));
    }
    Ok(stdout)
}

/// The full wire command for the inline form.
pub(crate) fn windows_encoded_command(script: &str) -> String {
    format!(
        "powershell.exe -NoProfile -NonInteractive -EncodedCommand {}",
        encode_powershell_script(script)
    )
}

/// Whether a script is too large to send inline and must be uploaded instead.
pub(crate) fn windows_script_needs_upload(script: &str) -> bool {
    windows_encoded_command(script).len() > WINDOWS_ENCODED_COMMAND_LIMIT
}

pub(crate) enum WindowsInvocation {
    Encoded(String),
    ScriptFile(String),
}

/// Chooses how to deliver a PowerShell script, preferring the single-round-trip
/// encoded form and falling back to it whenever the upload is not possible, so
/// this can never do worse than sending the command inline.
fn windows_remote_invocation(session: &Session, script: &str) -> WindowsInvocation {
    let encoded = windows_encoded_command(script);
    if !windows_script_needs_upload(script) {
        return WindowsInvocation::Encoded(encoded);
    }
    match upload_windows_script(session, script) {
        Ok(path) => WindowsInvocation::ScriptFile(path),
        Err(_) => WindowsInvocation::Encoded(encoded),
    }
}

/// Uploads the script under a content-addressed name and returns the path to run.
///
/// The name is derived from the contents, so an unchanged script is uploaded once
/// and later polls only pay for one `stat`.
fn upload_windows_script(session: &Session, script: &str) -> Result<String, String> {
    let sftp = session
        .sftp()
        .map_err(|error| format!("SFTP unavailable for script upload: {}", error))?;
    let home = sftp
        .realpath(Path::new("."))
        .map_err(|error| format!("failed to resolve the remote home directory: {}", error))?;
    let directory = remote_join(remote_join(&home, ".gputerm"), "scripts");

    let mut hasher = DefaultHasher::new();
    script.hash(&mut hasher);
    let path = remote_join(&directory, &format!("gputerm-{:016x}.ps1", hasher.finish()));

    // A UTF-8 BOM makes `-File` read the script unambiguously, keeping non-ASCII
    // content intact.
    let mut bytes = Vec::with_capacity(script.len() + 3);
    bytes.extend_from_slice(&[0xef, 0xbb, 0xbf]);
    bytes.extend_from_slice(script.as_bytes());

    // The name is content-addressed, so a matching size means this exact script
    // is already there. Repeat polls then cost one realpath and one stat.
    let already_uploaded = sftp
        .stat(Path::new(&path))
        .map(|stat| stat.size == Some(bytes.len() as u64))
        .unwrap_or(false);
    if !already_uploaded {
        for level in [remote_join(&home, ".gputerm"), directory.clone()] {
            if sftp.stat(Path::new(&level)).is_err() {
                sftp.mkdir(Path::new(&level), 0o700)
                    .map_err(|error| format!("failed to create {}: {}", level, error))?;
            }
        }
        write_remote_file_atomically(&sftp, &path, &bytes)?;
        prune_windows_scripts(&sftp, &directory);
    }
    Ok(native_remote_path(&path))
}

/// Drops uploaded scripts that no current GpuTerm version asks for any more.
fn prune_windows_scripts(sftp: &ssh2::Sftp, directory: &str) {
    let Ok(entries) = sftp.readdir(Path::new(directory)) else {
        return;
    };
    let cutoff = now_epoch_seconds().saturating_sub(7 * 24 * 60 * 60);
    for (path, stat) in entries {
        if stat.mtime.is_some_and(|mtime| mtime < cutoff) {
            let _ = sftp.unlink(&path);
        }
    }
}

fn now_epoch_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

fn encode_powershell_script(script: &str) -> String {
    let utf16: Vec<u8> = script
        .encode_utf16()
        .flat_map(|unit| unit.to_le_bytes())
        .collect();
    base64::engine::general_purpose::STANDARD.encode(utf16)
}

/// Raw exec used only for OS detection, where the remote shell dialect is
/// still unknown; a non-zero exit is data, not an error.
fn run_raw_remote_command(session: &Session, command: &str) -> Result<(String, i32), String> {
    exec_remote(session, command).map(|(stdout, _, exit_status)| (stdout, exit_status))
}

/// Execs a wire-ready command and returns (stdout, stderr, exit status).
/// Output is decoded lossily: Windows hosts in particular may emit non-UTF-8
/// bytes for localized names, which must not fail the whole poll.
fn exec_remote(session: &Session, wire_command: &str) -> Result<(String, String, i32), String> {
    let mut channel = session
        .channel_session()
        .map_err(|error| format!("failed to open telemetry command channel: {}", error))?;
    channel
        .exec(wire_command)
        .map_err(|error| format!("failed to execute telemetry command: {}", error))?;

    let mut stdout = Vec::new();
    channel
        .read_to_end(&mut stdout)
        .map_err(|error| format!("failed to read telemetry command output: {}", error))?;

    let mut stderr = Vec::new();
    let _ = channel.stderr().read_to_end(&mut stderr);
    let _ = channel.wait_close();
    let exit_status = channel.exit_status().unwrap_or(-1);
    Ok((
        String::from_utf8_lossy(&stdout).into_owned(),
        String::from_utf8_lossy(&stderr).into_owned(),
        exit_status,
    ))
}

fn non_zero_exit_error(stderr: &str, exit_status: i32) -> String {
    if stderr.trim().is_empty() {
        format!("telemetry command exited with status {}", exit_status)
    } else {
        stderr.trim().to_string()
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn parse_cpu_command_output(
    output: &str,
    previous_cpu: &mut Option<CpuStatSample>,
) -> Result<CpuMetric, String> {
    let sections = split_sections(output);
    let proc_stat = required_section(&sections, "PROC_STAT")?;
    let current_cpu = parse_proc_stat_cpu_sample(proc_stat)?;
    let usage_percent =
        previous_cpu.and_then(|previous| calculate_cpu_usage(previous, current_cpu));
    *previous_cpu = Some(current_cpu);

    let load = sections
        .get("LOADAVG")
        .map(|content| parse_loadavg(content))
        .unwrap_or((None, None, None));
    let cpuinfo = sections.get("CPUINFO").map(String::as_str).unwrap_or("");
    let lscpu = sections.get("LSCPU").map(String::as_str).unwrap_or("");

    let total_cores = sections
        .get("NPROC_ALL")
        .and_then(|content| parse_first_u64(content))
        .or_else(|| parse_lscpu_cpu_count(lscpu))
        .or_else(|| parse_cpuinfo_processor_count(cpuinfo));
    let online_cores = sections
        .get("NPROC_ONLINE")
        .and_then(|content| parse_first_u64(content))
        .or_else(|| parse_lscpu_online_count(lscpu))
        .or(total_cores);

    Ok(CpuMetric {
        model_name: parse_cpu_model(cpuinfo).or_else(|| parse_lscpu_value(lscpu, "Model name")),
        usage_percent,
        load_avg1: load.0,
        load_avg5: load.1,
        load_avg15: load.2,
        total_cores,
        online_cores,
        avg_clock_ghz: parse_average_clock(cpuinfo)
            .or_else(|| parse_lscpu_cpu_mhz(lscpu).map(|mhz| mhz / 1000.0)),
    })
}

pub fn parse_proc_stat_cpu_sample(output: &str) -> Result<CpuStatSample, String> {
    let line = output
        .lines()
        .find(|line| line.starts_with("cpu "))
        .ok_or_else(|| "missing aggregate cpu row in /proc/stat".to_string())?;
    let values = line
        .split_whitespace()
        .skip(1)
        .map(|value| value.parse::<u64>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "invalid numeric value in /proc/stat".to_string())?;

    if values.len() < 4 {
        return Err("not enough cpu columns in /proc/stat".to_string());
    }

    let idle = values.get(3).copied().unwrap_or(0) + values.get(4).copied().unwrap_or(0);
    let total = values.iter().sum();
    Ok(CpuStatSample { idle, total })
}

pub fn calculate_cpu_usage(previous: CpuStatSample, current: CpuStatSample) -> Option<f64> {
    let total_delta = current.total.checked_sub(previous.total)?;
    let idle_delta = current.idle.checked_sub(previous.idle)?;
    if total_delta == 0 {
        return None;
    }
    let active_delta = total_delta.saturating_sub(idle_delta);
    Some(((active_delta as f64 / total_delta as f64) * 100.0).clamp(0.0, 100.0))
}

pub fn parse_meminfo(output: &str) -> Result<MemoryMetric, String> {
    let values = parse_meminfo_values(output);

    let total = values
        .get("MemTotal")
        .copied()
        .ok_or_else(|| "missing MemTotal in /proc/meminfo".to_string())?;
    let free = values.get("MemFree").copied().unwrap_or(0);
    let available = values.get("MemAvailable").copied().unwrap_or_else(|| {
        free + values.get("Buffers").copied().unwrap_or(0)
            + values.get("Cached").copied().unwrap_or(0)
    });
    let used = total.saturating_sub(available);
    let swap_total = values.get("SwapTotal").copied().unwrap_or(0);
    let swap_free = values.get("SwapFree").copied().unwrap_or(0);
    let swap_used = swap_total.saturating_sub(swap_free);

    Ok(MemoryMetric {
        total_mi_b: Some(kib_to_mib(total)),
        used_mi_b: Some(kib_to_mib(used)),
        available_mi_b: Some(kib_to_mib(available)),
        free_mi_b: Some(kib_to_mib(free)),
        usage_percent: if total > 0 {
            Some((used as f64 / total as f64) * 100.0)
        } else {
            None
        },
        swap_total_mi_b: Some(kib_to_mib(swap_total)),
        swap_used_mi_b: Some(kib_to_mib(swap_used)),
        swap_free_mi_b: Some(kib_to_mib(swap_free)),
    })
}

pub fn parse_df_output(output: &str) -> Result<Vec<DiskMetric>, String> {
    let mut disks = Vec::new();
    for line in output
        .lines()
        .skip(1)
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 7 {
            continue;
        }
        let usage_percent = fields[5].trim_end_matches('%').parse::<f64>().ok();
        disks.push(DiskMetric {
            filesystem: fields[0].to_string(),
            fs_type: Some(fields[1].to_string()).filter(|value| !value.is_empty()),
            total_bytes: fields[2].parse::<u64>().ok(),
            used_bytes: fields[3].parse::<u64>().ok(),
            available_bytes: fields[4].parse::<u64>().ok(),
            usage_percent,
            mount_point: fields[6..].join(" "),
        });
    }

    if disks.is_empty() {
        return Err("df returned no parseable disk rows".to_string());
    }

    Ok(disks)
}

pub fn sort_disks(mut disks: Vec<DiskMetric>) -> Vec<DiskMetric> {
    disks.sort_by(|a, b| {
        disk_priority(&a.mount_point)
            .cmp(&disk_priority(&b.mount_point))
            .then_with(|| a.mount_point.cmp(&b.mount_point))
    });
    disks
}

fn parse_cpuinfo_processor_count(cpuinfo: &str) -> Option<u64> {
    let count = cpuinfo
        .lines()
        .filter(|line| {
            line.split_once(':')
                .map(|(key, _)| key.trim() == "processor")
                .unwrap_or(false)
        })
        .count();
    (count > 0).then_some(count as u64)
}

fn parse_lscpu_cpu_count(lscpu: &str) -> Option<u64> {
    parse_lscpu_value(lscpu, "CPU(s)").and_then(|value| value.parse::<u64>().ok())
}

fn parse_lscpu_online_count(lscpu: &str) -> Option<u64> {
    parse_lscpu_value(lscpu, "On-line CPU(s) list").and_then(|value| parse_cpu_list_count(&value))
}

fn parse_lscpu_cpu_mhz(lscpu: &str) -> Option<f64> {
    parse_lscpu_value(lscpu, "CPU MHz")
        .or_else(|| parse_lscpu_value(lscpu, "CPU max MHz"))
        .and_then(|value| value.parse::<f64>().ok())
}

fn parse_cpu_list_count(value: &str) -> Option<u64> {
    let mut count = 0_u64;
    for part in value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        if let Some((start, end)) = part.split_once('-') {
            let start = start.trim().parse::<u64>().ok()?;
            let end = end.trim().parse::<u64>().ok()?;
            count += end.checked_sub(start)? + 1;
        } else {
            part.parse::<u64>().ok()?;
            count += 1;
        }
    }
    Some(count)
}

fn disk_priority(mount_point: &str) -> u8 {
    match mount_point {
        "/" => 0,
        path if path == "/home" || path.starts_with("/home/") => 10,
        path if path == "/data" || path.starts_with("/data/") => 20,
        path if path == "/mnt" || path.starts_with("/mnt/") => 30,
        path if path == "/media" || path.starts_with("/media/") => 40,
        // Windows drive letters sort ahead of "other"; the alphabetical
        // tiebreak in sort_disks puts C:\ first among them.
        path if is_windows_drive(path) => 50,
        _ => 100,
    }
}

fn is_windows_drive(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn sanitize_settings(mut settings: SystemMonitorSettings) -> SystemMonitorSettings {
    if !matches!(settings.telemetry_interval_secs, 1 | 2 | 5 | 10) {
        settings.telemetry_interval_secs = DEFAULT_INTERVAL_SECS;
    }
    if !matches!(
        settings.display_mode.as_str(),
        "gpu-only" | "system-only" | "gpu-system"
    ) {
        settings.display_mode = "gpu-system".to_string();
    }
    settings.disk_ignore_fs_types = settings
        .disk_ignore_fs_types
        .into_iter()
        .map(|item| item.trim().to_lowercase())
        .filter(|item| !item.is_empty())
        .collect();
    settings
}

fn sleep_with_stop(interval_secs: u64, stop: &AtomicBool) {
    sleep_with_stop_duration(Duration::from_secs(interval_secs.max(1)), stop);
}

fn sleep_with_stop_duration(duration: Duration, stop: &AtomicBool) {
    let ticks = (duration.as_millis() / 100).max(1);
    for _ in 0..ticks {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    #[test]
    fn windows_scripts_switch_to_upload_only_once_they_outgrow_the_inline_form() {
        // A short script stays inline: one round trip, no remote file.
        assert!(!windows_script_needs_upload("Write-Output 'hi'"));
        // The encoded form inflates by about 2.7x, so this crosses the bound.
        let large = "Write-Output 'x'\n".repeat(400);
        assert!(windows_encoded_command(&large).len() > WINDOWS_ENCODED_COMMAND_LIMIT);
        assert!(windows_script_needs_upload(&large));
    }

    use super::*;

    #[test]
    fn parses_proc_stat_cpu_sample() {
        let sample = parse_proc_stat_cpu_sample(
            "cpu  100 20 30 400 50 0 0 0 0 0\ncpu0 10 0 0 40 5 0 0 0 0 0",
        )
        .unwrap();
        assert_eq!(sample.total, 600);
        assert_eq!(sample.idle, 450);
    }

    #[test]
    fn calculates_cpu_usage_from_deltas() {
        let previous = CpuStatSample {
            idle: 100,
            total: 200,
        };
        let current = CpuStatSample {
            idle: 150,
            total: 400,
        };
        assert_eq!(calculate_cpu_usage(previous, current).unwrap(), 75.0);
    }

    #[test]
    fn parses_meminfo_and_used_is_total_minus_available() {
        let memory = parse_meminfo(
            "MemTotal:       262144000 kB\nMemFree:         1024000 kB\nMemAvailable:   131072000 kB\nSwapTotal:       8388608 kB\nSwapFree:        4194304 kB\n",
        )
        .unwrap();
        assert_eq!(memory.total_mi_b, Some(256000));
        assert_eq!(memory.available_mi_b, Some(128000));
        assert_eq!(memory.used_mi_b, Some(128000));
        assert_eq!(memory.swap_used_mi_b, Some(4096));
    }

    #[test]
    fn parses_df_output() {
        let disks = parse_df_output(
            "Filesystem     Type 1-blocks Used Available Use% Mounted on\n/dev/sda1      ext4 1000 400 600 40% /\nserver:/data   nfs  2000 1000 1000 50% /data\n",
        )
        .unwrap();
        assert_eq!(disks.len(), 2);
        assert_eq!(disks[0].mount_point, "/");
        assert_eq!(disks[1].fs_type.as_deref(), Some("nfs"));
        assert_eq!(disks[1].usage_percent, Some(50.0));
    }

    #[test]
    fn prioritizes_disk_mounts_without_filtering() {
        let disks = parse_df_output(
            "Filesystem Type 1-blocks Used Available Use% Mounted on\ntmpfs tmpfs 10 1 9 10% /run\n/dev/sdb1 xfs 100 20 80 20% /data\n/dev/sda1 ext4 100 30 70 30% /\n",
        )
        .unwrap();
        // Filtering by fs type is a frontend concern; the backend keeps every
        // mount so the "show hidden filesystems" toggle can reveal them.
        let sorted = sort_disks(disks);
        assert_eq!(sorted.len(), 3);
        assert_eq!(sorted[0].mount_point, "/");
        assert_eq!(sorted[1].mount_point, "/data");
        assert_eq!(sorted[2].mount_point, "/run");
    }

    #[test]
    fn classifies_uname_output_per_os() {
        assert_eq!(classify_uname("Darwin"), Some(RemoteOs::MacOs));
        assert_eq!(classify_uname("Linux"), Some(RemoteOs::Linux));
        assert_eq!(classify_uname("FreeBSD"), Some(RemoteOs::Linux));
        // Git-for-Windows / MSYS / Cygwin run on a physical Windows host.
        assert_eq!(
            classify_uname("MINGW64_NT-10.0-19045"),
            Some(RemoteOs::Windows)
        );
        assert_eq!(
            classify_uname("MSYS_NT-10.0-22631"),
            Some(RemoteOs::Windows)
        );
        assert_eq!(
            classify_uname("CYGWIN_NT-10.0-19045"),
            Some(RemoteOs::Windows)
        );
        // Standalone Windows uname ports on the host PATH must not be
        // mistaken for Linux — they broke telemetry by routing a Windows
        // host to the POSIX command set.
        assert_eq!(classify_uname("Windows_NT"), Some(RemoteOs::Windows));
        assert_eq!(classify_uname("windows32"), Some(RemoteOs::Windows));
        assert_eq!(classify_uname("WindowsNT"), Some(RemoteOs::Windows));
        // Unknown flavours defer to the `ver` probe instead of guessing.
        assert_eq!(classify_uname("Haiku"), None);
    }

    #[test]
    fn windows_drives_sort_after_unix_data_mounts() {
        let disks = parse_df_output(
            "Filesystem Type 1-blocks Used Available Use% Mounted on\nD:\\ ntfs 100 20 80 20% D:\\\nC:\\ ntfs 100 30 70 30% C:\\\n",
        )
        .unwrap();
        let sorted = sort_disks(disks);
        assert_eq!(sorted[0].mount_point, "C:\\");
        assert_eq!(sorted[1].mount_point, "D:\\");
        assert_eq!(disk_priority("C:\\"), 50);
        assert!(disk_priority("C:\\") < disk_priority("/run"));
        assert!(disk_priority("/") < disk_priority("C:\\"));
    }

    #[test]
    fn encodes_powershell_script_as_utf16le_base64() {
        // "ab" → UTF-16LE 61 00 62 00 → base64 "YQBiAA==".
        assert_eq!(encode_powershell_script("ab"), "YQBiAA==");
    }

    #[test]
    fn builds_noninteractive_utf8_windows_local_collector() {
        let process = local_collector_command(
            RemoteOs::Windows,
            "Write-Output '__HOSTNAME__'; Write-Output $env:COMPUTERNAME",
        );
        assert_eq!(
            std::path::Path::new(process.get_program())
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("powershell.exe")
        );
        let args = process
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(args
            .windows(2)
            .any(|pair| pair == ["-OutputFormat", "Text"]));
        assert!(args.iter().any(|arg| arg == "-NonInteractive"));
        assert!(args.iter().any(|arg| arg == "-EncodedCommand"));

        let wrapped = windows_local_script("Write-Output 'ok'");
        assert!(wrapped.contains("[Console]::OutputEncoding = $utf8"));
        assert!(wrapped.ends_with("Write-Output 'ok'"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn executes_windows_local_collector_with_utf8_output() {
        let output =
            run_local_command_for(RemoteOs::Windows, "Write-Output '로컬 모니터링'").unwrap();
        assert_eq!(output.trim(), "로컬 모니터링");
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn executes_a_local_collector_without_ssh() {
        let output = run_local_command_for(local_os(), "printf local-telemetry").unwrap();
        assert_eq!(output, "local-telemetry");
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    #[ignore = "requires host telemetry permissions unavailable in some sandboxes"]
    fn collects_local_cpu_memory_and_disk_metrics_without_ssh() {
        let os = local_os();
        let mut previous_cpu = None;
        let mut gpu_probe = None;
        let mut agent_state = AgentMonitorState::default();
        let telemetry = collect_local_telemetry(
            "local-test",
            os,
            &mut previous_cpu,
            &mut gpu_probe,
            &mut agent_state,
        );

        assert!(telemetry.hostname.is_some());
        assert!(telemetry.cpu.is_some(), "{:?}", telemetry.errors.cpu);
        assert!(telemetry.memory.is_some(), "{:?}", telemetry.errors.memory);
        assert!(!telemetry.disks.is_empty(), "{:?}", telemetry.errors.disk);
    }

    #[test]
    fn converts_mib_to_gib_for_display_math() {
        assert_eq!(mib_to_gib_for_test(262_144), 256.0);
    }

    #[test]
    fn parses_full_cpu_command_output() {
        let output = "__PROC_STAT__\ncpu  100 0 0 100 0 0 0 0 0 0\n__LOADAVG__\n2.41 1.92 1.50 1/100 42\n__CPUINFO__\nprocessor : 0\nmodel name : Example CPU\ncpu MHz : 3800.000\nprocessor : 1\nmodel name : Example CPU\ncpu MHz : 3600.000\n__NPROC_ALL__\n2\n__NPROC_ONLINE__\n2\n__LSCPU__\nCPU(s): 2\nOn-line CPU(s) list: 0-1\nModel name: Example CPU\n";
        let mut previous = Some(CpuStatSample {
            idle: 50,
            total: 100,
        });
        let cpu = parse_cpu_command_output(output, &mut previous).unwrap();
        assert_eq!(cpu.model_name.as_deref(), Some("Example CPU"));
        assert_eq!(cpu.total_cores, Some(2));
        assert_eq!(cpu.online_cores, Some(2));
        assert_eq!(cpu.load_avg1, Some(2.41));
        assert_eq!(cpu.avg_clock_ghz, Some(3.7));
        assert_eq!(cpu.usage_percent, Some(50.0));
    }

    fn mib_to_gib_for_test(value: u64) -> f64 {
        value as f64 / 1024.0
    }

    #[test]
    fn detects_total_telemetry_failure() {
        let mut telemetry = RemoteTelemetry {
            session_id: "session-test".to_string(),
            timestamp: String::new(),
            hostname: None,
            cpu: None,
            memory: None,
            disks: Vec::new(),
            gpu: Vec::new(),
            users: Vec::new(),
            agents: Vec::new(),
            errors: TelemetryErrors {
                cpu: Some("failed".to_string()),
                memory: Some("failed".to_string()),
                disk: Some("failed".to_string()),
                gpu: Some("failed".to_string()),
                users: Some("failed".to_string()),
                agents: Some("failed".to_string()),
            },
        };
        assert!(telemetry_all_failed(&telemetry));

        // A healthy hostname (or any section) means the transport is alive.
        telemetry.hostname = Some("node01".to_string());
        assert!(!telemetry_all_failed(&telemetry));
    }

    #[test]
    fn parses_who_output_variants() {
        let sessions = parse_who_output(
            "alice    pts/0        2026-07-15 09:12 (10.0.0.5)\nbob      tty1         2026-07-14 22:03\ncarol    pts/2        Jul 15 09:30 (workstation.local)\n",
        );
        assert_eq!(sessions.len(), 3);
        assert_eq!(sessions[0].user, "alice");
        assert_eq!(sessions[0].tty, "pts/0");
        assert_eq!(sessions[0].login_time, "2026-07-15 09:12");
        assert_eq!(sessions[0].from.as_deref(), Some("10.0.0.5"));
        assert_eq!(sessions[1].from, None);
        assert_eq!(sessions[1].login_time, "2026-07-14 22:03");
        assert_eq!(sessions[2].login_time, "Jul 15 09:30");
        assert_eq!(sessions[2].from.as_deref(), Some("workstation.local"));
    }

    #[test]
    fn parses_empty_who_output_as_no_sessions() {
        assert!(parse_who_output("").is_empty());
        assert!(parse_who_output("\n  \n").is_empty());
    }
}
