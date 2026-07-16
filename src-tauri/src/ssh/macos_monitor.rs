//! macOS telemetry collectors: Apple Silicon CPU (P/E cores), unified memory
//! via vm_stat, BSD df disks, and the integrated Apple GPU read from ioreg's
//! IOAccelerator performance statistics (no root required).

use crate::ssh::gpu_monitor::GpuMetric;
use crate::ssh::parse_util::{parse_first_u64, parse_loadavg, required_section, split_sections};
use crate::ssh::system_monitor::{CpuMetric, DiskMetric, MemoryMetric};
use std::collections::HashMap;

const BYTES_PER_MIB: u64 = 1024 * 1024;

pub(crate) const MACOS_CPU_COMMAND: &str = "printf '__BRAND__\\n'; sysctl -n machdep.cpu.brand_string 2>/dev/null || true; printf '\\n__NCPU__\\n'; sysctl -n hw.logicalcpu 2>/dev/null || true; printf '\\n__NCPU_ONLINE__\\n'; sysctl -n hw.activecpu 2>/dev/null || true; printf '\\n__PCORES__\\n'; sysctl -n hw.perflevel0.logicalcpu 2>/dev/null || true; printf '\\n__ECORES__\\n'; sysctl -n hw.perflevel1.logicalcpu 2>/dev/null || true; printf '\\n__LOADAVG__\\n'; sysctl -n vm.loadavg 2>/dev/null || true; printf '\\n__TOP__\\n'; top -l 2 -n 0 -s 1 2>/dev/null || true";

pub(crate) const MACOS_MEMORY_COMMAND: &str = "printf '__MEMSIZE__\\n'; sysctl -n hw.memsize 2>/dev/null || true; printf '\\n__VMSTAT__\\n'; vm_stat 2>/dev/null || true; printf '\\n__SWAP__\\n'; sysctl -n vm.swapusage 2>/dev/null || true";

pub(crate) const MACOS_DISK_COMMAND: &str = "printf '__DF__\\n'; df -P -k 2>/dev/null || true; printf '\\n__MOUNT__\\n'; mount 2>/dev/null || true";

pub(crate) const MACOS_GPU_COMMAND: &str = "printf '__IOREG__\\n'; ioreg -r -d 1 -c IOAccelerator 2>/dev/null || true; printf '\\n__BRAND__\\n'; sysctl -n machdep.cpu.brand_string 2>/dev/null || true";

pub(crate) const MACOS_CPU_DETAIL_COMMAND: &str = "printf '__BRAND__\\n'; sysctl -n machdep.cpu.brand_string 2>/dev/null || true; printf '\\n__NCPU__\\n'; sysctl -n hw.logicalcpu 2>/dev/null || true; printf '\\n__NCPU_ONLINE__\\n'; sysctl -n hw.activecpu 2>/dev/null || true; printf '\\n__PCORES__\\n'; sysctl -n hw.perflevel0.logicalcpu 2>/dev/null || true; printf '\\n__ECORES__\\n'; sysctl -n hw.perflevel1.logicalcpu 2>/dev/null || true; printf '\\n__LOADAVG__\\n'; sysctl -n vm.loadavg 2>/dev/null || true; printf '\\n__BOOTTIME__\\n'; sysctl -n kern.boottime 2>/dev/null || true; printf '\\n__NOW__\\n'; date +%s 2>/dev/null || true; printf '\\n__TOP__\\n'; top -l 2 -n 0 -s 1 2>/dev/null || true; printf '\\n__PROCESSES__\\n'; ps -Ao pid=,user=,%cpu=,%mem=,etime=,comm= -r 2>/dev/null | head -n 15";

pub(crate) const MACOS_MEMORY_DETAIL_COMMAND: &str = "printf '__MEMSIZE__\\n'; sysctl -n hw.memsize 2>/dev/null || true; printf '\\n__VMSTAT__\\n'; vm_stat 2>/dev/null || true; printf '\\n__SWAP__\\n'; sysctl -n vm.swapusage 2>/dev/null || true; printf '\\n__PROCESSES__\\n'; ps -Ao pid=,user=,rss=,vsz=,%mem=,comm= -m 2>/dev/null | head -n 15";

