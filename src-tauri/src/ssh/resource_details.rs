use crate::ssh::gpu_monitor::GPU_PROBE_COMMAND;
use crate::ssh::gpu_monitor::{parse_gpu_probe, GpuMetric};
use crate::ssh::macos_monitor::{
    parse_kern_boottime, parse_macos_cpu_output, parse_swapusage, parse_vm_stat,
    MACOS_CPU_DETAIL_COMMAND, MACOS_MEMORY_DETAIL_COMMAND,
};
use crate::ssh::parse_util::{
    kib_to_mib, optional_string, parse_average_clock, parse_cpu_model, parse_first_u64,
    parse_loadavg, parse_lscpu_value, parse_meminfo_values, parse_optional_f64, parse_optional_u64,
    required_section, split_sections,
};
use crate::ssh::session::{target_for_active_session, with_ops_session, AppState};
use crate::ssh::system_monitor::{
    calculate_cpu_usage, collect_gpu_metrics, collect_local_gpu_metrics, detect_remote_os,
    local_os, run_local_command_for, run_remote_command, run_remote_command_for, RemoteOs,
    WINDOWS_COMMAND_TIMEOUT_MS,
};
use crate::ssh::windows_monitor::{
    parse_windows_cpu_info, parse_windows_cpu_times, parse_windows_gpu_counters,
    parse_windows_gpu_probe, parse_windows_memory_output, parse_windows_process_identity,
    parse_windows_process_samples, windows_process_identity_command, WINDOWS_CPU_DETAIL_COMMAND,
    WINDOWS_GPU_COUNTERS_COMMAND, WINDOWS_GPU_PROBE_COMMAND, WINDOWS_MEMORY_DETAIL_COMMAND,
};
use serde::Serialize;
use ssh2::Session;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tauri::State;

