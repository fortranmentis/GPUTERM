use crate::ssh::gpu_monitor::{parse_nvidia_smi_csv, GpuMetric, NVIDIA_SMI_QUERY};
use crate::ssh::session::{open_ssh_session, AppState, SshTarget};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use ssh2::Session;
use std::collections::{HashMap, HashSet};
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
    "tmpfs", "devtmpfs", "squashfs", "proc", "sysfs", "cgroup", "cgroup2", "overlay",
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
    model_name: Option<String>,
    usage_percent: Option<f64>,
    load_avg1: Option<f64>,
    load_avg5: Option<f64>,
    load_avg15: Option<f64>,
    total_cores: Option<u64>,
    online_cores: Option<u64>,
    avg_clock_ghz: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryMetric {
    total_mi_b: Option<u64>,
    used_mi_b: Option<u64>,
    available_mi_b: Option<u64>,
    free_mi_b: Option<u64>,
    usage_percent: Option<f64>,
    swap_total_mi_b: Option<u64>,
    swap_used_mi_b: Option<u64>,
    swap_free_mi_b: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskMetric {
    filesystem: String,
    fs_type: Option<String>,
    mount_point: String,
    total_bytes: Option<u64>,
    used_bytes: Option<u64>,
    available_bytes: Option<u64>,
    usage_percent: Option<f64>,
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
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteTelemetry {
    timestamp: String,
    hostname: Option<String>,
    cpu: Option<CpuMetric>,
    memory: Option<MemoryMetric>,
    disks: Vec<DiskMetric>,
    gpu: Vec<GpuMetric>,
    errors: TelemetryErrors,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuStatSample {
    idle: u64,
    total: u64,
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

pub fn start(
    app: AppHandle,
    target: SshTarget,
    stop: Arc<AtomicBool>,
    settings: Arc<Mutex<SystemMonitorSettings>>,
) {
    thread::spawn(move || {
        let session = match open_ssh_session(&target) {
            Ok(session) => session,
            Err(error) => {
                emit_telemetry(
                    &app,
                    RemoteTelemetry {
                        timestamp: timestamp(),
                        hostname: None,
                        cpu: None,
                        memory: None,
                        disks: Vec::new(),
                        gpu: Vec::new(),
                        errors: TelemetryErrors {
                            cpu: Some(format!(
                                "Telemetry SSH connection failed: {}",
                                error
                            )),
                            memory: Some("Telemetry SSH connection failed".to_string()),
                            disk: Some("Telemetry SSH connection failed".to_string()),
                            gpu: Some("Telemetry SSH connection failed".to_string()),
                        },
                    },
                );
                return;
            }
        };
        session.set_timeout(COMMAND_TIMEOUT_MS);

        let mut previous_cpu = None;
        while !stop.load(Ordering::SeqCst) {
            let settings_snapshot = settings
                .lock()
                .map(|settings| settings.clone())
                .unwrap_or_default();
            let telemetry = collect_remote_telemetry(
                &session,
                &settings_snapshot,
                &mut previous_cpu,
            );
            emit_telemetry(&app, telemetry);
            sleep_with_stop(settings_snapshot.telemetry_interval_secs, &stop);
        }
    });
}

fn emit_telemetry(app: &AppHandle, telemetry: RemoteTelemetry) {
    let _ = app.emit("remote-telemetry", telemetry);
}

fn collect_remote_telemetry(
    session: &Session,
    _settings: &SystemMonitorSettings,
    previous_cpu: &mut Option<CpuStatSample>,
) -> RemoteTelemetry {
    let mut errors = TelemetryErrors::default();

    let hostname = run_remote_command(session, "hostname 2>/dev/null || uname -n 2>/dev/null || true")
        .ok()
        .map(|hostname| hostname.trim().to_string())
        .filter(|hostname| !hostname.is_empty());

    let cpu = match run_remote_command(session, CPU_COMMAND)
        .and_then(|output| parse_cpu_command_output(&output, previous_cpu))
    {
        Ok(metric) => Some(metric),
        Err(error) => {
            errors.cpu = Some(error);
            None
        }
    };

    let memory = match run_remote_command(session, "cat /proc/meminfo 2>/dev/null")
        .and_then(|output| parse_meminfo(&output))
    {
        Ok(metric) => Some(metric),
        Err(error) => {
            errors.memory = Some(error);
            None
        }
    };

    let disks = match run_remote_command(session, "df -P -T -B1 2>/dev/null")
        .and_then(|output| parse_df_output(&output))
    {
        Ok(disks) => filter_and_sort_disks(disks, &[]),
        Err(error) => {
            errors.disk = Some(error);
            Vec::new()
        }
    };

    let gpu = match run_remote_command(session, NVIDIA_SMI_QUERY)
        .and_then(|output| parse_nvidia_smi_csv(&output))
    {
        Ok(metrics) => metrics,
        Err(error) => {
            errors.gpu = Some(error);
            Vec::new()
        }
    };

    RemoteTelemetry {
        timestamp: timestamp(),
        hostname,
        cpu,
        memory,
        disks,
        gpu,
        errors,
    }
}

fn run_remote_command(session: &Session, command: &str) -> Result<String, String> {
    let wrapped = format!(
        "timeout {}s sh -lc {}",
        COMMAND_TIMEOUT_SECS,
        shell_quote(command)
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
        .and_then(|content| parse_loadavg(content).ok())
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
        model_name: parse_cpu_model_name(cpuinfo).or_else(|| parse_lscpu_value(lscpu, "Model name")),
        usage_percent,
        load_avg1: load.0,
        load_avg5: load.1,
        load_avg15: load.2,
        total_cores,
        online_cores,
        avg_clock_ghz: parse_average_cpu_clock_ghz(cpuinfo)
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
    let mut values = HashMap::new();
    for line in output.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let Some(value) = rest.split_whitespace().next() else {
            continue;
        };
        if let Ok(kib) = value.parse::<u64>() {
            values.insert(key.to_string(), kib);
        }
    }

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

pub fn filter_and_sort_disks(
    mut disks: Vec<DiskMetric>,
    ignore_fs_types: &[String],
) -> Vec<DiskMetric> {
    let ignored = ignore_fs_types
        .iter()
        .map(|item| item.to_lowercase())
        .collect::<HashSet<_>>();
    disks.retain(|disk| {
        disk.fs_type
            .as_ref()
            .map(|fs_type| !ignored.contains(&fs_type.to_lowercase()))
            .unwrap_or(true)
    });
    disks.sort_by(|a, b| {
        disk_priority(&a.mount_point)
            .cmp(&disk_priority(&b.mount_point))
            .then_with(|| a.mount_point.cmp(&b.mount_point))
    });
    disks
}

fn split_sections(output: &str) -> HashMap<String, String> {
    let mut sections = HashMap::new();
    let mut current = None::<String>;
    for line in output.lines() {
        if let Some(section) = line
            .trim()
            .strip_prefix("__")
            .and_then(|value| value.strip_suffix("__"))
        {
            current = Some(section.to_string());
            sections.entry(section.to_string()).or_insert_with(String::new);
            continue;
        }
        if let Some(section) = &current {
            sections
                .entry(section.clone())
                .or_insert_with(String::new)
                .push_str(line);
            sections
                .entry(section.clone())
                .or_insert_with(String::new)
                .push('\n');
        }
    }
    sections
}

fn required_section<'a>(
    sections: &'a HashMap<String, String>,
    name: &str,
) -> Result<&'a str, String> {
    sections
        .get(name)
        .map(String::as_str)
        .filter(|content| !content.trim().is_empty())
        .ok_or_else(|| format!("missing {} telemetry section", name))
}

fn parse_loadavg(output: &str) -> Result<(Option<f64>, Option<f64>, Option<f64>), String> {
    let mut values = output.split_whitespace();
    Ok((
        values.next().and_then(|value| value.parse::<f64>().ok()),
        values.next().and_then(|value| value.parse::<f64>().ok()),
        values.next().and_then(|value| value.parse::<f64>().ok()),
    ))
}

fn parse_cpu_model_name(cpuinfo: &str) -> Option<String> {
    cpuinfo.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        if key.trim() == "model name" {
            Some(value.trim().to_string())
        } else {
            None
        }
    })
}

fn parse_average_cpu_clock_ghz(cpuinfo: &str) -> Option<f64> {
    let values = cpuinfo
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            if key.trim() == "cpu MHz" {
                value.trim().parse::<f64>().ok()
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    if values.is_empty() {
        return None;
    }
    Some((values.iter().sum::<f64>() / values.len() as f64) / 1000.0)
}

fn parse_cpuinfo_processor_count(cpuinfo: &str) -> Option<u64> {
    let count = cpuinfo
        .lines()
        .filter(|line| line.split_once(':').map(|(key, _)| key.trim() == "processor").unwrap_or(false))
        .count();
    (count > 0).then_some(count as u64)
}

fn parse_lscpu_value(lscpu: &str, key: &str) -> Option<String> {
    lscpu.lines().find_map(|line| {
        let (line_key, value) = line.split_once(':')?;
        (line_key.trim() == key).then(|| value.trim().to_string())
    })
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

fn parse_first_u64(output: &str) -> Option<u64> {
    output
        .split_whitespace()
        .next()
        .and_then(|value| value.parse::<u64>().ok())
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

fn kib_to_mib(value: u64) -> u64 {
    value / 1024
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
    let ticks = interval_secs.max(1) * 10;
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
    fn filters_and_prioritizes_disk_mounts() {
        let disks = parse_df_output(
            "Filesystem Type 1-blocks Used Available Use% Mounted on\ntmpfs tmpfs 10 1 9 10% /run\n/dev/sdb1 xfs 100 20 80 20% /data\n/dev/sda1 ext4 100 30 70 30% /\n",
        )
        .unwrap();
        let filtered = filter_and_sort_disks(disks, &["tmpfs".to_string()]);
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].mount_point, "/");
        assert_eq!(filtered[1].mount_point, "/data");
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
}