/// `sysctl -n vm.loadavg` prints `{ 1.85 2.10 2.20 }`; strip the braces and
/// reuse the shared loadavg parser.
pub(crate) fn parse_sysctl_loadavg(output: &str) -> (Option<f64>, Option<f64>, Option<f64>) {
    parse_loadavg(output.trim().trim_start_matches('{').trim_end_matches('}'))
}

/// Extracts CPU usage from `top -l 2` output. The FIRST `CPU usage:` line is
/// a since-boot average, so the LAST one (the real 1s sample) wins.
pub(crate) fn parse_top_cpu_usage(output: &str) -> Option<f64> {
    let line = output
        .lines()
        .rev()
        .find(|line| line.trim_start().starts_with("CPU usage:"))?;
    let mut user = None;
    let mut sys = None;
    let mut idle = None;
    for part in line.trim_start().trim_start_matches("CPU usage:").split(',') {
        let part = part.trim();
        let Some(percent_end) = part.find('%') else {
            continue;
        };
        let value = part[..percent_end].trim().parse::<f64>().ok();
        let label = part[percent_end + 1..].trim();
        match label {
            "user" => user = value,
            "sys" => sys = value,
            "idle" => idle = value,
            _ => {}
        }
    }
    match idle {
        Some(idle) => Some((100.0 - idle).clamp(0.0, 100.0)),
        None => match (user, sys) {
            (Some(user), Some(sys)) => Some((user + sys).clamp(0.0, 100.0)),
            _ => None,
        },
    }
}

pub(crate) fn parse_macos_cpu_output(output: &str) -> Result<CpuMetric, String> {
    let sections = split_sections(output);
    let section = |name: &str| sections.get(name).map(|value| value.trim().to_string());

    let brand = section("BRAND").filter(|value| !value.is_empty());
    let total_cores = sections.get("NCPU").and_then(|value| parse_first_u64(value));
    let online_cores = sections
        .get("NCPU_ONLINE")
        .and_then(|value| parse_first_u64(value))
        .or(total_cores);
    let usage = sections.get("TOP").and_then(|value| parse_top_cpu_usage(value));

    if brand.is_none() && total_cores.is_none() && usage.is_none() {
        return Err("macOS CPU telemetry unavailable (sysctl/top produced no output)".to_string());
    }

    let p_cores = sections.get("PCORES").and_then(|value| parse_first_u64(value));
    let e_cores = sections.get("ECORES").and_then(|value| parse_first_u64(value));
    let model_name = match (brand, p_cores, e_cores) {
        (Some(brand), Some(p), Some(e)) => Some(format!("{} ({}P+{}E)", brand, p, e)),
        (Some(brand), _, _) => Some(brand),
        (None, _, _) => None,
    };

    let load = sections
        .get("LOADAVG")
        .map(|value| parse_sysctl_loadavg(value))
        .unwrap_or((None, None, None));

    Ok(CpuMetric {
        model_name,
        usage_percent: usage,
        load_avg1: load.0,
        load_avg5: load.1,
        load_avg15: load.2,
        total_cores,
        online_cores,
        // Apple Silicon exposes no hw.cpufrequency; the UI renders n/a.
        avg_clock_ghz: None,
    })
}

/// Parses `vm_stat` output into (page_size_bytes, page counts by label).
pub(crate) fn parse_vm_stat(output: &str) -> Option<(u64, HashMap<String, u64>)> {
    let mut page_size = None;
    let mut counts = HashMap::new();
    for line in output.lines() {
        if line.contains("page size of") {
            page_size = line
                .split("page size of")
                .nth(1)
                .and_then(|rest| rest.split_whitespace().next())
                .and_then(|value| value.parse::<u64>().ok());
            continue;
        }
        if let Some((label, value)) = line.split_once(':') {
            if let Ok(pages) = value.trim().trim_end_matches('.').parse::<u64>() {
                counts.insert(label.trim().to_string(), pages);
            }
        }
    }
    page_size.map(|size| (size, counts))
}

