//! Windows telemetry collectors, executed remotely as PowerShell 5.1 scripts
//! (`powershell.exe -EncodedCommand`): CIM/WMI classes for CPU, memory, and
//! disks (class and property names are locale-independent, unlike Get-Counter
//! paths), `quser` for interactive sessions, and the WDDM GPU performance
//! counter classes for utilization/VRAM of adapters without a vendor CLI.

use crate::ssh::gpu_monitor::{GpuMetric, GpuProbe, WindowsGpuAdapter};
use crate::ssh::parse_util::{kib_to_mib, split_sections};
use crate::ssh::system_monitor::{
    calculate_cpu_usage, CpuMetric, CpuStatSample, DiskMetric, MemoryMetric, RemoteUserSession,
};
use serde_json::Value;
use std::collections::HashMap;

const BYTES_PER_MIB: u64 = 1024 * 1024;

/// One batched script per poll tick: PowerShell start-up costs 0.5–2 s, so
/// hostname, CPU, memory, disks, and users share a single invocation split by
/// `__SECTION__` markers. `quser` exits 1 when nobody is logged in (and is
/// absent on Home editions), hence the trailing `exit 0`.
pub(crate) const WINDOWS_TELEMETRY_COMMAND: &str = r#"$ErrorActionPreference='SilentlyContinue'
Write-Output '__HOSTNAME__'
Write-Output $env:COMPUTERNAME
Write-Output '__CPUINFO__'
Get-CimInstance Win32_Processor | Select-Object Name,NumberOfCores,NumberOfLogicalProcessors,MaxClockSpeed,CurrentClockSpeed | ConvertTo-Json -Compress
Write-Output '__CPUTIMES__'
Get-CimInstance Win32_PerfRawData_PerfOS_Processor -Filter "Name='_Total'" | Select-Object Name,PercentProcessorTime,Timestamp_Sys100NS | ConvertTo-Json -Compress
Write-Output '__MEMORY__'
Get-CimInstance Win32_OperatingSystem | Select-Object TotalVisibleMemorySize,FreePhysicalMemory | ConvertTo-Json -Compress
Write-Output '__PAGEFILE__'
Get-CimInstance Win32_PageFileUsage | Select-Object AllocatedBaseSize,CurrentUsage | ConvertTo-Json -Compress
Write-Output '__DISK__'
Get-CimInstance Win32_LogicalDisk -Filter 'DriveType=3' | Select-Object DeviceID,FileSystem,VolumeName,Size,FreeSpace | ConvertTo-Json -Compress
Write-Output '__USERS__'
quser 2>$null
exit 0"#;

/// CPU detail: two counter/process samples 500 ms apart give a true usage
/// delta per logical core and per process (Get-Process only exposes
/// cumulative CPU seconds). Uptime is computed to plain seconds in PowerShell
/// to dodge PS 5.1's `\/Date(...)\/` JSON DateTime serialization.
pub(crate) const WINDOWS_CPU_DETAIL_COMMAND: &str = r#"$ErrorActionPreference='SilentlyContinue'
Write-Output '__CPUINFO__'
Get-CimInstance Win32_Processor | Select-Object Name,NumberOfCores,NumberOfLogicalProcessors,MaxClockSpeed,CurrentClockSpeed | ConvertTo-Json -Compress
Write-Output '__UPTIME__'
[int64]((Get-Date) - (Get-CimInstance Win32_OperatingSystem).LastBootUpTime).TotalSeconds
Write-Output '__TOTALMEM__'
(Get-CimInstance Win32_OperatingSystem).TotalVisibleMemorySize
Write-Output '__CPUTIMES_1__'
Get-CimInstance Win32_PerfRawData_PerfOS_Processor | Select-Object Name,PercentProcessorTime,Timestamp_Sys100NS | ConvertTo-Json -Compress
Write-Output '__PROC_1__'
Get-Process | Select-Object Id,Name,@{n='CpuSeconds';e={$_.TotalProcessorTime.TotalSeconds}},WorkingSet64 | ConvertTo-Json -Compress
Start-Sleep -Milliseconds 500
Write-Output '__CPUTIMES_2__'
Get-CimInstance Win32_PerfRawData_PerfOS_Processor | Select-Object Name,PercentProcessorTime,Timestamp_Sys100NS | ConvertTo-Json -Compress
Write-Output '__PROC_2__'
Get-Process | Select-Object Id,Name,@{n='CpuSeconds';e={$_.TotalProcessorTime.TotalSeconds}},WorkingSet64 | ConvertTo-Json -Compress
exit 0"#;

pub(crate) const WINDOWS_MEMORY_DETAIL_COMMAND: &str = r#"$ErrorActionPreference='SilentlyContinue'
Write-Output '__MEMORY__'
Get-CimInstance Win32_OperatingSystem | Select-Object TotalVisibleMemorySize,FreePhysicalMemory | ConvertTo-Json -Compress
Write-Output '__PAGEFILE__'
Get-CimInstance Win32_PageFileUsage | Select-Object AllocatedBaseSize,CurrentUsage | ConvertTo-Json -Compress
Write-Output '__PROCESSES__'
Get-Process | Sort-Object WorkingSet64 -Descending | Select-Object -First 15 Id,Name,WorkingSet64,VirtualMemorySize64 | ConvertTo-Json -Compress
exit 0"#;

/// nvidia-smi ships in System32 with the NVIDIA driver, so a PATH lookup
/// suffices; Win32_VideoController covers AMD/Intel adapters that have no
/// vendor CLI on Windows. The DirectX registry key supplies each adapter's
/// LUID (which perf counter instance names embed — exact attribution on
/// hybrid iGPU+dGPU hosts) and its true dedicated VRAM size.
pub(crate) const WINDOWS_GPU_PROBE_COMMAND: &str = r#"$ErrorActionPreference='SilentlyContinue'
Write-Output '__NVSMI__'
if (Get-Command nvidia-smi.exe) { Write-Output 'nvidia-smi' }
Write-Output '__ADAPTERS__'
Get-CimInstance Win32_VideoController | Select-Object Name,AdapterCompatibility,DriverVersion | ConvertTo-Json -Compress
Write-Output '__DXADAPTERS__'
Get-ChildItem 'HKLM:\SOFTWARE\Microsoft\DirectX' | ForEach-Object { Get-ItemProperty $_.PSPath } | Where-Object { $_.Description -and $_.AdapterLuid } | Select-Object Description,AdapterLuid,DedicatedVideoMemory | ConvertTo-Json -Compress
exit 0"#;

