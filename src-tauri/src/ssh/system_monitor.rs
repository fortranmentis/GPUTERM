use crate::ssh::gpu_monitor::{
    parse_gpu_probe, parse_intel_gpu_top_stream, parse_nvidia_smi_csv, parse_rocm_smi_json,
    parse_xpu_discovery, parse_xpu_stats, xpu_stats_command, GpuMetric, GpuProbe,
    GPU_PROBE_COMMAND, INTEL_GPU_TOP_COMMAND, NVIDIA_SMI_QUERY, XPU_DISCOVERY_COMMAND,
};
use crate::ssh::macos_monitor;
use crate::ssh::parse_util::{
    kib_to_mib, parse_average_clock, parse_cpu_model, parse_first_u64, parse_loadavg,
    parse_lscpu_value, parse_meminfo_values, required_section, split_sections,
};
use crate::ssh::session::{open_ssh_session, AppState, SshTarget};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use ssh2::Session;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};

const COMMAND_TIMEOUT_SECS: u64 = 3;
const COMMAND_TIMEOUT_MS: u32 = 3_000;
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
    user: String,
    tty: String,
    login_time: String,
    from: Option<String>,
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
    errors: TelemetryErrors,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuStatSample {
    idle: u64,
    total: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoteOs {
    Linux,
    MacOs,
}

pub(crate) fn detect_remote_os(session: &Session) -> Option<RemoteOs> {
    let output = run_remote_command(session, "uname -s 2>/dev/null || true").ok()?;
    Some(if output.trim() == "Darwin" {
        RemoteOs::MacOs
    } else {
        RemoteOs::Linux
    })
}

#[tauri::command]
pub fn get_telemetry_settings(
    state: State<AppState>,
) -> Result<SystemMonitorSettings, String> {
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
) {
    thread::spawn(move || {
        let mut backoff = RECONNECT_BACKOFF_INITIAL;
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
                }
                let telemetry = collect_remote_telemetry(
                    &target.session_id,
                    session,
                    host_os.unwrap_or(RemoteOs::Linux),
                    &mut previous_cpu,
                    &mut gpu_probe,
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
            errors: TelemetryErrors {
                cpu: Some(format!("Telemetry SSH connection failed: {}", error)),
                memory: Some("Telemetry SSH connection failed".to_string()),
                disk: Some("Telemetry SSH connection failed".to_string()),
                gpu: Some("Telemetry SSH connection failed".to_string()),
                users: Some("Telemetry SSH connection failed".to_string()),
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
        && telemetry.errors.cpu.is_some()
        && telemetry.errors.memory.is_some()
        && telemetry.errors.disk.is_some()
}

fn emit_telemetry(app: &AppHandle, telemetry: RemoteTelemetry) {
    let _ = app.emit("remote-telemetry", telemetry);
}

fn collect_remote_telemetry(
    session_id: &str,
    session: &Session,
    os: RemoteOs,
    previous_cpu: &mut Option<CpuStatSample>,
    gpu_probe: &mut Option<GpuProbe>,
) -> RemoteTelemetry {
    let mut errors = TelemetryErrors::default();

    let hostname = run_remote_command(session, "hostname 2>/dev/null || uname -n 2>/dev/null || true")
        .ok()
        .map(|hostname| hostname.trim().to_string())
        .filter(|hostname| !hostname.is_empty());

    let cpu_result = match os {
        RemoteOs::Linux => run_remote_command(session, CPU_COMMAND)
            .and_then(|output| parse_cpu_command_output(&output, previous_cpu)),
        RemoteOs::MacOs => run_remote_command(session, macos_monitor::MACOS_CPU_COMMAND)
            .and_then(|output| macos_monitor::parse_macos_cpu_output(&output)),
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

    let gpu = match collect_gpu_metrics(session, gpu_probe) {
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

    RemoteTelemetry {
        session_id: session_id.to_string(),
        timestamp: timestamp(),
        hostname,
        cpu,
        memory,
        disks,
        gpu,
        users,
        errors,
    }
}

/// Detects available GPU tools once per connection (cached in `probe`), then
/// collects metrics from every detected vendor and concatenates them.
pub(crate) fn collect_gpu_metrics(
    session: &Session,
    probe: &mut Option<GpuProbe>,
) -> Result<Vec<GpuMetric>, String> {
    if probe.is_none() {
        let mut detected = run_remote_command(session, GPU_PROBE_COMMAND)
            .map(|output| parse_gpu_probe(&output))
            .unwrap_or_default();
        if detected.xpu_smi {
            detected.xpu_devices = run_remote_command(session, XPU_DISCOVERY_COMMAND)
                .map(|output| parse_xpu_discovery(&output))
                .unwrap_or_default();
        }
        *probe = Some(detected);
    }
    let probe = probe.as_ref().expect("probe is populated above");

    if !probe.any() {
        return Err(
            "No GPU monitoring source found (nvidia-smi / rocm-smi / xpu-smi / intel_gpu_top / macOS ioreg)"
                .to_string(),
        );
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

    if metrics.is_empty() && !errors.is_empty() {
        return Err(errors.join("; "));
    }
    Ok(metrics)
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
            let time_end = if from.is_some() { fields.len() - 1 } else { fields.len() };
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
    // `timeout` is missing on macOS <= 12, so fall back to a bare shell there;
    // libssh2's session timeout still bounds the fallback branch.
    let quoted = shell_quote(command);
    let wrapped = format!(
        "command -v timeout >/dev/null 2>&1 && exec timeout {secs}s sh -lc {quoted}; exec sh -lc {quoted}",
        secs = COMMAND_TIMEOUT_SECS,
        quoted = quoted
    );
    let mut channel = session
        .channel_session()
        .map_err(|error| format!("failed to open telemetry command channel: {}", error))?;
    channel
        .exec(&wrapped)
        .map_err(|error| format!("failed to execute telemetry command: {}", error))?;

    let mut stdout = String::new();
    channel
        .read_to_string(&mut stdout)
        .map_err(|error| format!("failed to read telemetry command output: {}", error))?;

    let mut stderr = String::new();
    let _ = channel.stderr().read_to_string(&mut stderr);
    let _ = channel.wait_close();
    let exit_status = channel.exit_status().unwrap_or(-1);

    if exit_status == 124 {
        return Err(format!(
            "telemetry command timed out after {}s",
            COMMAND_TIMEOUT_SECS
        ));
    }

    if exit_status != 0 {
        let detail = if stderr.trim().is_empty() {
            format!("telemetry command exited with status {}", exit_status)
        } else {
            stderr.trim().to_string()
        };
        return Err(detail);
    }

    Ok(stdout)
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
    let usage_percent = previous_cpu
        .and_then(|previous| calculate_cpu_usage(previous, current_cpu));
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
    for line in output.lines().skip(1).map(str::trim).filter(|line| !line.is_empty()) {
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
        .filter(|line| line.split_once(':').map(|(key, _)| key.trim() == "processor").unwrap_or(false))
        .count();
    (count > 0).then_some(count as u64)
}

fn parse_lscpu_cpu_count(lscpu: &str) -> Option<u64> {
    parse_lscpu_value(lscpu, "CPU(s)").and_then(|value| value.parse::<u64>().ok())
}

fn parse_lscpu_online_count(lscpu: &str) -> Option<u64> {
    parse_lscpu_value(lscpu, "On-line CPU(s) list")
        .and_then(|value| parse_cpu_list_count(&value))
}

fn parse_lscpu_cpu_mhz(lscpu: &str) -> Option<f64> {
    parse_lscpu_value(lscpu, "CPU MHz")
        .or_else(|| parse_lscpu_value(lscpu, "CPU max MHz"))
        .and_then(|value| value.parse::<f64>().ok())
}

fn parse_cpu_list_count(value: &str) -> Option<u64> {
    let mut count = 0_u64;
    for part in value.split(',').map(str::trim).filter(|part| !part.is_empty()) {
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
        _ => 100,
    }
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
            errors: TelemetryErrors {
                cpu: Some("failed".to_string()),
                memory: Some("failed".to_string()),
                disk: Some("failed".to_string()),
                gpu: Some("failed".to_string()),
                users: Some("failed".to_string()),
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