/// `sysctl -n vm.swapusage`: `total = 2048.00M  used = 1058.75M  free = 989.25M  (encrypted)`
/// Returns (total, used, free) in MiB.
pub(crate) fn parse_swapusage(output: &str) -> (Option<u64>, Option<u64>, Option<u64>) {
    let mut values: HashMap<&str, u64> = HashMap::new();
    let tokens: Vec<&str> = output.split_whitespace().collect();
    for window in tokens.windows(3) {
        let [key, eq, value] = window else { continue };
        if *eq != "=" {
            continue;
        }
        let (number, unit) = value.split_at(value.len().saturating_sub(1));
        let Ok(amount) = number.parse::<f64>() else {
            continue;
        };
        let mib = match unit {
            "K" => amount / 1024.0,
            "M" => amount,
            "G" => amount * 1024.0,
            _ => continue,
        };
        values.insert(*key, mib.round() as u64);
    }
    (
        values.get("total").copied(),
        values.get("used").copied(),
        values.get("free").copied(),
    )
}

pub(crate) fn parse_macos_memory_output(output: &str) -> Result<MemoryMetric, String> {
    let sections = split_sections(output);
    let total_bytes = sections
        .get("MEMSIZE")
        .and_then(|value| parse_first_u64(value))
        .ok_or_else(|| "macOS memory telemetry unavailable (hw.memsize missing)".to_string())?;
    let (page_size, pages) = required_section(&sections, "VMSTAT")
        .ok()
        .and_then(parse_vm_stat)
        .ok_or_else(|| "macOS memory telemetry unavailable (vm_stat missing)".to_string())?;

    let count = |label: &str| pages.get(label).copied().unwrap_or(0);
    // Approximates Activity Monitor's "Memory Used".
    let used_bytes = (count("Pages active")
        + count("Pages wired down")
        + count("Pages occupied by compressor"))
        * page_size;
    let free_bytes = count("Pages free") * page_size;
    let available_bytes = total_bytes.saturating_sub(used_bytes);

    let (swap_total, swap_used, swap_free) = sections
        .get("SWAP")
        .map(|value| parse_swapusage(value))
        .unwrap_or((None, None, None));

    Ok(MemoryMetric {
        total_mi_b: Some(total_bytes / BYTES_PER_MIB),
        used_mi_b: Some(used_bytes / BYTES_PER_MIB),
        available_mi_b: Some(available_bytes / BYTES_PER_MIB),
        free_mi_b: Some(free_bytes / BYTES_PER_MIB),
        usage_percent: (total_bytes > 0)
            .then_some((used_bytes as f64 / total_bytes as f64) * 100.0),
        swap_total_mi_b: swap_total,
        swap_used_mi_b: swap_used,
        swap_free_mi_b: swap_free,
    })
}

/// Maps `mount` output lines like
/// `/dev/disk3s1s1 on / (apfs, sealed, local, read-only journaled)`
/// to mount point -> filesystem type.
pub(crate) fn parse_mount_fs_types(output: &str) -> HashMap<String, String> {
    output
        .lines()
        .filter_map(|line| {
            let options_start = line.rfind(" (")?;
            let mount_and_device = &line[..options_start];
            let on_index = mount_and_device.find(" on ")?;
            let mount_point = mount_and_device[on_index + 4..].trim().to_string();
            let fs_type = line[options_start + 2..]
                .trim_end_matches(')')
                .split(',')
                .next()?
                .trim()
                .to_string();
            Some((mount_point, fs_type))
        })
        .collect()
}