/// Raw GPU engine counters sampled twice: the formatted variants commonly
/// read 0 on a one-shot ad-hoc query, so the busy delta is computed in Rust.
/// Instance names (`pid_…_phys_0_engtype_3D`) are machine-generated and not
/// localized.
pub(crate) const WINDOWS_GPU_COUNTERS_COMMAND: &str = r#"$ErrorActionPreference='SilentlyContinue'
Write-Output '__ENG1__'
Get-CimInstance Win32_PerfRawData_GPUPerformanceCounters_GPUEngine | Select-Object Name,UtilizationPercentage,Timestamp_Sys100NS | ConvertTo-Json -Compress
Start-Sleep -Milliseconds 500
Write-Output '__ENG2__'
Get-CimInstance Win32_PerfRawData_GPUPerformanceCounters_GPUEngine | Select-Object Name,UtilizationPercentage,Timestamp_Sys100NS | ConvertTo-Json -Compress
Write-Output '__ADAPTERMEM__'
Get-CimInstance Win32_PerfRawData_GPUPerformanceCounters_GPUAdapterMemory | Select-Object Name,DedicatedUsage,SharedUsage | ConvertTo-Json -Compress
exit 0"#;

/// PowerShell 5.1's `ConvertTo-Json` serializes a single pipeline object as a
/// bare object rather than a one-element array; normalize both shapes.
pub(crate) fn json_array(value: Value) -> Vec<Value> {
    match value {
        Value::Array(items) => items,
        Value::Null => Vec::new(),
        other => vec![other],
    }
}

fn parse_json_rows(raw: &str) -> Vec<Value> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    serde_json::from_str::<Value>(trimmed)
        .map(json_array)
        .unwrap_or_default()
}

fn json_section_rows(sections: &HashMap<String, String>, name: &str) -> Vec<Value> {
    parse_json_rows(sections.get(name).map(String::as_str).unwrap_or(""))
}