const DETAIL_TIMEOUT_MS: u32 = 3_000;
const CPU_DETAIL_COMMAND: &str = "printf '__PROC_STAT_1__\\n'; cat /proc/stat 2>/dev/null; sleep 0.2; printf '\\n__PROC_STAT_2__\\n'; cat /proc/stat 2>/dev/null; printf '\\n__LOADAVG__\\n'; cat /proc/loadavg 2>/dev/null; printf '\\n__CPUINFO__\\n'; cat /proc/cpuinfo 2>/dev/null; printf '\\n__NPROC_ALL__\\n'; nproc --all 2>/dev/null || true; printf '\\n__NPROC_ONLINE__\\n'; nproc 2>/dev/null || true; printf '\\n__LSCPU__\\n'; lscpu 2>/dev/null || true; printf '\\n__UPTIME__\\n'; cat /proc/uptime 2>/dev/null; printf '\\n__PROCESSES__\\n'; ps -eo pid=,user=,%cpu=,%mem=,etime=,comm= --sort=-%cpu 2>/dev/null | head -n 15";
const MEMORY_DETAIL_COMMAND: &str = "printf '__MEMINFO__\\n'; cat /proc/meminfo 2>/dev/null; printf '\\n__PROCESSES__\\n'; ps -eo pid=,user=,rss=,vsz=,%mem=,comm= --sort=-rss 2>/dev/null | head -n 15";
const GPU_DETAIL_QUERY: &str = "nvidia-smi --query-gpu=index,name,uuid,driver_version,power.draw,power.limit,temperature.gpu,utilization.gpu,utilization.memory,memory.total,memory.used,memory.free,fan.speed,clocks.current.graphics,clocks.current.memory,pci.bus_id,persistence_mode,mig.mode.current --format=csv,noheader,nounits";
const GPU_PROCESS_QUERY: &str = "nvidia-smi --query-compute-apps=gpu_uuid,pid,process_name,used_memory --format=csv,noheader,nounits";

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessMetric {
    pid: u32,
    user: Option<String>,
    command: Option<String>,
    cpu_percent: Option<f64>,
    memory_percent: Option<f64>,
    rss_bytes: Option<u64>,
    vsz_bytes: Option<u64>,
    elapsed_time: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CpuDetailMetric {
    model_name: Option<String>,
    usage_percent: Option<f64>,
    load_avg1: Option<f64>,
    load_avg5: Option<f64>,
    load_avg15: Option<f64>,
    total_cores: Option<u64>,
    online_cores: Option<u64>,
    avg_clock_ghz: Option<f64>,
    uptime_seconds: Option<f64>,
    logical_core_usage_percent: Vec<Option<f64>>,
    top_processes: Vec<ProcessMetric>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryDetailMetric {
    total_mi_b: Option<u64>,
    used_mi_b: Option<u64>,
    available_mi_b: Option<u64>,
    free_mi_b: Option<u64>,
    buffers_mi_b: Option<u64>,
    cached_mi_b: Option<u64>,
    swap_total_mi_b: Option<u64>,
    swap_used_mi_b: Option<u64>,
    swap_free_mi_b: Option<u64>,
    usage_percent: Option<f64>,
    top_processes: Vec<ProcessMetric>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuProcessMetric {
    gpu_index: Option<u32>,
    gpu_uuid: Option<String>,
    pid: u32,
    user: Option<String>,
    process_name: Option<String>,
    command: Option<String>,
    used_memory_mi_b: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuDetailMetric {
    index: u32,
    name: String,
    uuid: String,
    driver_version: Option<String>,
    gpu_util_percent: Option<f64>,
    memory_util_percent: Option<f64>,
    memory_total_mi_b: Option<u64>,
    memory_used_mi_b: Option<u64>,
    memory_free_mi_b: Option<u64>,
    temperature_c: Option<f64>,
    power_draw_w: Option<f64>,
    power_limit_w: Option<f64>,
    fan_speed_percent: Option<f64>,
    graphics_clock_m_hz: Option<f64>,
    memory_clock_m_hz: Option<f64>,
    pci_bus_id: Option<String>,
    persistence_mode: Option<String>,
    mig_mode: Option<String>,
    processes: Vec<GpuProcessMetric>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ResourceDetailErrors {
    #[serde(skip_serializing_if = "Option::is_none")]
    cpu: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    memory: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gpu: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceDetails {
    cpu: Option<CpuDetailMetric>,
    memory: Option<MemoryDetailMetric>,
    gpus: Vec<GpuDetailMetric>,
    errors: ResourceDetailErrors,
}

#[derive(Clone, Copy)]
enum ResourceType {
    Cpu,
    Memory,
    Gpu,
}

impl ResourceType {
    fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_lowercase().as_str() {
            "cpu" => Ok(Self::Cpu),
            "memory" | "ram" => Ok(Self::Memory),
            "gpu" => Ok(Self::Gpu),
            _ => Err(format!("Unsupported resource type: {}", value)),
        }
    }
}

#[tauri::command]
pub async fn get_resource_details(
    state: State<'_, AppState>,
    session_id: String,
    resource_type: String,
) -> Result<ResourceDetails, String> {
    let resource_type = ResourceType::parse(&resource_type)?;
    let is_local = state
        .active_connections
        .lock()
        .map_err(|_| "Active session state is unavailable".to_string())?
        .get(&session_id)
        .map(|connection| connection.profile.is_local)
        .ok_or_else(|| "No active terminal session is available".to_string())?;
    if is_local {
        return tauri::async_runtime::spawn_blocking(move || {
            let mut details = ResourceDetails::default();
            match collect_local_details(resource_type) {
                Ok(ResourceDetailValue::Cpu(metric)) => details.cpu = Some(metric),
                Ok(ResourceDetailValue::Memory(metric)) => details.memory = Some(metric),
                Ok(ResourceDetailValue::Gpu(metrics)) => details.gpus = metrics,
                Err(error) => {
                    details.set_error(resource_type, format!("Metrics unavailable: {}", error))
                }
            }
            details
        })
        .await
        .map_err(|error| format!("Local resource detail task failed: {}", error));
    }
    let target = target_for_active_session(&state, &session_id)?;
    let ops = Arc::clone(&state.ops_sessions);
    let os_cache = Arc::clone(&state.remote_os_cache);
    tauri::async_runtime::spawn_blocking(move || {
        let mut details = ResourceDetails::default();
        let result = with_ops_session(&ops, &target, DETAIL_TIMEOUT_MS, |session| {
            let os = resolve_remote_os(&os_cache, &target.session_id, session);
            if os == RemoteOs::Windows {
                // PowerShell start-up plus the 500 ms counter sampling exceed
                // the Unix detail timeout.
                session.set_timeout(WINDOWS_COMMAND_TIMEOUT_MS);
            }
            collect_details(session, resource_type, os)
        });
        match result {
            Ok(ResourceDetailValue::Cpu(metric)) => details.cpu = Some(metric),
            Ok(ResourceDetailValue::Memory(metric)) => details.memory = Some(metric),
            Ok(ResourceDetailValue::Gpu(metrics)) => details.gpus = metrics,
            Err(error) => {
                details.set_error(resource_type, format!("Metrics unavailable: {}", error))
            }
        }
        details
    })
    .await
    .map_err(|error| format!("Resource detail task failed: {}", error))
}

fn collect_local_details(resource_type: ResourceType) -> Result<ResourceDetailValue, String> {
    let os = local_os();
    match (resource_type, os) {
        (ResourceType::Cpu, RemoteOs::Linux) => run_local_command_for(os, CPU_DETAIL_COMMAND)
            .and_then(|output| parse_cpu_detail_output(&output))
            .map(ResourceDetailValue::Cpu),
        (ResourceType::Cpu, RemoteOs::MacOs) => run_local_command_for(os, MACOS_CPU_DETAIL_COMMAND)
            .and_then(|output| parse_macos_cpu_detail_output(&output))
            .map(ResourceDetailValue::Cpu),
        (ResourceType::Cpu, RemoteOs::Windows) => {
            run_local_command_for(os, WINDOWS_CPU_DETAIL_COMMAND)
                .and_then(|output| parse_windows_cpu_detail_output(&output))
                .map(ResourceDetailValue::Cpu)
        }
        (ResourceType::Memory, RemoteOs::Linux) => run_local_command_for(os, MEMORY_DETAIL_COMMAND)
            .and_then(|output| parse_memory_detail_output(&output))
            .map(ResourceDetailValue::Memory),
        (ResourceType::Memory, RemoteOs::MacOs) => {
            run_local_command_for(os, MACOS_MEMORY_DETAIL_COMMAND)
                .and_then(|output| parse_macos_memory_detail_output(&output))
                .map(ResourceDetailValue::Memory)
        }
        (ResourceType::Memory, RemoteOs::Windows) => {
            run_local_command_for(os, WINDOWS_MEMORY_DETAIL_COMMAND)
                .and_then(|output| parse_windows_memory_detail_output(&output))
                .map(ResourceDetailValue::Memory)
        }
        (ResourceType::Gpu, _) => {
            let mut probe = None;
            collect_local_gpu_metrics(os, &mut probe)
                .map(|metrics| metrics.into_iter().map(gpu_metric_to_detail).collect())
                .map(ResourceDetailValue::Gpu)
        }
    }
}

impl ResourceDetails {
    fn set_error(&mut self, resource_type: ResourceType, error: String) {
        match resource_type {
            ResourceType::Cpu => self.errors.cpu = Some(error),
            ResourceType::Memory => self.errors.memory = Some(error),
            ResourceType::Gpu => self.errors.gpu = Some(error),
        }
    }
}

enum ResourceDetailValue {
    Cpu(CpuDetailMetric),
    Memory(MemoryDetailMetric),
    Gpu(Vec<GpuDetailMetric>),
}

/// Returns the cached remote OS for the session, detecting and caching it on
/// first use. Detection failure (dead transport) falls back to Linux like the
/// poller, without poisoning the cache.
fn resolve_remote_os(
    cache: &Mutex<HashMap<String, RemoteOs>>,
    session_id: &str,
    session: &Session,
) -> RemoteOs {
    if let Ok(map) = cache.lock() {
        if let Some(os) = map.get(session_id) {
            return *os;
        }
    }
    match detect_remote_os(session) {
        Some(os) => {
            if let Ok(mut map) = cache.lock() {
                map.insert(session_id.to_string(), os);
            }
            os
        }
        None => RemoteOs::Linux,
    }
}

fn collect_details(
    session: &Session,
    resource_type: ResourceType,
    os: RemoteOs,
) -> Result<ResourceDetailValue, String> {
    match (resource_type, os) {
        (ResourceType::Cpu, RemoteOs::Linux) => run_remote_command(session, CPU_DETAIL_COMMAND)
            .and_then(|output| parse_cpu_detail_output(&output))
            .map(ResourceDetailValue::Cpu),
        (ResourceType::Cpu, RemoteOs::MacOs) => {
            run_remote_command(session, MACOS_CPU_DETAIL_COMMAND)
                .and_then(|output| parse_macos_cpu_detail_output(&output))
                .map(ResourceDetailValue::Cpu)
        }
        (ResourceType::Cpu, RemoteOs::Windows) => {
            run_remote_command_for(session, os, WINDOWS_CPU_DETAIL_COMMAND)
                .and_then(|output| parse_windows_cpu_detail_output(&output))
                .map(ResourceDetailValue::Cpu)
        }
        (ResourceType::Memory, RemoteOs::Linux) => {
            run_remote_command(session, MEMORY_DETAIL_COMMAND)
                .and_then(|output| parse_memory_detail_output(&output))
                .map(ResourceDetailValue::Memory)
        }
        (ResourceType::Memory, RemoteOs::MacOs) => {
            run_remote_command(session, MACOS_MEMORY_DETAIL_COMMAND)
                .and_then(|output| parse_macos_memory_detail_output(&output))
                .map(ResourceDetailValue::Memory)
        }
        (ResourceType::Memory, RemoteOs::Windows) => {
            run_remote_command_for(session, os, WINDOWS_MEMORY_DETAIL_COMMAND)
                .and_then(|output| parse_windows_memory_detail_output(&output))
                .map(ResourceDetailValue::Memory)
        }
        (ResourceType::Gpu, _) => collect_gpu_details(session, os).map(ResourceDetailValue::Gpu),
    }
}

fn collect_gpu_details(session: &Session, os: RemoteOs) -> Result<Vec<GpuDetailMetric>, String> {
    if os == RemoteOs::Windows {
        return collect_windows_gpu_details(session);
    }
    // NVIDIA keeps its rich per-process detail path; AMD/Intel reuse the
    // telemetry collectors and map into the same shape with nulls for the
    // fields their tools do not expose.
    let has_nvidia = run_remote_command(session, GPU_PROBE_COMMAND)
        .map(|output| parse_gpu_probe(&output).nvidia)
        .unwrap_or(true);
    if !has_nvidia {
        let mut probe = None;
        let metrics = collect_gpu_metrics(session, os, &mut probe)?;
        return Ok(metrics.into_iter().map(gpu_metric_to_detail).collect());
    }
    let gpu_output = run_remote_command(session, GPU_DETAIL_QUERY)?;
    let process_output = run_remote_command(session, GPU_PROCESS_QUERY).unwrap_or_default();
    let process_rows = parse_gpu_process_output(&process_output)?;
    let pids = process_rows
        .iter()
        .map(|process| process.pid.to_string())
        .collect::<Vec<_>>();
    let ps_output = if pids.is_empty() {
        String::new()
    } else {
        let command = format!(
            "ps -p {} -o pid=,user=,args= 2>/dev/null || true",
            pids.join(",")
        );
        run_remote_command(session, &command).unwrap_or_default()
    };
    let identity = parse_ps_identity_output(&ps_output);
    parse_gpu_detail_output(&gpu_output, process_rows, &identity)
}

/// Windows GPU details: the nvidia-smi queries are argument-identical to the
/// Linux path, so only the command transport and the PID→identity join
/// (Win32_Process instead of `ps -p`) differ. Hosts without nvidia-smi fall
/// back to the counter-based telemetry collectors, like AMD/Intel on Linux.
fn collect_windows_gpu_details(session: &Session) -> Result<Vec<GpuDetailMetric>, String> {
    let probe = run_remote_command_for(session, RemoteOs::Windows, WINDOWS_GPU_PROBE_COMMAND)
        .map(|output| parse_windows_gpu_probe(&output))
        .unwrap_or_default();
    if !probe.nvidia {
        let mut probe = Some(probe);
        let metrics = collect_gpu_metrics(session, RemoteOs::Windows, &mut probe)?;
        return Ok(metrics.into_iter().map(gpu_metric_to_detail).collect());
    }
    let gpu_output = run_remote_command_for(session, RemoteOs::Windows, GPU_DETAIL_QUERY)?;
    let process_output =
        run_remote_command_for(session, RemoteOs::Windows, GPU_PROCESS_QUERY).unwrap_or_default();
    let process_rows = parse_gpu_process_output(&process_output)?;
    let pids = process_rows
        .iter()
        .map(|process| process.pid)
        .collect::<Vec<_>>();
    let identity = if pids.is_empty() {
        HashMap::new()
    } else {
        run_remote_command_for(
            session,
            RemoteOs::Windows,
            &windows_process_identity_command(&pids),
        )
        .map(|output| parse_windows_process_identity(&output))
        .unwrap_or_default()
    };
    let mut details = parse_gpu_detail_output(&gpu_output, process_rows, &identity)?;

    // Hybrid hosts: append the adapters nvidia-smi does not cover (e.g. the
    // integrated GPU) from the counters, best-effort — the nvidia cards are
    // already in hand, so a counter failure must not fail the popover.
    let has_other_adapters = probe
        .windows_adapters
        .iter()
        .any(|adapter| adapter.vendor != "nvidia");
    if has_other_adapters {
        let next_index = details.iter().map(|gpu| gpu.index + 1).max().unwrap_or(0);
        if let Ok(extra) =
            run_remote_command_for(session, RemoteOs::Windows, WINDOWS_GPU_COUNTERS_COMMAND)
                .and_then(|output| {
                    parse_windows_gpu_counters(&output, &probe.windows_adapters, true, next_index)
                })
        {
            details.extend(extra.into_iter().map(gpu_metric_to_detail));
        }
    }
    Ok(details)
}

fn parse_macos_cpu_detail_output(output: &str) -> Result<CpuDetailMetric, String> {
    // The command shares its sysctl/top sections with the telemetry collector.
    let base = parse_macos_cpu_output(output)?;
    let sections = split_sections(output);
    let uptime_seconds = match (
        sections
            .get("BOOTTIME")
            .and_then(|value| parse_kern_boottime(value)),
        sections.get("NOW").and_then(|value| parse_first_u64(value)),
    ) {
        (Some(boot), Some(now)) if now > boot => Some((now - boot) as f64),
        _ => None,
    };

    Ok(CpuDetailMetric {
        model_name: base.model_name,
        usage_percent: base.usage_percent,
        load_avg1: base.load_avg1,
        load_avg5: base.load_avg5,
        load_avg15: base.load_avg15,
        total_cores: base.total_cores,
        online_cores: base.online_cores,
        avg_clock_ghz: None,
        uptime_seconds,
        // Per-core usage on macOS requires root (powermetrics); the popover
        // hides the grid for an empty list.
        logical_core_usage_percent: Vec::new(),
        top_processes: sections
            .get("PROCESSES")
            .map(|value| parse_cpu_processes(value))
            .unwrap_or_default(),
    })
}

fn parse_macos_memory_detail_output(output: &str) -> Result<MemoryDetailMetric, String> {
    let sections = split_sections(output);
    let total_bytes = sections
        .get("MEMSIZE")
        .and_then(|value| parse_first_u64(value))
        .ok_or_else(|| "macOS memory details unavailable (hw.memsize missing)".to_string())?;
    let (page_size, pages) = required_section(&sections, "VMSTAT")
        .ok()
        .and_then(parse_vm_stat)
        .ok_or_else(|| "macOS memory details unavailable (vm_stat missing)".to_string())?;
    let count = |label: &str| pages.get(label).copied().unwrap_or(0);
    let used_bytes =
        (count("Pages active") + count("Pages wired down") + count("Pages occupied by compressor"))
            * page_size;
    let free_bytes = count("Pages free") * page_size;
    let cached_bytes = pages
        .get("File-backed pages")
        .map(|file_backed| file_backed * page_size);
    let available_bytes = total_bytes.saturating_sub(used_bytes);
    let (swap_total, swap_used, swap_free) = sections
        .get("SWAP")
        .map(|value| parse_swapusage(value))
        .unwrap_or((None, None, None));
    const MIB: u64 = 1024 * 1024;

    Ok(MemoryDetailMetric {
        total_mi_b: Some(total_bytes / MIB),
        used_mi_b: Some(used_bytes / MIB),
        available_mi_b: Some(available_bytes / MIB),
        free_mi_b: Some(free_bytes / MIB),
        buffers_mi_b: None,
        cached_mi_b: cached_bytes.map(|bytes| bytes / MIB),
        swap_total_mi_b: swap_total,
        swap_used_mi_b: swap_used,
        swap_free_mi_b: swap_free,
        usage_percent: (total_bytes > 0)
            .then_some((used_bytes as f64 / total_bytes as f64) * 100.0),
        top_processes: sections
            .get("PROCESSES")
            .map(|value| parse_memory_processes(value))
            .unwrap_or_default(),
    })
}

fn parse_windows_cpu_detail_output(output: &str) -> Result<CpuDetailMetric, String> {
    let sections = split_sections(output);
    let info = parse_windows_cpu_info(sections.get("CPUINFO").map(String::as_str).unwrap_or(""));
    let first =
        parse_windows_cpu_times(sections.get("CPUTIMES_1").map(String::as_str).unwrap_or(""));
    let second =
        parse_windows_cpu_times(sections.get("CPUTIMES_2").map(String::as_str).unwrap_or(""));
    if info.model_name.is_none() && info.logical_cores.is_none() && second.is_empty() {
        return Err("Windows CPU details unavailable (WMI queries produced no output)".to_string());
    }

    let total_pair = (first.get("_Total"), second.get("_Total"));
    let usage_percent = match total_pair {
        (Some(previous), Some(current)) => calculate_cpu_usage(*previous, *current),
        _ => None,
    };
    // Per-core instances are named "0", "1", ...; "_Total" is excluded.
    let mut core_names: Vec<&String> = second
        .keys()
        .filter(|name| !name.is_empty() && name.chars().all(|c| c.is_ascii_digit()))
        .collect();
    core_names.sort_by_key(|name| name.parse::<u32>().unwrap_or(u32::MAX));
    let logical_core_usage_percent = core_names
        .iter()
        .map(|name| match (first.get(*name), second.get(*name)) {
            (Some(previous), Some(current)) => calculate_cpu_usage(*previous, *current),
            _ => None,
        })
        .collect();

    // Get-Process only exposes cumulative CPU seconds, so per-process usage is
    // the delta over the sampled window (un-normalized across cores, like ps).
    let elapsed_seconds = match total_pair {
        (Some(previous), Some(current)) if current.total > previous.total => {
            Some((current.total - previous.total) as f64 / 1e7)
        }
        _ => None,
    };
    let total_memory_bytes = sections
        .get("TOTALMEM")
        .and_then(|value| parse_first_u64(value))
        .map(|kib| kib * 1024);
    let processes_before =
        parse_windows_process_samples(sections.get("PROC_1").map(String::as_str).unwrap_or(""));
    let processes_after =
        parse_windows_process_samples(sections.get("PROC_2").map(String::as_str).unwrap_or(""));
    let mut top_processes: Vec<ProcessMetric> = processes_after
        .iter()
        .map(|(pid, sample)| {
            let cpu_percent = match (
                elapsed_seconds,
                processes_before.get(pid).and_then(|s| s.cpu_seconds),
                sample.cpu_seconds,
            ) {
                (Some(elapsed), Some(before), Some(after)) if elapsed > 0.0 => {
                    Some((after - before).max(0.0) / elapsed * 100.0)
                }
                _ => None,
            };
            let memory_percent = match (sample.working_set_bytes, total_memory_bytes) {
                (Some(working_set), Some(total)) if total > 0 => {
                    Some(working_set as f64 / total as f64 * 100.0)
                }
                _ => None,
            };
            ProcessMetric {
                pid: *pid,
                // The process owner needs elevation on Windows
                // (Get-Process -IncludeUserName); the popover renders n/a.
                user: None,
                command: sample.name.clone(),
                cpu_percent,
                memory_percent,
                rss_bytes: sample.working_set_bytes,
                vsz_bytes: sample.virtual_bytes,
                elapsed_time: None,
            }
        })
        .collect();
    top_processes.sort_by(|a, b| {
        b.cpu_percent
            .partial_cmp(&a.cpu_percent)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    top_processes.truncate(15);

    Ok(CpuDetailMetric {
        model_name: info.model_name,
        usage_percent,
        // Load averages do not exist on Windows.
        load_avg1: None,
        load_avg5: None,
        load_avg15: None,
        total_cores: info.logical_cores,
        online_cores: info.logical_cores,
        avg_clock_ghz: info.avg_clock_ghz,
        uptime_seconds: sections
            .get("UPTIME")
            .and_then(|value| parse_first_u64(value))
            .map(|value| value as f64),
        logical_core_usage_percent,
        top_processes,
    })
}

fn parse_windows_memory_detail_output(output: &str) -> Result<MemoryDetailMetric, String> {
    let sections = split_sections(output);
    // The MEMORY/PAGEFILE sections are shared with the telemetry collector.
    let base = parse_windows_memory_output(&sections)?;
    let total_bytes = base.total_mi_b.map(|mib| mib * 1024 * 1024);

    let mut top_processes: Vec<ProcessMetric> =
        parse_windows_process_samples(sections.get("PROCESSES").map(String::as_str).unwrap_or(""))
            .iter()
            .map(|(pid, sample)| ProcessMetric {
                pid: *pid,
                user: None,
                command: sample.name.clone(),
                cpu_percent: None,
                memory_percent: match (sample.working_set_bytes, total_bytes) {
                    (Some(working_set), Some(total)) if total > 0 => {
                        Some(working_set as f64 / total as f64 * 100.0)
                    }
                    _ => None,
                },
                rss_bytes: sample.working_set_bytes,
                vsz_bytes: sample.virtual_bytes,
                elapsed_time: None,
            })
            .collect();
    top_processes.sort_by_key(|process| std::cmp::Reverse(process.rss_bytes));
    top_processes.truncate(15);

    Ok(MemoryDetailMetric {
        total_mi_b: base.total_mi_b,
        used_mi_b: base.used_mi_b,
        available_mi_b: base.available_mi_b,
        free_mi_b: base.free_mi_b,
        buffers_mi_b: None,
        cached_mi_b: None,
        swap_total_mi_b: base.swap_total_mi_b,
        swap_used_mi_b: base.swap_used_mi_b,
        swap_free_mi_b: base.swap_free_mi_b,
        usage_percent: base.usage_percent,
        top_processes,
    })
}

fn gpu_metric_to_detail(metric: GpuMetric) -> GpuDetailMetric {
    GpuDetailMetric {
        index: metric.index,
        name: metric.name,
        uuid: metric.uuid,
        driver_version: (!metric.driver_version.is_empty()).then_some(metric.driver_version),
        gpu_util_percent: metric.gpu_util_percent,
        memory_util_percent: metric.mem_util_percent,
        memory_total_mi_b: metric.memory_total_mi_b,
        memory_used_mi_b: metric.memory_used_mi_b,
        memory_free_mi_b: metric.memory_free_mi_b,
        temperature_c: metric.temperature_c,
        power_draw_w: metric.power_draw_w,
        power_limit_w: metric.power_limit_w,
        fan_speed_percent: None,
        graphics_clock_m_hz: None,
        memory_clock_m_hz: None,
        pci_bus_id: None,
        persistence_mode: None,
        mig_mode: None,
        processes: Vec::new(),
    }
}

fn parse_cpu_detail_output(output: &str) -> Result<CpuDetailMetric, String> {
    let sections = split_sections(output);
    let first = parse_cpu_samples(required_section(&sections, "PROC_STAT_1")?)?;
    let second = parse_cpu_samples(required_section(&sections, "PROC_STAT_2")?)?;
    let usage_percent = calculate_sample_usage(first.get("cpu"), second.get("cpu"));
    let mut logical_names = second
        .keys()
        .filter(|name| name.starts_with("cpu") && *name != "cpu")
        .cloned()
        .collect::<Vec<_>>();
    logical_names.sort_by_key(|name| {
        name.trim_start_matches("cpu")
            .parse::<u32>()
            .unwrap_or(u32::MAX)
    });
    let logical_core_usage_percent = logical_names
        .iter()
        .map(|name| calculate_sample_usage(first.get(name), second.get(name)))
        .collect();

    let load = sections
        .get("LOADAVG")
        .map(|value| parse_loadavg(value))
        .unwrap_or((None, None, None));
    let cpuinfo = sections.get("CPUINFO").map(String::as_str).unwrap_or("");
    let lscpu = sections.get("LSCPU").map(String::as_str).unwrap_or("");
    let total_cores = sections
        .get("NPROC_ALL")
        .and_then(|value| parse_first_u64(value))
        .or_else(|| parse_lscpu_value(lscpu, "CPU(s)").and_then(|value| value.parse().ok()));
    let online_cores = sections
        .get("NPROC_ONLINE")
        .and_then(|value| parse_first_u64(value))
        .or(total_cores);

    Ok(CpuDetailMetric {
        model_name: parse_cpu_model(cpuinfo).or_else(|| parse_lscpu_value(lscpu, "Model name")),
        usage_percent,
        load_avg1: load.0,
        load_avg5: load.1,
        load_avg15: load.2,
        total_cores,
        online_cores,
        avg_clock_ghz: parse_average_clock(cpuinfo).or_else(|| {
            parse_lscpu_value(lscpu, "CPU MHz")
                .and_then(|value| value.parse::<f64>().ok())
                .map(|mhz| mhz / 1000.0)
        }),
        uptime_seconds: sections
            .get("UPTIME")
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.parse().ok()),
        logical_core_usage_percent,
        top_processes: sections
            .get("PROCESSES")
            .map(|value| parse_cpu_processes(value))
            .unwrap_or_default(),
    })
}

fn parse_memory_detail_output(output: &str) -> Result<MemoryDetailMetric, String> {
    let sections = split_sections(output);
    let values = parse_meminfo_values(required_section(&sections, "MEMINFO")?);
    let total = values
        .get("MemTotal")
        .copied()
        .ok_or_else(|| "missing MemTotal in /proc/meminfo".to_string())?;
    let free = values.get("MemFree").copied().unwrap_or(0);
    let buffers = values.get("Buffers").copied().unwrap_or(0);
    let cached = values.get("Cached").copied().unwrap_or(0);
    let available = values
        .get("MemAvailable")
        .copied()
        .unwrap_or(free.saturating_add(buffers).saturating_add(cached));
    let used = total.saturating_sub(available);
    let swap_total = values.get("SwapTotal").copied().unwrap_or(0);
    let swap_free = values.get("SwapFree").copied().unwrap_or(0);
    let swap_used = swap_total.saturating_sub(swap_free);

    Ok(MemoryDetailMetric {
        total_mi_b: Some(kib_to_mib(total)),
        used_mi_b: Some(kib_to_mib(used)),
        available_mi_b: Some(kib_to_mib(available)),
        free_mi_b: Some(kib_to_mib(free)),
        buffers_mi_b: Some(kib_to_mib(buffers)),
        cached_mi_b: Some(kib_to_mib(cached)),
        swap_total_mi_b: Some(kib_to_mib(swap_total)),
        swap_used_mi_b: Some(kib_to_mib(swap_used)),
        swap_free_mi_b: Some(kib_to_mib(swap_free)),
        usage_percent: (total > 0).then_some((used as f64 / total as f64) * 100.0),
        top_processes: sections
            .get("PROCESSES")
            .map(|value| parse_memory_processes(value))
            .unwrap_or_default(),
    })
}

fn parse_cpu_processes(output: &str) -> Vec<ProcessMetric> {
    output
        .lines()
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 6 {
                return None;
            }
            Some(ProcessMetric {
                pid: fields[0].parse().ok()?,
                user: optional_string(fields[1]),
                cpu_percent: parse_optional_f64(fields[2]),
                memory_percent: parse_optional_f64(fields[3]),
                elapsed_time: optional_string(fields[4]),
                command: optional_string(&fields[5..].join(" ")),
                ..Default::default()
            })
        })
        .take(15)
        .collect()
}

fn parse_memory_processes(output: &str) -> Vec<ProcessMetric> {
    output
        .lines()
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 6 {
                return None;
            }
            Some(ProcessMetric {
                pid: fields[0].parse().ok()?,
                user: optional_string(fields[1]),
                rss_bytes: fields[2].parse::<u64>().ok().map(|value| value * 1024),
                vsz_bytes: fields[3].parse::<u64>().ok().map(|value| value * 1024),
                memory_percent: parse_optional_f64(fields[4]),
                command: optional_string(&fields[5..].join(" ")),
                ..Default::default()
            })
        })
        .take(15)
        .collect()
}

#[derive(Debug, Clone)]
struct GpuProcessRow {
    gpu_uuid: Option<String>,
    pid: u32,
    process_name: Option<String>,
    used_memory_mi_b: Option<u64>,
}

fn parse_gpu_process_output(output: &str) -> Result<Vec<GpuProcessRow>, String> {
    let mut rows = Vec::new();
    for line in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let fields = line.split(',').map(str::trim).collect::<Vec<_>>();
        if fields.len() < 4 {
            continue;
        }
        let Some(pid) = fields[1].parse::<u32>().ok() else {
            continue;
        };
        rows.push(GpuProcessRow {
            gpu_uuid: optional_string(fields[0]),
            pid,
            process_name: optional_string(fields[2]),
            used_memory_mi_b: parse_optional_u64(fields[3]),
        });
    }
    Ok(rows)
}

fn parse_gpu_detail_output(
    output: &str,
    process_rows: Vec<GpuProcessRow>,
    identity: &HashMap<u32, (Option<String>, Option<String>)>,
) -> Result<Vec<GpuDetailMetric>, String> {
    let mut metrics = Vec::new();
    for line in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let fields = line.split(',').map(str::trim).collect::<Vec<_>>();
        if fields.len() < 18 {
            return Err(format!(
                "unexpected nvidia-smi GPU detail column count {}",
                fields.len()
            ));
        }
        metrics.push(GpuDetailMetric {
            index: fields[0]
                .parse()
                .map_err(|_| "invalid GPU index".to_string())?,
            name: fields[1].to_string(),
            uuid: fields[2].to_string(),
            driver_version: optional_string(fields[3]),
            power_draw_w: parse_optional_f64(fields[4]),
            power_limit_w: parse_optional_f64(fields[5]),
            temperature_c: parse_optional_f64(fields[6]),
            gpu_util_percent: parse_optional_f64(fields[7]),
            memory_util_percent: parse_optional_f64(fields[8]),
            memory_total_mi_b: parse_optional_u64(fields[9]),
            memory_used_mi_b: parse_optional_u64(fields[10]),
            memory_free_mi_b: parse_optional_u64(fields[11]),
            fan_speed_percent: parse_optional_f64(fields[12]),
            graphics_clock_m_hz: parse_optional_f64(fields[13]),
            memory_clock_m_hz: parse_optional_f64(fields[14]),
            pci_bus_id: optional_string(fields[15]),
            persistence_mode: optional_string(fields[16]),
            mig_mode: optional_string(fields[17]),
            processes: Vec::new(),
        });
    }
    if metrics.is_empty() {
        return Err("nvidia-smi returned no GPU detail rows".to_string());
    }

    let index_by_uuid = metrics
        .iter()
        .map(|metric| (metric.uuid.clone(), metric.index))
        .collect::<HashMap<_, _>>();
    for process in process_rows {
        let Some(uuid) = process.gpu_uuid.clone() else {
            continue;
        };
        let Some(index) = index_by_uuid.get(&uuid).copied() else {
            continue;
        };
        let identity = identity.get(&process.pid);
        let metric = GpuProcessMetric {
            gpu_index: Some(index),
            gpu_uuid: Some(uuid.clone()),
            pid: process.pid,
            user: identity.and_then(|value| value.0.clone()),
            process_name: process.process_name,
            command: identity.and_then(|value| value.1.clone()),
            used_memory_mi_b: process.used_memory_mi_b,
        };
        if let Some(gpu) = metrics.iter_mut().find(|gpu| gpu.uuid == uuid) {
            gpu.processes.push(metric);
        }
    }
    for gpu in &mut metrics {
        gpu.processes
            .sort_by_key(|process| std::cmp::Reverse(process.used_memory_mi_b));
    }
    Ok(metrics)
}