/// Parses BSD `df -P -k` (6 columns, no fstype) combined with `mount` output.
/// Filesystems (`map auto_home`) and mount points (`/Volumes/My Disk`) may
/// contain spaces, so the numeric triple + `%` column is located by scanning.
pub(crate) fn parse_macos_disk_output(output: &str) -> Result<Vec<DiskMetric>, String> {
    let sections = split_sections(output);
    let df = required_section(&sections, "DF")
        .map_err(|_| "macOS disk telemetry unavailable (df produced no output)".to_string())?;
    let fs_types = sections
        .get("MOUNT")
        .map(|mount| parse_mount_fs_types(mount))
        .unwrap_or_default();

    let mut disks = Vec::new();
    for line in df.lines().skip(1).map(str::trim).filter(|line| !line.is_empty()) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 6 {
            continue;
        }
        // Find the first position where three numbers are followed by "N%".
        let anchor = (1..fields.len().saturating_sub(4)).find(|&index| {
            fields[index..index + 3].iter().all(|f| f.parse::<u64>().is_ok())
                && fields[index + 3].ends_with('%')
        });
        let Some(index) = anchor else { continue };

        let blocks = |offset: usize| fields[index + offset].parse::<u64>().ok().map(|kib| kib * 1024);
        let mount_point = fields[index + 4..].join(" ");
        disks.push(DiskMetric {
            filesystem: fields[..index].join(" "),
            fs_type: fs_types.get(&mount_point).cloned(),
            total_bytes: blocks(0),
            used_bytes: blocks(1),
            available_bytes: blocks(2),
            usage_percent: fields[index + 3].trim_end_matches('%').parse::<f64>().ok(),
            mount_point,
        });
    }

    if disks.is_empty() {
        return Err("macOS disk telemetry unavailable (df returned no parseable rows)".to_string());
    }
    Ok(disks)
}

/// Finds `"key"=value` or `"key" = value` in ioreg text output and parses the
/// numeric value.
fn extract_ioreg_number(node: &str, key: &str) -> Option<f64> {
    let needle = format!("\"{}\"", key);
    let start = node.find(&needle)? + needle.len();
    let rest = node[start..].trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    let numeric: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    numeric.parse::<f64>().ok()
}

/// Parses `ioreg -r -d 1 -c IOAccelerator` plus the CPU brand string into GPU
/// cards. Power/temperature need root powermetrics and stay None; total
/// unified memory is intentionally None.
pub(crate) fn parse_macos_gpu_output(output: &str) -> Result<Vec<GpuMetric>, String> {
    let sections = split_sections(output);
    let ioreg = required_section(&sections, "IOREG").map_err(|_| {
        "Apple GPU metrics unavailable: no IOAccelerator node in ioreg output".to_string()
    })?;
    let brand = sections
        .get("BRAND")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty());
    let name = brand
        .map(|brand| format!("{} GPU", brand))
        .unwrap_or_else(|| "Apple GPU".to_string());

    // `+-o AGXAcceleratorG14X ...` starts each accelerator node; treat the
    // whole output as one node when the marker is absent.
    let nodes: Vec<&str> = if ioreg.contains("+-o ") {
        ioreg
            .split("+-o ")
            .skip(1)
            .filter(|node| !node.trim().is_empty())
            .collect()
    } else {
        vec![ioreg]
    };
    if nodes.is_empty() {
        return Err(
            "Apple GPU metrics unavailable: no IOAccelerator node in ioreg output".to_string(),
        );
    }

    let metrics = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| GpuMetric {
            index: index as u32,
            name: name.clone(),
            uuid: if index == 0 {
                "apple-gpu".to_string()
            } else {
                format!("apple-gpu-{}", index)
            },
            vendor: "apple".to_string(),
            driver_version: String::new(),
            power_draw_w: None,
            power_limit_w: None,
            temperature_c: None,
            gpu_util_percent: extract_ioreg_number(node, "Device Utilization %")
                .or_else(|| extract_ioreg_number(node, "GPU Activity(%)")),
            mem_util_percent: None,
            memory_total_mi_b: None,
            memory_used_mi_b: extract_ioreg_number(node, "In use system memory")
                .map(|bytes| (bytes / BYTES_PER_MIB as f64).round() as u64),
            memory_free_mi_b: None,
        })
        .collect();
    Ok(metrics)
}