fn value_str(row: &Value, key: &str) -> Option<String> {
    row.get(key)
        .and_then(Value::as_str)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn value_u64(row: &Value, key: &str) -> Option<u64> {
    match row.get(key)? {
        Value::Number(number) => number
            .as_u64()
            // Registry QWORDs (e.g. AdapterLuid) arrive as i64.
            .or_else(|| number.as_i64().map(|value| value as u64))
            .or_else(|| number.as_f64().filter(|value| *value >= 0.0).map(|value| value.round() as u64)),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

fn value_f64(row: &Value, key: &str) -> Option<f64> {
    match row.get(key)? {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub(crate) struct WindowsCpuInfo {
    pub(crate) model_name: Option<String>,
    pub(crate) logical_cores: Option<u64>,
    pub(crate) avg_clock_ghz: Option<f64>,
}

pub(crate) fn parse_windows_cpu_info(section: &str) -> WindowsCpuInfo {
    let rows = parse_json_rows(section);
    let names: Vec<String> = rows
        .iter()
        .filter_map(|row| value_str(row, "Name"))
        .collect();
    let model_name = match names.as_slice() {
        [] => None,
        [only] => Some(only.clone()),
        all if all.iter().all(|name| name == &all[0]) => {
            Some(format!("{} × {}", all[0], all.len()))
        }
        all => Some(all.join(", ")),
    };
    let logical: u64 = rows
        .iter()
        .filter_map(|row| value_u64(row, "NumberOfLogicalProcessors"))
        .sum();
    let clocks: Vec<f64> = rows
        .iter()
        .filter_map(|row| {
            value_f64(row, "CurrentClockSpeed")
                .filter(|mhz| *mhz > 0.0)
                .or_else(|| value_f64(row, "MaxClockSpeed"))
        })
        .collect();
    let avg_clock_ghz =
        (!clocks.is_empty()).then(|| clocks.iter().sum::<f64>() / clocks.len() as f64 / 1000.0);
    WindowsCpuInfo {
        model_name,
        logical_cores: (logical > 0).then_some(logical),
        avg_clock_ghz,
    }
}

/// Parses a `Win32_PerfRawData_PerfOS_Processor` JSON section into per-instance
/// samples. Raw `PercentProcessorTime` accumulates *idle* 100 ns ticks
/// (PERF_100NSEC_TIMER_INV) and `Timestamp_Sys100NS` is the wall clock in the
/// same unit, so the pair maps directly onto the existing `CpuStatSample`
/// two-poll delta math.
pub(crate) fn parse_windows_cpu_times(section: &str) -> HashMap<String, CpuStatSample> {
    parse_json_rows(section)
        .iter()
        .filter_map(|row| {
            let name = value_str(row, "Name")?;
            let idle = value_u64(row, "PercentProcessorTime")?;
            let total = value_u64(row, "Timestamp_Sys100NS")?;
            Some((name, CpuStatSample { idle, total }))
        })
        .collect()
}

pub(crate) fn parse_windows_cpu_output(
    sections: &HashMap<String, String>,
    previous_cpu: &mut Option<CpuStatSample>,
) -> Result<CpuMetric, String> {
    let info = parse_windows_cpu_info(sections.get("CPUINFO").map(String::as_str).unwrap_or(""));
    let times =
        parse_windows_cpu_times(sections.get("CPUTIMES").map(String::as_str).unwrap_or(""));
    let current = times.get("_Total").copied();
    if info.model_name.is_none() && info.logical_cores.is_none() && current.is_none() {
        return Err("Windows CPU telemetry unavailable (WMI queries produced no output)".to_string());
    }
    let usage_percent = current.and_then(|current| {
        let usage = previous_cpu.and_then(|previous| calculate_cpu_usage(previous, current));
        *previous_cpu = Some(current);
        usage
    });

    Ok(CpuMetric {
        model_name: info.model_name,
        usage_percent,
        // Load averages do not exist on Windows; the UI renders n/a.
        load_avg1: None,
        load_avg5: None,
        load_avg15: None,
        total_cores: info.logical_cores,
        online_cores: info.logical_cores,
        avg_clock_ghz: info.avg_clock_ghz,
    })
}

pub(crate) fn parse_windows_memory_output(
    sections: &HashMap<String, String>,
) -> Result<MemoryMetric, String> {
    let rows = json_section_rows(sections, "MEMORY");
    let os = rows.first().ok_or_else(|| {
        "Windows memory telemetry unavailable (Win32_OperatingSystem query produced no output)"
            .to_string()
    })?;
    let total_kib = value_u64(os, "TotalVisibleMemorySize")
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            "Windows memory telemetry unavailable (TotalVisibleMemorySize missing)".to_string()
        })?;
    // FreePhysicalMemory includes standby pages, i.e. Task Manager's
    // "available"; a strict "free" figure is not exposed by this class.
    let available_kib = value_u64(os, "FreePhysicalMemory").unwrap_or(0);
    let used_kib = total_kib.saturating_sub(available_kib);

    // Swap = page files; both columns are already MiB. No rows means the page
    // file is disabled, which is a real zero rather than missing data.
    let pagefiles = json_section_rows(sections, "PAGEFILE");
    let swap_total_mib: u64 = pagefiles
        .iter()
        .filter_map(|row| value_u64(row, "AllocatedBaseSize"))
        .sum();
    let swap_used_mib: u64 = pagefiles
        .iter()
        .filter_map(|row| value_u64(row, "CurrentUsage"))
        .sum();

    Ok(MemoryMetric {
        total_mi_b: Some(kib_to_mib(total_kib)),
        used_mi_b: Some(kib_to_mib(used_kib)),
        available_mi_b: Some(kib_to_mib(available_kib)),
        free_mi_b: None,
        usage_percent: Some((used_kib as f64 / total_kib as f64) * 100.0),
        swap_total_mi_b: Some(swap_total_mib),
        swap_used_mi_b: Some(swap_used_mib),
        swap_free_mi_b: Some(swap_total_mib.saturating_sub(swap_used_mib)),
    })
}

pub(crate) fn parse_windows_disk_output(
    sections: &HashMap<String, String>,
) -> Result<Vec<DiskMetric>, String> {
    let mut disks = Vec::new();
    for row in json_section_rows(sections, "DISK") {
        let Some(device_id) = value_str(&row, "DeviceID") else {
            continue;
        };
        let total = value_u64(&row, "Size");
        let available = value_u64(&row, "FreeSpace");
        let used = match (total, available) {
            (Some(total), Some(available)) => Some(total.saturating_sub(available)),
            _ => None,
        };
        let usage_percent = match (total, used) {
            (Some(total), Some(used)) if total > 0 => {
                Some((used as f64 / total as f64) * 100.0)
            }
            _ => None,
        };
        disks.push(DiskMetric {
            filesystem: device_id.clone(),
            fs_type: value_str(&row, "FileSystem"),
            mount_point: format!("{}\\", device_id),
            total_bytes: total,
            used_bytes: used,
            available_bytes: available,
            usage_percent,
        });
    }
    if disks.is_empty() {
        return Err(
            "Windows disk telemetry unavailable (Win32_LogicalDisk returned no fixed drives)"
                .to_string(),
        );
    }
    Ok(disks)
}

/// Parses `quser` positionally because the header row is localized. A data row
/// carries the numeric session ID in column 2 (active, SESSIONNAME present) or
/// column 1 (disconnected, SESSIONNAME blank); anything else — headers,
/// localized "No User exists" text — yields no row. The leading `>` marks the
/// caller's own session.
pub(crate) fn parse_quser_output(output: &str) -> Vec<RemoteUserSession> {
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let line = line.strip_prefix('>').unwrap_or(line);
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() >= 5 && fields[1].parse::<u32>().is_ok() {
                return Some(RemoteUserSession {
                    user: fields[0].to_string(),
                    tty: "-".to_string(),
                    login_time: fields[4..].join(" "),
                    from: None,
                });
            }
            if fields.len() >= 6 && fields[2].parse::<u32>().is_ok() {
                return Some(RemoteUserSession {
                    user: fields[0].to_string(),
                    tty: fields[1].to_string(),
                    login_time: fields[5..].join(" "),
                    from: None,
                });
            }
            None
        })
        .collect()
}

pub(crate) fn parse_windows_gpu_probe(output: &str) -> GpuProbe {
    let sections = split_sections(output);
    let mut probe = GpuProbe {
        nvidia: sections
            .get("NVSMI")
            .map(|section| section.lines().any(|line| line.trim() == "nvidia-smi"))
            .unwrap_or(false),
        ..GpuProbe::default()
    };
    let dx_rows = json_section_rows(&sections, "DXADAPTERS");
    probe.windows_adapters = json_section_rows(&sections, "ADAPTERS")
        .iter()
        .filter_map(|row| {
            let name = value_str(row, "Name")?;
            let compatibility = value_str(row, "AdapterCompatibility").unwrap_or_default();
            let vendor = windows_gpu_vendor(&name, &compatibility)?;
            // Join the DirectX registry rows by adapter description; stale
            // entries from previous boots share the description, so keep
            // every LUID — only the live one shows up in the counters.
            let matching: Vec<&Value> = dx_rows
                .iter()
                .filter(|dx| {
                    value_str(dx, "Description")
                        .map(|description| description.eq_ignore_ascii_case(&name))
                        .unwrap_or(false)
                })
                .collect();
            Some(WindowsGpuAdapter {
                luids: matching
                    .iter()
                    .filter_map(|dx| value_u64(dx, "AdapterLuid"))
                    .collect(),
                memory_total_bytes: matching
                    .iter()
                    .filter_map(|dx| value_u64(dx, "DedicatedVideoMemory"))
                    .filter(|bytes| *bytes > 0)
                    .max(),
                name,
                vendor: vendor.to_string(),
                driver_version: value_str(row, "DriverVersion").unwrap_or_default(),
            })
        })
        .collect();
    probe
}

/// Maps a Win32_VideoController row to a GPU vendor tag; virtual and software
/// adapters (Microsoft Basic Display, Remote Display, DisplayLink, ...) return
/// None and are excluded from telemetry.
fn windows_gpu_vendor(name: &str, compatibility: &str) -> Option<&'static str> {
    let haystack = format!("{} {}", name, compatibility).to_lowercase();
    if haystack.contains("nvidia") {
        Some("nvidia")
    } else if haystack.contains("advanced micro devices")
        || haystack.contains("radeon")
        || haystack.starts_with("amd")
        || haystack.contains(" amd")
    {
        Some("amd")
    } else if haystack.contains("intel") {
        Some("intel")
    } else {
        None
    }
}