fn parse_ps_identity_output(output: &str) -> HashMap<u32, (Option<String>, Option<String>)> {
    output
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let pid = parts.next()?.parse::<u32>().ok()?;
            let user = parts.next().and_then(optional_string);
            let command = optional_string(&parts.collect::<Vec<_>>().join(" "));
            Some((pid, (user, command)))
        })
        .collect()
}

#[derive(Debug, Clone, Copy)]
struct CpuSample {
    idle: u64,
    total: u64,
}

fn parse_cpu_samples(output: &str) -> Result<HashMap<String, CpuSample>, String> {
    let mut samples = HashMap::new();
    for line in output.lines().filter(|line| line.starts_with("cpu")) {
        let mut fields = line.split_whitespace();
        let Some(name) = fields.next() else { continue };
        if name != "cpu"
            && !name
                .trim_start_matches("cpu")
                .chars()
                .all(|value| value.is_ascii_digit())
        {
            continue;
        }
        let values = fields
            .filter_map(|value| value.parse::<u64>().ok())
            .collect::<Vec<_>>();
        if values.len() < 4 {
            continue;
        }
        let idle = values.get(3).copied().unwrap_or(0) + values.get(4).copied().unwrap_or(0);
        samples.insert(
            name.to_string(),
            CpuSample {
                idle,
                total: values.iter().sum(),
            },
        );
    }
    if !samples.contains_key("cpu") {
        return Err("missing aggregate CPU sample".to_string());
    }
    Ok(samples)
}