/// `sysctl -n kern.boottime`: `{ sec = 1752418000, usec = 292149 } Sun Jul 13 ...`
pub(crate) fn parse_kern_boottime(output: &str) -> Option<u64> {
    let start = output.find("sec")? + 3;
    let rest = output[start..].trim_start().strip_prefix('=')?.trim_start();
    let numeric: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    numeric.parse::<u64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sysctl_loadavg_with_braces() {
        assert_eq!(
            parse_sysctl_loadavg("{ 1.85 2.10 2.20 }"),
            (Some(1.85), Some(2.10), Some(2.20))
        );
    }

    #[test]
    fn parses_top_cpu_usage_takes_last_sample() {
        let output = "Processes: 500 total, 2 running\nLoad Avg: 2.0, 2.1, 2.2\nCPU usage: 3.15% user, 2.10% sys, 94.75% idle \nSharedLibs: 300M resident\nCPU usage: 12.50% user, 6.25% sys, 81.25% idle \nMemRegions: 100 total";
        assert_eq!(parse_top_cpu_usage(output), Some(18.75));
    }

    #[test]
    fn parses_macos_cpu_output_apple_silicon() {
        let output = "__BRAND__\nApple M2 Pro\n__NCPU__\n12\n__NCPU_ONLINE__\n12\n__PCORES__\n8\n__ECORES__\n4\n__LOADAVG__\n{ 1.85 2.10 2.20 }\n__TOP__\nCPU usage: 3.00% user, 2.00% sys, 95.00% idle \nCPU usage: 10.00% user, 5.00% sys, 85.00% idle \n";
        let cpu = parse_macos_cpu_output(output).unwrap();
        assert_eq!(cpu.model_name.as_deref(), Some("Apple M2 Pro (8P+4E)"));
        assert_eq!(cpu.total_cores, Some(12));
        assert_eq!(cpu.online_cores, Some(12));
        assert_eq!(cpu.usage_percent, Some(15.0));
        assert_eq!(cpu.load_avg1, Some(1.85));
        assert_eq!(cpu.avg_clock_ghz, None);
    }

    #[test]
    fn macos_cpu_output_without_perflevels_omits_suffix() {
        let output = "__BRAND__\nIntel(R) Core(TM) i7-9750H\n__NCPU__\n12\n__NCPU_ONLINE__\n12\n__PCORES__\n\n__ECORES__\n\n__LOADAVG__\n{ 1.0 1.0 1.0 }\n__TOP__\nCPU usage: 10.00% user, 5.00% sys, 85.00% idle \n";
        let cpu = parse_macos_cpu_output(output).unwrap();
        assert_eq!(cpu.model_name.as_deref(), Some("Intel(R) Core(TM) i7-9750H"));
    }

    #[test]
    fn parses_vm_stat_page_size_and_counts() {
        let output = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free:                               31461.\nPages active:                            434653.\nPages inactive:                          401043.\nPages wired down:                        129535.\nPages occupied by compressor:            240236.\n";
        let (page_size, counts) = parse_vm_stat(output).unwrap();
        assert_eq!(page_size, 16384);
        assert_eq!(counts.get("Pages free"), Some(&31461));
        assert_eq!(counts.get("Pages wired down"), Some(&129535));
        assert_eq!(counts.get("Pages occupied by compressor"), Some(&240236));
    }

    #[test]
    fn parses_swapusage() {
        let (total, used, free) =
            parse_swapusage("total = 2048.00M  used = 1058.75M  free = 989.25M  (encrypted)");
        assert_eq!(total, Some(2048));
        assert_eq!(used, Some(1059));
        assert_eq!(free, Some(989));
    }

    #[test]
    fn parses_macos_memory_output_matches_activity_monitor_math() {
        let output = "__MEMSIZE__\n34359738368\n__VMSTAT__\nMach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free:      100000.\nPages active:    400000.\nPages wired down: 130000.\nPages occupied by compressor: 240000.\n__SWAP__\ntotal = 2048.00M  used = 1024.00M  free = 1024.00M  (encrypted)\n";
        let memory = parse_macos_memory_output(output).unwrap();
        // used = (400000 + 130000 + 240000) * 16384 bytes = 12032.5 GiB-ish in MiB
        assert_eq!(memory.used_mi_b, Some(770_000 * 16384 / (1024 * 1024)));
        assert_eq!(memory.total_mi_b, Some(32768));
        assert_eq!(memory.swap_used_mi_b, Some(1024));
        assert!(memory.usage_percent.unwrap() > 0.0);
    }

    #[test]
    fn parses_macos_df_with_mount_fstypes() {
        let output = "__DF__\nFilesystem   1024-blocks       Used Available Capacity  Mounted on\n/dev/disk3s1s1  971350180   10485760 549453824     2%    /\ndevfs                 200        200         0   100%    /dev\n/dev/disk3s5    971350180  401408000 549453824    43%    /System/Volumes/Data\nmap auto_home           0          0         0   100%    /System/Volumes/Data/home\n__MOUNT__\n/dev/disk3s1s1 on / (apfs, sealed, local, read-only journaled)\ndevfs on /dev (devfs, local, nobrowse)\n/dev/disk3s5 on /System/Volumes/Data (apfs, local, journaled, nobrowse)\nmap auto_home on /System/Volumes/Data/home (autofs, automounted, nobrowse)\n";
        let disks = parse_macos_disk_output(output).unwrap();
        assert_eq!(disks.len(), 4);
        let root = disks.iter().find(|d| d.mount_point == "/").unwrap();
        assert_eq!(root.fs_type.as_deref(), Some("apfs"));
        assert_eq!(root.total_bytes, Some(971350180 * 1024));
        assert_eq!(root.usage_percent, Some(2.0));
        let auto_home = disks
            .iter()
            .find(|d| d.mount_point == "/System/Volumes/Data/home")
            .unwrap();
        assert_eq!(auto_home.filesystem, "map auto_home");
        assert_eq!(auto_home.fs_type.as_deref(), Some("autofs"));
        let dev = disks.iter().find(|d| d.mount_point == "/dev").unwrap();
        assert_eq!(dev.fs_type.as_deref(), Some("devfs"));
    }

    #[test]
    fn parses_df_row_with_space_in_mount_point() {
        let output = "__DF__\nFilesystem 1024-blocks Used Available Capacity Mounted on\n/dev/disk4s1 976762584 1024 976761560 1% /Volumes/My Disk\n__MOUNT__\n/dev/disk4s1 on /Volumes/My Disk (apfs, local, journaled)\n";
        let disks = parse_macos_disk_output(output).unwrap();
        assert_eq!(disks[0].mount_point, "/Volumes/My Disk");
        assert_eq!(disks[0].fs_type.as_deref(), Some("apfs"));
    }

    #[test]
    fn parses_ioreg_performance_statistics() {
        let output = "__IOREG__\n+-o AGXAcceleratorG14X  <class AGXAcceleratorG14X, id 0x100000abc, registered, matched, active, busy 0 (0 ms), retain 100>\n    {\n      \"PerformanceStatistics\" = {\"Device Utilization %\"=37,\"Renderer Utilization %\"=35,\"Tiler Utilization %\"=12,\"In use system memory\"=2147483648,\"Alloc system memory\"=3221225472}\n      \"gpu-core-count\" = 19\n    }\n__BRAND__\nApple M2 Pro\n";
        let gpus = parse_macos_gpu_output(output).unwrap();
        assert_eq!(gpus.len(), 1);
        let gpu = &gpus[0];
        assert_eq!(gpu.vendor, "apple");
        assert_eq!(gpu.uuid, "apple-gpu");
        assert_eq!(gpu.name, "Apple M2 Pro GPU");
        assert_eq!(gpu.gpu_util_percent, Some(37.0));
        assert_eq!(gpu.memory_used_mi_b, Some(2048));
        assert_eq!(gpu.power_draw_w, None);
    }

    #[test]
    fn ioreg_node_without_perf_stats_still_yields_card() {
        let output = "__IOREG__\n+-o IntelAccelerator  <class IntelAccelerator, id 0x100000abc>\n    {\n      \"IOClass\" = \"IntelAccelerator\"\n    }\n__BRAND__\n\n";
        let gpus = parse_macos_gpu_output(output).unwrap();
        assert_eq!(gpus.len(), 1);
        assert_eq!(gpus[0].name, "Apple GPU");
        assert_eq!(gpus[0].gpu_util_percent, None);
    }

    #[test]
    fn ioreg_empty_output_is_error() {
        let output = "__IOREG__\n\n__BRAND__\nApple M2\n";
        assert!(parse_macos_gpu_output(output)
            .unwrap_err()
            .contains("no IOAccelerator node"));
    }

    #[test]
    fn parses_kern_boottime() {
        assert_eq!(
            parse_kern_boottime("{ sec = 1752418000, usec = 292149 } Sun Jul 13 08:26:40 2026"),
            Some(1752418000)
        );
    }
}