/// Extracts the value following `{key}_` in a perf counter instance name,
/// e.g. `phys` in `pid_1234_luid_0x0_0xC64A_phys_0_engtype_3D`.
fn instance_token<'a>(name: &'a str, key: &str) -> Option<&'a str> {
    let marker = format!("{}_", key);
    let start = name.find(&marker)? + marker.len();
    let rest = &name[start..];
    Some(&rest[..rest.find('_').unwrap_or(rest.len())])
}

fn instance_phys(name: &str) -> Option<u32> {
    instance_token(name, "phys").and_then(|value| value.parse().ok())
}

/// Reassembles the adapter LUID embedded in a counter instance name
/// (`luid_0xHIGH_0xLOW`) into the u64 the DirectX registry reports.
fn instance_luid(name: &str) -> Option<u64> {
    let start = name.find("luid_")? + "luid_".len();
    let mut parts = name[start..].split('_');
    let high = u64::from_str_radix(parts.next()?.strip_prefix("0x")?, 16).ok()?;
    let low = u64::from_str_radix(parts.next()?.strip_prefix("0x")?, 16).ok()?;
    Some((high << 32) | low)
}

/// The engine type is the trailing token and may itself contain underscores.
fn instance_engine_type(name: &str) -> Option<&str> {
    let marker = "engtype_";
    name.find(marker).map(|start| &name[start + marker.len()..])
}

fn parse_engine_samples(section: &str) -> HashMap<String, (u64, u64)> {
    parse_json_rows(section)
        .iter()
        .filter_map(|row| {
            let name = value_str(row, "Name")?;
            let busy = value_u64(row, "UtilizationPercentage")?;
            let timestamp = value_u64(row, "Timestamp_Sys100NS")?;
            Some((name, (busy, timestamp)))
        })
        .collect()
}

/// Computes per-GPU utilization/VRAM from two raw GPU Engine counter samples.
/// Per physical GPU the per-engine-type busy sums are reduced with `max`,
/// matching Task Manager's overall GPU % (a plain sum over-counts concurrent
/// engines; 3D-only misses video/compute loads). Counter instances are
/// attributed to adapters by the LUID embedded in their names (exact, so
/// hybrid iGPU+dGPU hosts report both cards correctly); `phys_N` → Nth
/// adapter is the fallback when the DirectX registry gave no LUIDs.
pub(crate) fn parse_windows_gpu_counters(
    output: &str,
    adapters: &[WindowsGpuAdapter],
    nvidia_covered: bool,
    next_index: u32,
) -> Result<Vec<GpuMetric>, String> {
    let sections = split_sections(output);
    let first = parse_engine_samples(sections.get("ENG1").map(String::as_str).unwrap_or(""));
    let second = parse_engine_samples(sections.get("ENG2").map(String::as_str).unwrap_or(""));
    if second.is_empty() {
        return Err(
            "Windows GPU counters unavailable (no GPU Engine performance counter instances; requires Windows 10 1709+ with a WDDM 2.x driver)"
                .to_string(),
        );
    }

    let mut luid_by_phys: HashMap<u32, u64> = HashMap::new();
    let mut engine_busy: HashMap<(u32, String), f64> = HashMap::new();
    for (name, (busy_now, time_now)) in &second {
        let Some(phys) = instance_phys(name) else {
            continue;
        };
        if let Some(luid) = instance_luid(name) {
            luid_by_phys.insert(phys, luid);
        }
        let Some((busy_before, time_before)) = first.get(name) else {
            continue;
        };
        let time_delta = time_now.saturating_sub(*time_before);
        if time_delta == 0 {
            continue;
        }
        let engine = instance_engine_type(name).unwrap_or("unknown").to_string();
        let busy_delta = busy_now.saturating_sub(*busy_before);
        *engine_busy.entry((phys, engine)).or_insert(0.0) +=
            busy_delta as f64 / time_delta as f64 * 100.0;
    }
    let mut util_by_phys: HashMap<u32, f64> = HashMap::new();
    for ((phys, _), busy) in engine_busy {
        let entry = util_by_phys.entry(phys).or_insert(0.0);
        if busy > *entry {
            *entry = busy;
        }
    }

    let mut memory_by_phys: HashMap<u32, u64> = HashMap::new();
    for row in json_section_rows(&sections, "ADAPTERMEM") {
        let Some(name) = value_str(&row, "Name") else {
            continue;
        };
        let Some(phys) = instance_phys(&name) else {
            continue;
        };
        if let Some(luid) = instance_luid(&name) {
            luid_by_phys.insert(phys, luid);
        }
        if let Some(bytes) = value_u64(&row, "DedicatedUsage") {
            *memory_by_phys.entry(phys).or_insert(0) += bytes;
        }
    }

    let mut phys_ids: Vec<u32> = util_by_phys
        .keys()
        .chain(memory_by_phys.keys())
        .copied()
        .collect();
    phys_ids.sort_unstable();
    phys_ids.dedup();

    let mut metrics = Vec::new();
    for phys in phys_ids {
        let adapter = luid_by_phys
            .get(&phys)
            .and_then(|luid| adapters.iter().find(|adapter| adapter.luids.contains(luid)))
            .or_else(|| adapters.get(phys as usize));
        let Some(adapter) = adapter else {
            continue;
        };
        if nvidia_covered && adapter.vendor == "nvidia" {
            continue;
        }
        let memory_total_mi_b = adapter.memory_total_bytes.map(|bytes| bytes / BYTES_PER_MIB);
        let memory_used_mi_b = memory_by_phys.get(&phys).map(|bytes| bytes / BYTES_PER_MIB);
        metrics.push(GpuMetric {
            index: next_index + metrics.len() as u32,
            name: adapter.name.clone(),
            uuid: format!("win-gpu-{}", phys),
            vendor: adapter.vendor.clone(),
            driver_version: adapter.driver_version.clone(),
            // Power/temperature need a vendor CLI; the counters expose neither.
            power_draw_w: None,
            power_limit_w: None,
            temperature_c: None,
            gpu_util_percent: util_by_phys.get(&phys).map(|value| value.clamp(0.0, 100.0)),
            mem_util_percent: None,
            memory_total_mi_b,
            memory_free_mi_b: match (memory_total_mi_b, memory_used_mi_b) {
                (Some(total), Some(used)) => Some(total.saturating_sub(used)),
                _ => None,
            },
            memory_used_mi_b,
        });
    }
    if metrics.is_empty() {
        return Err(
            "Windows GPU counters found no adapters to attribute metrics to".to_string(),
        );
    }
    Ok(metrics)
}

