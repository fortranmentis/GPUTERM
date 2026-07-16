use crate::ssh::parse_util::{
    kib_to_mib, optional_string, parse_average_clock, parse_cpu_model, parse_first_u64,
    parse_loadavg, parse_lscpu_value, parse_meminfo_values, parse_optional_f64,
    parse_optional_u64, required_section, split_sections,
};
use crate::ssh::gpu_monitor::{parse_gpu_probe, GpuMetric};
use crate::ssh::session::{target_for_active_session, with_ops_session, AppState};
use crate::ssh::system_monitor::{collect_gpu_metrics, run_remote_command};
use crate::ssh::gpu_monitor::GPU_PROBE_COMMAND;
use serde::Serialize;
use ssh2::Session;
use std::collections::HashMap;
use std::sync::Arc;
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
    let target = target_for_active_session(&state, &session_id)?;
    let ops = Arc::clone(&state.ops_sessions);
    tauri::async_runtime::spawn_blocking(move || {
        let mut details = ResourceDetails::default();
        let result = with_ops_session(&ops, &target, DETAIL_TIMEOUT_MS, |session| {
            collect_details(session, resource_type)
        });
        match result {
            Ok(ResourceDetailValue::Cpu(metric)) => details.cpu = Some(metric),
            Ok(ResourceDetailValue::Memory(metric)) => details.memory = Some(metric),
            Ok(ResourceDetailValue::Gpu(metrics)) => details.gpus = metrics,
            Err(error) => details.set_error(resource_type, format!("Metrics unavailable: {}", error)),
        }
        details
    })
    .await
    .map_err(|error| format!("Resource detail task failed: {}", error))
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

fn collect_details(session: &Session, resource_type: ResourceType) -> Result<ResourceDetailValue, String> {
    match resource_type {
        ResourceType::Cpu => run_remote_command(session, CPU_DETAIL_COMMAND)
            .and_then(|output| parse_cpu_detail_output(&output))
            .map(ResourceDetailValue::Cpu),
        ResourceType::Memory => run_remote_command(session, MEMORY_DETAIL_COMMAND)
            .and_then(|output| parse_memory_detail_output(&output))
            .map(ResourceDetailValue::Memory),
        ResourceType::Gpu => collect_gpu_details(session).map(ResourceDetailValue::Gpu),
    }
}

fn collect_gpu_details(session: &Session) -> Result<Vec<GpuDetailMetric>, String> {
    // NVIDIA keeps its rich per-process detail path; AMD/Intel reuse the
    // telemetry collectors and map into the same shape with nulls for the
    // fields their tools do not expose.
    let has_nvidia = run_remote_command(session, GPU_PROBE_COMMAND)
        .map(|output| parse_gpu_probe(&output).nvidia)
        .unwrap_or(true);
    if !has_nvidia {
        let mut probe = None;
        let metrics = collect_gpu_metrics(session, &mut probe)?;
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
    parse_gpu_detail_output(&gpu_output, process_rows, &ps_output)
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
    logical_names.sort_by_key(|name| name.trim_start_matches("cpu").parse::<u32>().unwrap_or(u32::MAX));
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
        avg_clock_ghz: parse_average_clock(cpuinfo)
            .or_else(|| parse_lscpu_value(lscpu, "CPU MHz").and_then(|value| value.parse::<f64>().ok()).map(|mhz| mhz / 1000.0)),
        uptime_seconds: sections.get("UPTIME").and_then(|value| value.split_whitespace().next()).and_then(|value| value.parse().ok()),
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
    for line in output.lines().map(str::trim).filter(|line| !line.is_empty()) {
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
    ps_output: &str,
) -> Result<Vec<GpuDetailMetric>, String> {
    let mut metrics = Vec::new();
    for line in output.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let fields = line.split(',').map(str::trim).collect::<Vec<_>>();
        if fields.len() < 18 {
            return Err(format!("unexpected nvidia-smi GPU detail column count {}", fields.len()));
        }
        metrics.push(GpuDetailMetric {
            index: fields[0].parse().map_err(|_| "invalid GPU index".to_string())?,
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

    let ps = parse_ps_identity_output(ps_output);
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
        let identity = ps.get(&process.pid);
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
        gpu.processes.sort_by_key(|process| std::cmp::Reverse(process.used_memory_mi_b));
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
        if name != "cpu" && !name.trim_start_matches("cpu").chars().all(|value| value.is_ascii_digit()) {
            continue;
        }
        let values = fields.filter_map(|value| value.parse::<u64>().ok()).collect::<Vec<_>>();
        if values.len() < 4 {
            continue;
        }
        let idle = values.get(3).copied().unwrap_or(0) + values.get(4).copied().unwrap_or(0);
        samples.insert(name.to_string(), CpuSample { idle, total: values.iter().sum() });
    }
    if !samples.contains_key("cpu") {
        return Err("missing aggregate CPU sample".to_string());
    }
    Ok(samples)
}

fn calculate_sample_usage(previous: Option<&CpuSample>, current: Option<&CpuSample>) -> Option<f64> {
    let previous = previous?;
    let current = current?;
    let total = current.total.checked_sub(previous.total)?;
    let idle = current.idle.checked_sub(previous.idle)?;
    (total > 0).then_some(((total.saturating_sub(idle)) as f64 / total as f64 * 100.0).clamp(0.0, 100.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cpu_process_output() {
        let rows = parse_cpu_processes("42 root 87.5 1.2 01:20 python3\n7 user 12.0 0.5 00:10 worker\n");
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
        let metrics = parse_gpu_detail_output(output, Vec::new(), "").unwrap();
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].index, 0);
        assert_eq!(metrics[0].fan_speed_percent, None);
        assert_eq!(metrics[0].mig_mode, None);
    }

    #[test]
    fn joins_gpu_processes_with_linux_process_identity() {
        let gpu_output = "0, NVIDIA H100, GPU-abc, 550.54, 510, 600, 69, 82, 55, 98304, 43008, 55296, 40, 1800, 1593, 00000000:17:00.0, Enabled, Disabled\n";
        let processes = parse_gpu_process_output("GPU-abc, 4242, python, 4096\n").unwrap();
        let metrics = parse_gpu_detail_output(gpu_output, processes, "4242 alice python train.py --epochs 10\n").unwrap();
        assert_eq!(metrics[0].processes.len(), 1);
        assert_eq!(metrics[0].processes[0].user.as_deref(), Some("alice"));
        assert_eq!(metrics[0].processes[0].command.as_deref(), Some("python train.py --epochs 10"));
    }
}