fn calculate_sample_usage(
    previous: Option<&CpuSample>,
    current: Option<&CpuSample>,
) -> Option<f64> {
    let previous = previous?;
    let current = current?;
    let total = current.total.checked_sub(previous.total)?;
    let idle = current.idle.checked_sub(previous.idle)?;
    (total > 0)
        .then_some(((total.saturating_sub(idle)) as f64 / total as f64 * 100.0).clamp(0.0, 100.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cpu_process_output() {
        let rows =
            parse_cpu_processes("42 root 87.5 1.2 01:20 python3\n7 user 12.0 0.5 00:10 worker\n");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].pid, 42);
        assert_eq!(rows[0].cpu_percent, Some(87.5));
        assert_eq!(rows[0].command.as_deref(), Some("python3"));
    }

    #[test]
    fn parses_memory_process_output() {
        let rows = parse_memory_processes("99 alice 1048576 2097152 18.5 python\n");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].rss_bytes, Some(1_073_741_824));
        assert_eq!(rows[0].vsz_bytes, Some(2_147_483_648));
        assert_eq!(rows[0].memory_percent, Some(18.5));
    }

    #[test]
    fn parses_gpu_detail_output_with_optional_fields() {
        let output = "0, NVIDIA H100, GPU-abc, 550.54, 510, 600, 69, 82, 55, 98304, 43008, 55296, N/A, 1800, 1593, 00000000:17:00.0, Enabled, Not Supported\n";
        let metrics = parse_gpu_detail_output(output, Vec::new(), &HashMap::new()).unwrap();
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].index, 0);
        assert_eq!(metrics[0].fan_speed_percent, None);
        assert_eq!(metrics[0].mig_mode, None);
    }

    #[test]
    fn joins_gpu_processes_with_linux_process_identity() {
        let gpu_output = "0, NVIDIA H100, GPU-abc, 550.54, 510, 600, 69, 82, 55, 98304, 43008, 55296, 40, 1800, 1593, 00000000:17:00.0, Enabled, Disabled\n";
        let processes = parse_gpu_process_output("GPU-abc, 4242, python, 4096\n").unwrap();
        let identity = parse_ps_identity_output("4242 alice python train.py --epochs 10\n");
        let metrics = parse_gpu_detail_output(gpu_output, processes, &identity).unwrap();
        assert_eq!(metrics[0].processes.len(), 1);
        assert_eq!(metrics[0].processes[0].user.as_deref(), Some("alice"));
        assert_eq!(
            metrics[0].processes[0].command.as_deref(),
            Some("python train.py --epochs 10")
        );
    }

    #[test]
    fn joins_gpu_processes_with_windows_process_identity() {
        let gpu_output = "0, NVIDIA RTX 4090, GPU-win, 560.94, 300, 450, 60, 90, 40, 24564, 12000, 12564, 55, 2520, 10501, 00000000:01:00.0, N/A, N/A\n";
        let processes =
            parse_gpu_process_output("GPU-win, 4242, C:\\Python\\python.exe, 8192\n").unwrap();
        let identity = parse_windows_process_identity(
            "{\"ProcessId\":4242,\"Name\":\"python.exe\",\"CommandLine\":\"python train.py\"}",
        );
        let metrics = parse_gpu_detail_output(gpu_output, processes, &identity).unwrap();
        assert_eq!(metrics[0].processes.len(), 1);
        // Win32_Process exposes no owner without elevation.
        assert_eq!(metrics[0].processes[0].user, None);
        assert_eq!(
            metrics[0].processes[0].command.as_deref(),
            Some("python train.py")
        );
        assert_eq!(metrics[0].processes[0].used_memory_mi_b, Some(8192));
    }

    #[test]
    fn parses_windows_cpu_detail_output_with_cores_and_processes() {
        let output = concat!(
            "__CPUINFO__\n{\"Name\":\"Intel(R) Core(TM) i7-13700K\",\"NumberOfLogicalProcessors\":24,\"CurrentClockSpeed\":3400}\n",
            "__UPTIME__\n86400\n",
            "__TOTALMEM__\n1048576\n",
            "__CPUTIMES_1__\n[{\"Name\":\"_Total\",\"PercentProcessorTime\":1000,\"Timestamp_Sys100NS\":10000000},{\"Name\":\"0\",\"PercentProcessorTime\":1000,\"Timestamp_Sys100NS\":10000000},{\"Name\":\"1\",\"PercentProcessorTime\":1000,\"Timestamp_Sys100NS\":10000000}]\n",
            "__PROC_1__\n[{\"Id\":4242,\"Name\":\"python\",\"CpuSeconds\":10.0,\"WorkingSet64\":536870912},{\"Id\":7,\"Name\":\"idleapp\",\"CpuSeconds\":5.0,\"WorkingSet64\":1048576}]\n",
            "__CPUTIMES_2__\n[{\"Name\":\"_Total\",\"PercentProcessorTime\":3000000,\"Timestamp_Sys100NS\":20000000},{\"Name\":\"0\",\"PercentProcessorTime\":2000000,\"Timestamp_Sys100NS\":20000000},{\"Name\":\"1\",\"PercentProcessorTime\":8000000,\"Timestamp_Sys100NS\":20000000}]\n",
            "__PROC_2__\n[{\"Id\":4242,\"Name\":\"python\",\"CpuSeconds\":11.0,\"WorkingSet64\":536870912},{\"Id\":7,\"Name\":\"idleapp\",\"CpuSeconds\":5.0,\"WorkingSet64\":1048576}]\n",
        );
        let detail = parse_windows_cpu_detail_output(output).unwrap();
        assert_eq!(
            detail.model_name.as_deref(),
            Some("Intel(R) Core(TM) i7-13700K")
        );
        assert_eq!(detail.total_cores, Some(24));
        assert_eq!(detail.uptime_seconds, Some(86400.0));
        assert_eq!(detail.load_avg1, None);
        // Δidle ≈ 3e6−1e3, Δtotal = 1e7 → ~70% busy.
        assert!((detail.usage_percent.unwrap() - 70.01).abs() < 0.1);
        assert_eq!(detail.logical_core_usage_percent.len(), 2);
        assert!((detail.logical_core_usage_percent[0].unwrap() - 80.01).abs() < 0.1);
        assert!((detail.logical_core_usage_percent[1].unwrap() - 20.01).abs() < 0.1);
        // Δcpu 1 s over a 1 s window → 100%, sorted first.
        assert_eq!(detail.top_processes[0].pid, 4242);
        assert!((detail.top_processes[0].cpu_percent.unwrap() - 100.0).abs() < 0.5);
        assert_eq!(detail.top_processes[0].user, None);
        // WorkingSet 512 MiB of 1 GiB total → 50%.
        assert!((detail.top_processes[0].memory_percent.unwrap() - 50.0).abs() < 0.1);
        assert_eq!(detail.top_processes[1].cpu_percent, Some(0.0));
    }

    #[test]
    fn parses_windows_memory_detail_output_sorted_by_working_set() {
        let output = concat!(
            "__MEMORY__\n{\"TotalVisibleMemorySize\":1048576,\"FreePhysicalMemory\":524288}\n",
            "__PAGEFILE__\n{\"AllocatedBaseSize\":4096,\"CurrentUsage\":100}\n",
            "__PROCESSES__\n[{\"Id\":1,\"Name\":\"small\",\"WorkingSet64\":1048576,\"VirtualMemorySize64\":2097152},{\"Id\":2,\"Name\":\"big\",\"WorkingSet64\":536870912,\"VirtualMemorySize64\":1073741824}]\n",
        );
        let detail = parse_windows_memory_detail_output(output).unwrap();
        assert_eq!(detail.total_mi_b, Some(1024));
        assert_eq!(detail.swap_total_mi_b, Some(4096));
        assert_eq!(detail.buffers_mi_b, None);
        assert_eq!(detail.top_processes.len(), 2);
        assert_eq!(detail.top_processes[0].command.as_deref(), Some("big"));
        assert_eq!(detail.top_processes[0].rss_bytes, Some(536870912));
        assert_eq!(detail.top_processes[0].vsz_bytes, Some(1073741824));
        // 512 MiB of 1 GiB → 50%.
        assert!((detail.top_processes[0].memory_percent.unwrap() - 50.0).abs() < 0.1);
    }
}