#[derive(Debug, Clone)]
pub(crate) struct WindowsProcessSample {
    pub(crate) name: Option<String>,
    pub(crate) cpu_seconds: Option<f64>,
    pub(crate) working_set_bytes: Option<u64>,
    pub(crate) virtual_bytes: Option<u64>,
}

pub(crate) fn parse_windows_process_samples(section: &str) -> HashMap<u32, WindowsProcessSample> {
    parse_json_rows(section)
        .iter()
        .filter_map(|row| {
            let pid = value_u64(row, "Id")? as u32;
            Some((
                pid,
                WindowsProcessSample {
                    name: value_str(row, "Name"),
                    cpu_seconds: value_f64(row, "CpuSeconds"),
                    working_set_bytes: value_u64(row, "WorkingSet64"),
                    virtual_bytes: value_u64(row, "VirtualMemorySize64"),
                },
            ))
        })
        .collect()
}

/// Resolves GPU process PIDs to names/command lines for the detail popover.
pub(crate) fn windows_process_identity_command(pids: &[u32]) -> String {
    let filter = pids
        .iter()
        .map(|pid| format!("ProcessId={}", pid))
        .collect::<Vec<_>>()
        .join(" OR ");
    format!(
        "$ErrorActionPreference='SilentlyContinue'\nGet-CimInstance Win32_Process -Filter '{}' | Select-Object ProcessId,Name,CommandLine | ConvertTo-Json -Compress\nexit 0",
        filter
    )
}

/// Owner stays None: Win32_Process exposes it only through a per-process
/// GetOwner method call, and CommandLine of other users' processes already
/// requires elevation (it comes back null without it — Name is the fallback).
pub(crate) fn parse_windows_process_identity(
    output: &str,
) -> HashMap<u32, (Option<String>, Option<String>)> {
    parse_json_rows(output)
        .iter()
        .filter_map(|row| {
            let pid = value_u64(row, "ProcessId")? as u32;
            let command = value_str(row, "CommandLine").or_else(|| value_str(row, "Name"));
            Some((pid, (None, command)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_array_normalizes_ps51_shapes() {
        assert_eq!(json_array(serde_json::json!(null)).len(), 0);
        assert_eq!(json_array(serde_json::json!({"a": 1})).len(), 1);
        assert_eq!(json_array(serde_json::json!([{"a": 1}, {"a": 2}])).len(), 2);
    }

    #[test]
    fn parses_windows_cpu_output_with_two_poll_delta() {
        // Single-socket: PS 5.1 emits a bare object, not a one-element array.
        let output = "__CPUINFO__\n{\"Name\":\"Intel(R) Core(TM) i9-14900K\",\"NumberOfCores\":24,\"NumberOfLogicalProcessors\":32,\"MaxClockSpeed\":3200,\"CurrentClockSpeed\":3187}\n__CPUTIMES__\n{\"Name\":\"_Total\",\"PercentProcessorTime\":1500,\"Timestamp_Sys100NS\":3000}\n";
        let sections = split_sections(output);
        let mut previous = Some(CpuStatSample {
            idle: 1000,
            total: 2000,
        });
        let cpu = parse_windows_cpu_output(&sections, &mut previous).unwrap();
        assert_eq!(cpu.model_name.as_deref(), Some("Intel(R) Core(TM) i9-14900K"));
        assert_eq!(cpu.total_cores, Some(32));
        assert_eq!(cpu.online_cores, Some(32));
        // Δidle = 500, Δtotal = 1000 → 50% busy.
        assert_eq!(cpu.usage_percent, Some(50.0));
        assert_eq!(cpu.load_avg1, None);
        assert!((cpu.avg_clock_ghz.unwrap() - 3.187).abs() < 1e-9);
        // The new sample replaces the previous one for the next poll.
        assert_eq!(
            previous,
            Some(CpuStatSample {
                idle: 1500,
                total: 3000
            })
        );
    }

    #[test]
    fn windows_cpu_first_poll_has_no_usage() {
        let output = "__CPUINFO__\n{\"Name\":\"AMD Ryzen 9 7950X\",\"NumberOfLogicalProcessors\":32,\"MaxClockSpeed\":4500}\n__CPUTIMES__\n{\"Name\":\"_Total\",\"PercentProcessorTime\":10,\"Timestamp_Sys100NS\":20}\n";
        let sections = split_sections(output);
        let mut previous = None;
        let cpu = parse_windows_cpu_output(&sections, &mut previous).unwrap();
        assert_eq!(cpu.usage_percent, None);
        assert!(previous.is_some());
        // MaxClockSpeed fallback when CurrentClockSpeed is missing.
        assert_eq!(cpu.avg_clock_ghz, Some(4.5));
    }

    #[test]
    fn windows_cpu_two_sockets_dedupe_model_name() {
        let output = "__CPUINFO__\n[{\"Name\":\"Xeon Gold 6338\",\"NumberOfLogicalProcessors\":64},{\"Name\":\"Xeon Gold 6338\",\"NumberOfLogicalProcessors\":64}]\n__CPUTIMES__\n\n";
        let sections = split_sections(output);
        let cpu = parse_windows_cpu_output(&sections, &mut None).unwrap();
        assert_eq!(cpu.model_name.as_deref(), Some("Xeon Gold 6338 × 2"));
        assert_eq!(cpu.total_cores, Some(128));
    }

    #[test]
    fn windows_cpu_without_any_data_is_error() {
        let sections = split_sections("__CPUINFO__\n\n__CPUTIMES__\n\n");
        assert!(parse_windows_cpu_output(&sections, &mut None)
            .unwrap_err()
            .contains("Windows CPU telemetry unavailable"));
    }

    #[test]
    fn parses_windows_memory_output_with_pagefile_array() {
        let output = "__MEMORY__\n{\"TotalVisibleMemorySize\":33445532,\"FreePhysicalMemory\":16722766}\n__PAGEFILE__\n[{\"AllocatedBaseSize\":4096,\"CurrentUsage\":512},{\"AllocatedBaseSize\":2048,\"CurrentUsage\":0}]\n";
        let sections = split_sections(output);
        let memory = parse_windows_memory_output(&sections).unwrap();
        assert_eq!(memory.total_mi_b, Some(32661));
        assert_eq!(memory.available_mi_b, Some(16330));
        assert_eq!(memory.used_mi_b, Some(16330));
        assert_eq!(memory.free_mi_b, None);
        assert_eq!(memory.swap_total_mi_b, Some(6144));
        assert_eq!(memory.swap_used_mi_b, Some(512));
        assert_eq!(memory.swap_free_mi_b, Some(5632));
        assert!((memory.usage_percent.unwrap() - 50.0).abs() < 0.1);
    }

    #[test]
    fn windows_memory_without_pagefile_reports_zero_swap() {
        let output = "__MEMORY__\n{\"TotalVisibleMemorySize\":1048576,\"FreePhysicalMemory\":524288}\n__PAGEFILE__\n\n";
        let sections = split_sections(output);
        let memory = parse_windows_memory_output(&sections).unwrap();
        assert_eq!(memory.swap_total_mi_b, Some(0));
        assert_eq!(memory.swap_used_mi_b, Some(0));
    }

    #[test]
    fn windows_memory_missing_section_is_error() {
        let sections = split_sections("__MEMORY__\n\n");
        assert!(parse_windows_memory_output(&sections).is_err());
    }

    #[test]
    fn parses_windows_disks_and_tolerates_null_volume_name() {
        let output = "__DISK__\n[{\"DeviceID\":\"C:\",\"FileSystem\":\"NTFS\",\"VolumeName\":\"Windows\",\"Size\":511224635392,\"FreeSpace\":127806158848},{\"DeviceID\":\"D:\",\"FileSystem\":\"ReFS\",\"VolumeName\":null,\"Size\":2000381018112,\"FreeSpace\":1000190509056}]\n";
        let sections = split_sections(output);
        let disks = parse_windows_disk_output(&sections).unwrap();
        assert_eq!(disks.len(), 2);
        assert_eq!(disks[0].filesystem, "C:");
        assert_eq!(disks[0].mount_point, "C:\\");
        assert_eq!(disks[0].fs_type.as_deref(), Some("NTFS"));
        assert_eq!(disks[0].total_bytes, Some(511224635392));
        assert_eq!(disks[0].used_bytes, Some(511224635392 - 127806158848));
        assert!((disks[0].usage_percent.unwrap() - 75.0).abs() < 0.1);
        assert_eq!(disks[1].mount_point, "D:\\");
        assert!((disks[1].usage_percent.unwrap() - 50.0).abs() < 0.1);
    }

    #[test]
    fn windows_disk_empty_section_is_error() {
        let sections = split_sections("__DISK__\n\n");
        assert!(parse_windows_disk_output(&sections)
            .unwrap_err()
            .contains("no fixed drives"));
    }

    #[test]
    fn parses_quser_active_disconnected_and_idle_variants() {
        let output = " USERNAME              SESSIONNAME        ID  STATE   IDLE TIME  LOGON TIME\n>administrator         console             1  Active      none   7/15/2026 9:12 AM\n alice                                     2  Disc     1+03:04   7/14/2026 10:00 PM\n bob                   rdp-tcp#55          3  Active        59   7/15/2026 8:00 AM\n";
        let sessions = parse_quser_output(output);
        assert_eq!(sessions.len(), 3);
        assert_eq!(sessions[0].user, "administrator");
        assert_eq!(sessions[0].tty, "console");
        assert_eq!(sessions[0].login_time, "7/15/2026 9:12 AM");
        assert_eq!(sessions[0].from, None);
        // Disconnected sessions have an empty SESSIONNAME column.
        assert_eq!(sessions[1].user, "alice");
        assert_eq!(sessions[1].tty, "-");
        assert_eq!(sessions[1].login_time, "7/14/2026 10:00 PM");
        assert_eq!(sessions[2].tty, "rdp-tcp#55");
    }

    #[test]
    fn quser_headers_and_localized_noise_yield_no_rows() {
        assert!(parse_quser_output("").is_empty());
        assert!(parse_quser_output("No User exists for *\n").is_empty());
        // A localized header alone must not produce a phantom session.
        assert!(parse_quser_output(" BENUTZERNAME          SITZUNGSNAME       ID  STATUS  LEERLAUF   ANMELDEZEIT\n").is_empty());
    }

    #[test]
    fn quser_output_with_crlf_line_endings_parses() {
        let output = " USERNAME  SESSIONNAME  ID  STATE  IDLE TIME  LOGON TIME\r\n>admin  console  1  Active  none  7/15/2026 9:12 AM\r\n";
        let sessions = parse_quser_output(output);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].user, "admin");
    }

    #[test]
    fn parses_windows_gpu_probe_and_filters_virtual_adapters() {
        let output = "__NVSMI__\nnvidia-smi\n__ADAPTERS__\n[{\"Name\":\"NVIDIA GeForce RTX 4090\",\"AdapterCompatibility\":\"NVIDIA\",\"DriverVersion\":\"32.0.15.6094\"},{\"Name\":\"Intel(R) UHD Graphics 770\",\"AdapterCompatibility\":\"Intel Corporation\",\"DriverVersion\":\"31.0.101.4502\"},{\"Name\":\"Microsoft Basic Display Adapter\",\"AdapterCompatibility\":\"(Standard display types)\",\"DriverVersion\":\"10.0.19041.1\"}]\n";
        let probe = parse_windows_gpu_probe(output);
        assert!(probe.nvidia);
        assert!(probe.any());
        assert_eq!(probe.windows_adapters.len(), 2);
        assert_eq!(probe.windows_adapters[0].vendor, "nvidia");
        assert_eq!(probe.windows_adapters[1].vendor, "intel");
        assert_eq!(probe.windows_adapters[1].driver_version, "31.0.101.4502");
    }

    #[test]
    fn windows_gpu_probe_amd_by_adapter_compatibility() {
        let output = "__NVSMI__\n\n__ADAPTERS__\n{\"Name\":\"Radeon RX 7900 XTX\",\"AdapterCompatibility\":\"Advanced Micro Devices, Inc.\",\"DriverVersion\":\"31.0.24027.1012\"}\n";
        let probe = parse_windows_gpu_probe(output);
        assert!(!probe.nvidia);
        assert_eq!(probe.windows_adapters.len(), 1);
        assert_eq!(probe.windows_adapters[0].vendor, "amd");
        assert!(probe.any());
    }

    fn engine_row(name: &str, busy: u64, timestamp: u64) -> String {
        format!(
            "{{\"Name\":\"{}\",\"UtilizationPercentage\":{},\"Timestamp_Sys100NS\":{}}}",
            name, busy, timestamp
        )
    }

    fn adapter(name: &str, vendor: &str) -> WindowsGpuAdapter {
        WindowsGpuAdapter {
            name: name.to_string(),
            vendor: vendor.to_string(),
            driver_version: String::new(),
            luids: Vec::new(),
            memory_total_bytes: None,
        }
    }

    #[test]
    fn windows_gpu_counters_take_max_engine_type_sum_per_phys() {
        // Two 3D instances (30% + 30%) and one Copy instance (10%): overall
        // utilization is the max engine-type sum (60), not the plain sum (70).
        let eng1 = format!(
            "[{},{},{}]",
            engine_row("pid_100_luid_0x0_0xC64A_phys_0_eng_0_engtype_3D", 0, 0),
            engine_row("pid_200_luid_0x0_0xC64A_phys_0_eng_0_engtype_3D", 0, 0),
            engine_row("pid_100_luid_0x0_0xC64A_phys_0_eng_1_engtype_Copy", 0, 0)
        );
        let eng2 = format!(
            "[{},{},{}]",
            engine_row("pid_100_luid_0x0_0xC64A_phys_0_eng_0_engtype_3D", 300, 1000),
            engine_row("pid_200_luid_0x0_0xC64A_phys_0_eng_0_engtype_3D", 300, 1000),
            engine_row("pid_100_luid_0x0_0xC64A_phys_0_eng_1_engtype_Copy", 100, 1000)
        );
        let output = format!(
            "__ENG1__\n{}\n__ENG2__\n{}\n__ADAPTERMEM__\n{{\"Name\":\"luid_0x0_0xC64A_phys_0\",\"DedicatedUsage\":2147483648,\"SharedUsage\":0}}\n",
            eng1, eng2
        );
        let mut arc = adapter("Intel(R) Arc(TM) A770", "intel");
        arc.driver_version = "31.0.101.5522".to_string();
        let metrics = parse_windows_gpu_counters(&output, &[arc], false, 0).unwrap();
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].vendor, "intel");
        assert_eq!(metrics[0].uuid, "win-gpu-0");
        assert_eq!(metrics[0].gpu_util_percent, Some(60.0));
        assert_eq!(metrics[0].memory_used_mi_b, Some(2048));
        assert_eq!(metrics[0].memory_total_mi_b, None);
        assert_eq!(metrics[0].power_draw_w, None);
    }

    #[test]
    fn windows_gpu_counters_skip_nvidia_when_smi_covers_it() {
        let eng1 = format!(
            "[{},{}]",
            engine_row("pid_1_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 0, 0),
            engine_row("pid_2_luid_0x0_0x2_phys_1_eng_0_engtype_3D", 0, 0)
        );
        let eng2 = format!(
            "[{},{}]",
            engine_row("pid_1_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 500, 1000),
            engine_row("pid_2_luid_0x0_0x2_phys_1_eng_0_engtype_3D", 250, 1000)
        );
        let output = format!("__ENG1__\n{}\n__ENG2__\n{}\n__ADAPTERMEM__\n\n", eng1, eng2);
        let adapters = vec![
            adapter("NVIDIA GeForce RTX 4090", "nvidia"),
            adapter("Intel(R) UHD Graphics 770", "intel"),
        ];
        // nvidia-smi already reports phys_0; only the Intel iGPU remains, and
        // its index continues after the nvidia metrics (next_index = 1).
        let metrics = parse_windows_gpu_counters(&output, &adapters, true, 1).unwrap();
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].index, 1);
        assert_eq!(metrics[0].vendor, "intel");
        assert_eq!(metrics[0].gpu_util_percent, Some(25.0));
    }

    #[test]
    fn windows_gpu_counters_tolerate_instances_missing_from_first_sample() {
        let eng2 = engine_row("pid_9_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 100, 1000);
        let output = format!(
            "__ENG1__\n\n__ENG2__\n{}\n__ADAPTERMEM__\n{{\"Name\":\"luid_0x0_0x1_phys_0\",\"DedicatedUsage\":1048576,\"SharedUsage\":0}}\n",
            eng2
        );
        let adapters = vec![adapter("AMD Radeon 780M", "amd")];
        // No first sample → no utilization delta, but VRAM still reports.
        let metrics = parse_windows_gpu_counters(&output, &adapters, false, 0).unwrap();
        assert_eq!(metrics[0].gpu_util_percent, None);
        assert_eq!(metrics[0].memory_used_mi_b, Some(1));
    }

    #[test]
    fn windows_gpu_probe_merges_directx_registry_luids_and_vram() {
        let output = concat!(
            "__NVSMI__\nnvidia-smi\n",
            "__ADAPTERS__\n[{\"Name\":\"NVIDIA GeForce RTX 4070 Laptop GPU\",\"AdapterCompatibility\":\"NVIDIA\",\"DriverVersion\":\"32.0.15.6094\"},{\"Name\":\"Intel(R) Iris(R) Xe Graphics\",\"AdapterCompatibility\":\"Intel Corporation\",\"DriverVersion\":\"31.0.101.4502\"}]\n",
            // Two registry rows for the Intel adapter: one stale from a prior
            // boot, one live. Both LUIDs are kept.
            "__DXADAPTERS__\n[{\"Description\":\"NVIDIA GeForce RTX 4070 Laptop GPU\",\"AdapterLuid\":70000,\"DedicatedVideoMemory\":8589934592},{\"Description\":\"Intel(R) Iris(R) Xe Graphics\",\"AdapterLuid\":50000,\"DedicatedVideoMemory\":134217728},{\"Description\":\"Intel(R) Iris(R) Xe Graphics\",\"AdapterLuid\":50789,\"DedicatedVideoMemory\":134217728},{\"Description\":\"Microsoft Basic Render Driver\",\"AdapterLuid\":123}]\n",
        );
        let probe = parse_windows_gpu_probe(output);
        assert_eq!(probe.windows_adapters.len(), 2);
        assert_eq!(probe.windows_adapters[0].luids, vec![70000]);
        assert_eq!(
            probe.windows_adapters[0].memory_total_bytes,
            Some(8589934592)
        );
        assert_eq!(probe.windows_adapters[1].luids, vec![50000, 50789]);
        assert_eq!(
            probe.windows_adapters[1].memory_total_bytes,
            Some(134217728)
        );
    }

    #[test]
    fn windows_gpu_counters_map_by_luid_when_phys_order_disagrees() {
        // Hybrid laptop: the counters enumerate the iGPU as phys_0 and the
        // dGPU as phys_1, but Win32_VideoController lists the dGPU first.
        // LUID matching must win over the positional fallback.
        let igpu_engine = "pid_1_luid_0x00000000_0x0000C350_phys_0_eng_0_engtype_3D";
        let dgpu_engine = "pid_2_luid_0x00000000_0x00011170_phys_1_eng_0_engtype_3D";
        let eng1 = format!(
            "[{},{}]",
            engine_row(igpu_engine, 0, 0),
            engine_row(dgpu_engine, 0, 0)
        );
        let eng2 = format!(
            "[{},{}]",
            engine_row(igpu_engine, 400, 1000),
            engine_row(dgpu_engine, 900, 1000)
        );
        let output = format!(
            "__ENG1__\n{}\n__ENG2__\n{}\n__ADAPTERMEM__\n{{\"Name\":\"luid_0x00000000_0x0000C350_phys_0\",\"DedicatedUsage\":268435456,\"SharedUsage\":0}}\n",
            eng1, eng2
        );
        // 0xC350 = 50000 (iGPU), 0x11170 = 70000 (dGPU).
        let mut dgpu = adapter("NVIDIA GeForce RTX 4070 Laptop GPU", "nvidia");
        dgpu.luids = vec![70000];
        let mut igpu = adapter("Intel(R) Iris(R) Xe Graphics", "intel");
        igpu.luids = vec![50000];
        igpu.memory_total_bytes = Some(134217728);
        let metrics =
            parse_windows_gpu_counters(&output, &[dgpu, igpu], true, 1).unwrap();
        // The dGPU is nvidia-smi's job; only the iGPU comes from the counters,
        // correctly attributed despite phys_0 pointing at the dGPU positionally.
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].name, "Intel(R) Iris(R) Xe Graphics");
        assert_eq!(metrics[0].vendor, "intel");
        assert_eq!(metrics[0].index, 1);
        assert_eq!(metrics[0].gpu_util_percent, Some(40.0));
        assert_eq!(metrics[0].memory_total_mi_b, Some(128));
        assert_eq!(metrics[0].memory_used_mi_b, Some(256));
    }

    #[test]
    fn instance_luid_reassembles_high_and_low_parts() {
        assert_eq!(
            instance_luid("pid_1_luid_0x00000001_0x0000C64A_phys_0_engtype_3D"),
            Some((1u64 << 32) | 0xC64A)
        );
        assert_eq!(instance_luid("luid_0x00000000_0x0000C64A_phys_0"), Some(0xC64A));
        assert_eq!(instance_luid("no_luid_here"), None);
    }

    #[test]
    fn windows_gpu_counters_without_instances_is_error() {
        let output = "__ENG1__\n\n__ENG2__\n\n__ADAPTERMEM__\n\n";
        assert!(parse_windows_gpu_counters(output, &[], false, 0)
            .unwrap_err()
            .contains("WDDM"));
    }

    #[test]
    fn parses_windows_cpu_times_all_instances() {
        let json = "[{\"Name\":\"0\",\"PercentProcessorTime\":10,\"Timestamp_Sys100NS\":100},{\"Name\":\"1\",\"PercentProcessorTime\":20,\"Timestamp_Sys100NS\":100},{\"Name\":\"_Total\",\"PercentProcessorTime\":15,\"Timestamp_Sys100NS\":100}]";
        let times = parse_windows_cpu_times(json);
        assert_eq!(times.len(), 3);
        assert_eq!(
            times.get("_Total"),
            Some(&CpuStatSample {
                idle: 15,
                total: 100
            })
        );
    }

    #[test]
    fn parses_windows_process_samples_with_null_cpu() {
        let json = "[{\"Id\":4,\"Name\":\"System\",\"CpuSeconds\":null,\"WorkingSet64\":151552},{\"Id\":4242,\"Name\":\"python\",\"CpuSeconds\":123.5,\"WorkingSet64\":1073741824}]";
        let samples = parse_windows_process_samples(json);
        assert_eq!(samples.len(), 2);
        assert_eq!(samples.get(&4).unwrap().cpu_seconds, None);
        assert_eq!(samples.get(&4242).unwrap().cpu_seconds, Some(123.5));
        assert_eq!(
            samples.get(&4242).unwrap().working_set_bytes,
            Some(1073741824)
        );
    }

    #[test]
    fn builds_and_parses_windows_process_identity() {
        let command = windows_process_identity_command(&[100, 200]);
        assert!(command.contains("ProcessId=100 OR ProcessId=200"));
        let identity = parse_windows_process_identity(
            "[{\"ProcessId\":100,\"Name\":\"python.exe\",\"CommandLine\":\"python train.py\"},{\"ProcessId\":200,\"Name\":\"svchost.exe\",\"CommandLine\":null}]",
        );
        assert_eq!(
            identity.get(&100).unwrap().1.as_deref(),
            Some("python train.py")
        );
        // CommandLine needs elevation for other users' processes → Name fallback.
        assert_eq!(identity.get(&200).unwrap().1.as_deref(), Some("svchost.exe"));
        assert_eq!(identity.get(&100).unwrap().0, None);
    }

    #[test]
    fn windows_telemetry_sections_survive_crlf_output() {
        let output = "__HOSTNAME__\r\nWIN-GPU01\r\n__MEMORY__\r\n{\"TotalVisibleMemorySize\":2097152,\"FreePhysicalMemory\":1048576}\r\n__PAGEFILE__\r\n\r\n";
        let sections = split_sections(output);
        assert_eq!(sections.get("HOSTNAME").map(|s| s.trim().to_string()), Some("WIN-GPU01".to_string()));
        let memory = parse_windows_memory_output(&sections).unwrap();
        assert_eq!(memory.total_mi_b, Some(2048));
    }
}
