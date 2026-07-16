use crate::ssh::parse_util::{parse_optional_f64, parse_optional_u64};
use serde::Serialize;
use serde_json::Value;

pub const NVIDIA_SMI_QUERY: &str = "nvidia-smi --query-gpu=index,name,uuid,driver_version,power.draw,power.limit,temperature.gpu,utilization.gpu,utilization.memory,memory.total,memory.used,memory.free --format=csv,noheader,nounits";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuMetric {
    pub index: u32,
    pub name: String,
    pub uuid: String,
    pub vendor: String,
    pub driver_version: String,
    pub power_draw_w: Option<f64>,
    pub power_limit_w: Option<f64>,
    pub temperature_c: Option<f64>,
    pub gpu_util_percent: Option<f64>,
    pub mem_util_percent: Option<f64>,
    pub memory_total_mi_b: Option<u64>,
    pub memory_used_mi_b: Option<u64>,
    pub memory_free_mi_b: Option<u64>,
}

pub fn parse_nvidia_smi_csv(output: &str) -> Result<Vec<GpuMetric>, String> {
    let mut metrics = Vec::new();

    for line in output.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        if fields.len() != 12 {
            return Err(format!(
                "GPU metrics unavailable: unexpected nvidia-smi column count {}",
                fields.len()
            ));
        }

        metrics.push(GpuMetric {
            index: fields[0]
                .parse::<u32>()
                .map_err(|_| "GPU metrics unavailable: invalid GPU index".to_string())?,
            name: fields[1].to_string(),
            uuid: fields[2].to_string(),
            vendor: "nvidia".to_string(),
            driver_version: fields[3].to_string(),
            power_draw_w: parse_optional_f64(fields[4]),
            power_limit_w: parse_optional_f64(fields[5]),
            temperature_c: parse_optional_f64(fields[6]),
            gpu_util_percent: parse_optional_f64(fields[7]),
            mem_util_percent: parse_optional_f64(fields[8]),
            memory_total_mi_b: parse_optional_u64(fields[9]),
            memory_used_mi_b: parse_optional_u64(fields[10]),
            memory_free_mi_b: parse_optional_u64(fields[11]),
        });
    }

    if metrics.is_empty() {
        return Err("GPU metrics unavailable: no GPU rows returned by nvidia-smi".to_string());
    }

    Ok(metrics)
}

const BYTES_PER_MIB: f64 = 1024.0 * 1024.0;

/// Detected GPU management tools on a host, discovered once per connection.
#[derive(Debug, Clone, Default)]
pub struct GpuProbe {
    pub nvidia: bool,
    pub rocm_smi: bool,
    pub amd_smi: bool,
    pub xpu_smi: bool,
    pub intel_gpu_top: bool,
    /// macOS host: the integrated Apple GPU is read from ioreg, no tool needed.
    pub apple: bool,
    pub xpu_devices: Vec<XpuDevice>,
    /// Windows host: physical Win32_VideoController adapters in enumeration
    /// order. Those without a vendor CLI are read from the WDDM performance
    /// counters.
    pub windows_adapters: Vec<WindowsGpuAdapter>,
}

#[derive(Debug, Clone)]
pub struct XpuDevice {
    pub id: u32,
    pub name: String,
    pub uuid: Option<String>,
}

#[derive(Debug, Clone)]
pub struct WindowsGpuAdapter {
    pub name: String,
    /// One of the existing vendor tags ("nvidia" / "amd" / "intel").
    pub vendor: String,
    pub driver_version: String,
    /// Adapter LUIDs from the DirectX registry (stale reboots leave extra
    /// entries, hence a list). Perf counter instance names embed the live
    /// LUID, giving an exact adapter match on hybrid iGPU+dGPU hosts where
    /// the `phys_N` ordering heuristic would misattribute.
    pub luids: Vec<u64>,
    /// DedicatedVideoMemory from the DirectX registry; the perf counters
    /// expose only usage, and Win32_VideoController.AdapterRAM caps at 4 GiB.
    pub memory_total_bytes: Option<u64>,
}

impl GpuProbe {
    pub fn any(&self) -> bool {
        self.nvidia
            || self.rocm_smi
            || self.amd_smi
            || self.xpu_smi
            || self.intel_gpu_top
            || self.apple
            || !self.windows_adapters.is_empty()
    }
}

/// Shell snippet that lists which GPU tools exist. Each tool is probed with a
/// single-argument `command -v` inside a loop: dash's `command -v` (Ubuntu's
/// /bin/sh, used by run_remote_command's `sh -lc`) only inspects its first
/// operand, so a multi-argument form would silently ignore every tool after
/// the first. The trailing `true` keeps the overall exit status zero.
pub const GPU_PROBE_COMMAND: &str = "uname -s 2>/dev/null; for t in nvidia-smi rocm-smi amd-smi xpu-smi intel_gpu_top; do command -v \"$t\" 2>/dev/null; done; for p in /opt/rocm/bin/rocm-smi /opt/rocm/bin/amd-smi; do [ -x \"$p\" ] && echo \"$p\"; done; true";

pub fn parse_gpu_probe(output: &str) -> GpuProbe {
    let mut probe = GpuProbe::default();
    for line in output.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let basename = line.rsplit(['/', '\\']).next().unwrap_or(line);
        match basename {
            "nvidia-smi" => probe.nvidia = true,
            "rocm-smi" => probe.rocm_smi = true,
            "amd-smi" => probe.amd_smi = true,
            "xpu-smi" => probe.xpu_smi = true,
            "intel_gpu_top" => probe.intel_gpu_top = true,
            // From the leading `uname -s`: a Darwin host exposes its Apple GPU
            // through ioreg without any dedicated tool.
            "Darwin" => probe.apple = true,
            _ => {}
        }
    }
    probe
}

fn number_from_value(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => {
            let trimmed = text.trim();
            // Strip a trailing unit token if present (e.g. "45.0 C", "35 W").
            let numeric: String = trimmed
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-' || *c == '+')
                .collect();
            numeric.parse::<f64>().ok()
        }
        _ => None,
    }
}

/// Parses `rocm-smi ... --json`, whose values are unit-suffixed strings and
/// whose key names drift across ROCm releases, so keys are matched by
/// lowercase substring rather than exact name.
pub fn parse_rocm_smi_json(output: &str) -> Result<Vec<GpuMetric>, String> {
    let root: Value = serde_json::from_str(output)
        .map_err(|error| format!("AMD GPU metrics unavailable: invalid rocm-smi JSON: {}", error))?;
    let object = root
        .as_object()
        .ok_or_else(|| "AMD GPU metrics unavailable: rocm-smi JSON is not an object".to_string())?;

    let mut cards: Vec<(u32, &Value)> = object
        .iter()
        .filter_map(|(key, value)| {
            let rest = key.strip_prefix("card")?;
            rest.parse::<u32>().ok().map(|index| (index, value))
        })
        .collect();
    cards.sort_by_key(|(index, _)| *index);

    let mut metrics = Vec::new();
    for (index, card) in cards {
        let fields = match card.as_object() {
            Some(fields) => fields,
            None => continue,
        };
        let find = |needles: &[&str]| -> Option<&Value> {
            fields.iter().find_map(|(key, value)| {
                let lower = key.to_lowercase();
                if needles.iter().all(|needle| lower.contains(needle)) {
                    Some(value)
                } else {
                    None
                }
            })
        };
        let find_str = |needles: &[&str]| -> Option<String> {
            find(needles).and_then(|value| match value {
                Value::String(text) if !text.trim().is_empty() => Some(text.trim().to_string()),
                _ => None,
            })
        };

        let used_bytes = find(&["vram", "used"]).and_then(number_from_value);
        let total_bytes = find(&["vram", "total", "memory"])
            .or_else(|| find(&["vram", "total"]))
            .and_then(number_from_value);
        // rocm-smi reports VRAM as bytes; the UI works in MiB.
        let memory_used_mi_b = used_bytes.map(|bytes| (bytes / BYTES_PER_MIB).round() as u64);
        let memory_total_mi_b = total_bytes.map(|bytes| (bytes / BYTES_PER_MIB).round() as u64);
        let memory_free_mi_b = match (memory_total_mi_b, memory_used_mi_b) {
            (Some(total), Some(used)) => Some(total.saturating_sub(used)),
            _ => None,
        };

        let temperature_c = find(&["temperature", "edge"])
            .or_else(|| find(&["temperature", "junction"]))
            .or_else(|| find(&["temperature", "sensor"]))
            .and_then(number_from_value);
        let power_draw_w = find(&["average graphics package"])
            .or_else(|| find(&["current socket graphics package"]))
            .or_else(|| find(&["average socket"]))
            .or_else(|| find(&["socket power"]))
            .and_then(number_from_value);
        let power_limit_w = find(&["max graphics package"]).and_then(number_from_value);

        let name = find_str(&["card series"])
            .or_else(|| find_str(&["card model"]))
            .or_else(|| find_str(&["device name"]))
            .unwrap_or_else(|| "AMD GPU".to_string());
        let uuid = find_str(&["unique id"])
            .filter(|id| id != "N/A" && id != "0x0" && id != "0")
            .unwrap_or_else(|| format!("amd-{}", index));

        metrics.push(GpuMetric {
            index,
            name,
            uuid,
            vendor: "amd".to_string(),
            driver_version: String::new(),
            power_draw_w,
            power_limit_w,
            temperature_c,
            gpu_util_percent: find(&["gpu use"]).and_then(number_from_value),
            mem_util_percent: find(&["gpu memory use"])
                .or_else(|| find(&["memory allocated"]))
                .and_then(number_from_value),
            memory_total_mi_b,
            memory_used_mi_b,
            memory_free_mi_b,
        });
    }

    if metrics.is_empty() {
        return Err("AMD GPU metrics unavailable: no cards in rocm-smi output".to_string());
    }
    Ok(metrics)
}

pub const XPU_DISCOVERY_COMMAND: &str = "xpu-smi discovery -j 2>/dev/null || true";

pub fn parse_xpu_discovery(output: &str) -> Vec<XpuDevice> {
    let root: Value = match serde_json::from_str(output) {
        Ok(value) => value,
        Err(_) => return Vec::new(),
    };
    let list = root
        .get("device_list")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    list.iter()
        .filter_map(|device| {
            let id = device
                .get("device_id")
                .and_then(|value| match value {
                    Value::Number(number) => number.as_u64().map(|id| id as u32),
                    Value::String(text) => text.trim().parse::<u32>().ok(),
                    _ => None,
                })?;
            let name = device
                .get("device_name")
                .and_then(Value::as_str)
                .unwrap_or("Intel GPU")
                .to_string();
            let uuid = device
                .get("uuid")
                .and_then(Value::as_str)
                .filter(|text| !text.trim().is_empty())
                .map(|text| text.to_string());
            Some(XpuDevice { id, name, uuid })
        })
        .collect()
}

/// Builds a single command that dumps `xpu-smi stats` JSON for each device,
/// separated by `__XPU_{id}__` markers so one round-trip covers all devices.
pub fn xpu_stats_command(devices: &[XpuDevice]) -> String {
    let mut command = String::new();
    for device in devices {
        command.push_str(&format!(
            "printf '\\n__XPU_{id}__\\n'; xpu-smi stats -d {id} -j 2>/dev/null || true; ",
            id = device.id
        ));
    }
    command.push_str("true");
    command
}

/// Finds a numeric metric in an xpu-smi JSON tree, matching either
/// (a) a key whose name contains all `needles`, or (b) a `{metrics_type: "...",
/// value: N}` tag object whose tag contains all `needles`. `exclude` rejects
/// either form when any token is present.
fn find_metric_deep(value: &Value, needles: &[&str], exclude: &[&str]) -> Option<f64> {
    let matches = |haystack: &str| {
        let lower = haystack.to_lowercase();
        needles.iter().all(|needle| lower.contains(needle))
            && !exclude.iter().any(|token| lower.contains(token))
    };
    match value {
        Value::Object(map) => {
            // Tag-object form: {"metrics_type": "...UTILIZATION", "value": 42.5}
            let tag = map
                .get("metrics_type")
                .or_else(|| map.get("metricsType"))
                .and_then(Value::as_str);
            if let Some(tag) = tag {
                if matches(tag) {
                    if let Some(number) = map
                        .get("value")
                        .or_else(|| map.get("data"))
                        .and_then(number_from_value)
                    {
                        return Some(number);
                    }
                }
            }
            for (key, child) in map {
                if matches(key) {
                    if let Some(number) = number_from_value(child) {
                        return Some(number);
                    }
                }
                if let Some(found) = find_metric_deep(child, needles, exclude) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(items) => items
            .iter()
            .find_map(|item| find_metric_deep(item, needles, exclude)),
        _ => None,
    }
}

pub fn parse_xpu_stats(output: &str, device: &XpuDevice) -> Option<GpuMetric> {
    let root: Value = serde_json::from_str(output).ok()?;
    let gpu_util = find_metric_deep(&root, &["utilization"], &["memory"]);
    let mem_util = find_metric_deep(&root, &["memory", "utilization"], &[]);
    let mem_used = find_metric_deep(&root, &["memory", "used"], &["utilization"]);
    let temperature = find_metric_deep(&root, &["temperature"], &[]);
    let power = find_metric_deep(&root, &["power"], &[]);

    Some(GpuMetric {
        index: device.id,
        name: device.name.clone(),
        uuid: device
            .uuid
            .clone()
            .unwrap_or_else(|| format!("intel-{}", device.id)),
        vendor: "intel".to_string(),
        driver_version: String::new(),
        power_draw_w: power,
        power_limit_w: None,
        temperature_c: temperature,
        gpu_util_percent: gpu_util,
        mem_util_percent: mem_util,
        memory_total_mi_b: None,
        memory_used_mi_b: mem_used.map(|value| value.round() as u64),
        memory_free_mi_b: None,
    })
}

/// Runs intel_gpu_top for ~2s and masks its timeout kill so the outer
/// run_remote_command wrapper sees success; stderr is merged for the
/// permission-denied diagnostic.
pub const INTEL_GPU_TOP_COMMAND: &str =
    "{ timeout -s INT 2s intel_gpu_top -J -s 500; } 2>&1 || true";

/// Extracts complete top-level `{...}` JSON objects from an intel_gpu_top
/// stream, tolerating array framing, concatenated objects, a truncated tail,
/// and interleaved stderr text.
fn extract_json_objects(stream: &str) -> Vec<Value> {
    let bytes = stream.as_bytes();
    let mut objects = Vec::new();
    let mut depth = 0usize;
    let mut start = None;
    let mut in_string = false;
    let mut escaped = false;
    for (index, &byte) in bytes.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => {
                if depth == 0 {
                    start = Some(index);
                }
                depth += 1;
            }
            b'}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    if let Some(begin) = start.take() {
                        if let Ok(value) = serde_json::from_str::<Value>(&stream[begin..=index]) {
                            objects.push(value);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    objects
}

pub fn parse_intel_gpu_top_stream(output: &str, index: u32) -> Result<GpuMetric, String> {
    let objects = extract_json_objects(output);
    let sample = objects.last().ok_or_else(|| {
        let mut message = String::from(
            "intel_gpu_top produced no samples — it usually requires root or CAP_PERFMON (sudo setcap cap_perfmon=+ep $(which intel_gpu_top))",
        );
        let trimmed = output.trim();
        if !trimmed.is_empty() {
            message.push_str(&format!(": {}", trimmed.lines().next().unwrap_or("")));
        }
        message
    })?;

    // Utilization: the Render/3D engine's busy percentage.
    let gpu_util = sample
        .get("engines")
        .and_then(Value::as_object)
        .and_then(|engines| {
            engines
                .iter()
                .find(|(key, _)| key.starts_with("Render/3D"))
                .and_then(|(_, engine)| engine.get("busy"))
                .and_then(number_from_value)
        });
    let power = sample.get("power").and_then(|power| match power {
        Value::Object(map) => map.get("GPU").and_then(number_from_value),
        other => number_from_value(other),
    });

    Ok(GpuMetric {
        index,
        name: "Intel integrated GPU".to_string(),
        uuid: "intel-igpu".to_string(),
        vendor: "intel".to_string(),
        driver_version: String::new(),
        power_draw_w: power,
        power_limit_w: None,
        temperature_c: None,
        gpu_util_percent: gpu_util,
        mem_util_percent: None,
        memory_total_mi_b: None,
        memory_used_mi_b: None,
        memory_free_mi_b: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_matches_tool_basenames() {
        let probe = parse_gpu_probe("/usr/bin/nvidia-smi\n/opt/rocm/bin/rocm-smi\nxpu-smi\n");
        assert!(probe.nvidia);
        assert!(probe.rocm_smi);
        assert!(probe.xpu_smi);
        assert!(!probe.amd_smi);
        assert!(!probe.intel_gpu_top);
        assert!(!probe.apple);
        assert!(probe.any());
    }

    #[test]
    fn probe_marks_apple_on_darwin_uname_line() {
        let probe = parse_gpu_probe("Darwin\n");
        assert!(probe.apple);
        assert!(probe.any());
        assert!(!probe.nvidia);
    }

    #[test]
    fn parses_rocm_smi_older_format() {
        let json = r#"{
            "card0": {
                "Card series": "Radeon RX 7900 XTX",
                "Unique ID": "0x1234abcd",
                "GPU use (%)": "37",
                "GPU Memory Allocated (VRAM%)": "50",
                "VRAM Total Memory (B)": "25753026560",
                "VRAM Total Used Memory (B)": "12876513280",
                "Temperature (Sensor edge) (C)": "61.0",
                "Average Graphics Package Power (W)": "210.0"
            },
            "system": { "Driver version": "6.7.0" }
        }"#;
        let gpus = parse_rocm_smi_json(json).unwrap();
        assert_eq!(gpus.len(), 1);
        let gpu = &gpus[0];
        assert_eq!(gpu.vendor, "amd");
        assert_eq!(gpu.name, "Radeon RX 7900 XTX");
        assert_eq!(gpu.uuid, "0x1234abcd");
        assert_eq!(gpu.gpu_util_percent, Some(37.0));
        assert_eq!(gpu.temperature_c, Some(61.0));
        assert_eq!(gpu.power_draw_w, Some(210.0));
        assert_eq!(gpu.memory_total_mi_b, Some(24560));
        assert_eq!(gpu.memory_used_mi_b, Some(12280));
        assert_eq!(gpu.memory_free_mi_b, Some(12280));
    }

    #[test]
    fn parses_rocm_smi_newer_format_with_numeric_values_and_uuid_fallback() {
        let json = r#"{
            "card0": {
                "Card model": "Instinct MI300X",
                "GPU use (%)": 88,
                "VRAM Total Memory (B)": 205520896000,
                "VRAM Total Used Memory (B)": 102760448000,
                "Temperature (Sensor junction) (C)": 72,
                "Current Socket Graphics Package Power (W)": 550
            }
        }"#;
        let gpus = parse_rocm_smi_json(json).unwrap();
        let gpu = &gpus[0];
        assert_eq!(gpu.name, "Instinct MI300X");
        // No Unique ID → stable synthetic uuid required for React keys.
        assert_eq!(gpu.uuid, "amd-0");
        assert_eq!(gpu.gpu_util_percent, Some(88.0));
        assert_eq!(gpu.temperature_c, Some(72.0));
        assert_eq!(gpu.power_draw_w, Some(550.0));
    }

    #[test]
    fn parses_xpu_discovery_with_string_device_id() {
        let json = r#"{"device_list":[
            {"device_id":0,"device_name":"Intel Data Center GPU Max 1550","uuid":"abc"},
            {"device_id":"1","device_name":"Intel Arc A770"}
        ]}"#;
        let devices = parse_xpu_discovery(json);
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].id, 0);
        assert_eq!(devices[0].uuid.as_deref(), Some("abc"));
        assert_eq!(devices[1].id, 1);
        assert_eq!(devices[1].uuid, None);
    }

    #[test]
    fn parses_xpu_stats_tolerantly() {
        let json = r#"{"device_level":[
            {"metrics_type":"XPUM_STATS_GPU_UTILIZATION","value":42.5},
            {"metrics_type":"XPUM_STATS_MEMORY_USED","value":8192.0},
            {"metrics_type":"XPUM_STATS_GPU_CORE_TEMPERATURE","value":58.0},
            {"metrics_type":"XPUM_STATS_POWER","value":120.0}
        ]}"#;
        let device = XpuDevice { id: 3, name: "Arc".to_string(), uuid: None };
        let gpu = parse_xpu_stats(json, &device).unwrap();
        assert_eq!(gpu.vendor, "intel");
        assert_eq!(gpu.uuid, "intel-3");
        assert_eq!(gpu.gpu_util_percent, Some(42.5));
        assert_eq!(gpu.temperature_c, Some(58.0));
        assert_eq!(gpu.power_draw_w, Some(120.0));
        assert_eq!(gpu.memory_used_mi_b, Some(8192));
    }

    #[test]
    fn parses_intel_gpu_top_array_uses_last_sample() {
        let stream = r#"[
        {"engines":{"Render/3D/0":{"busy":5.0}},"power":{"GPU":2.0}},
        {"engines":{"Render/3D/0":{"busy":63.5}},"power":{"GPU":8.5}}
        ]"#;
        let gpu = parse_intel_gpu_top_stream(stream, 0).unwrap();
        assert_eq!(gpu.vendor, "intel");
        assert_eq!(gpu.uuid, "intel-igpu");
        assert_eq!(gpu.gpu_util_percent, Some(63.5));
        assert_eq!(gpu.power_draw_w, Some(8.5));
        assert_eq!(gpu.memory_used_mi_b, None);
    }

    #[test]
    fn parses_intel_gpu_top_concatenated_and_truncated_tail() {
        // Concatenated objects (no array), with a truncated final object.
        let stream = r#"{"engines":{"Render/3D/0":{"busy":10.0}},"power":{"GPU":3.0}}
{"engines":{"Render/3D/0":{"busy":40.0}},"power":{"GPU":6.0}}
{"engines":{"Render/3D/0":{"busy":99.0"#;
        let gpu = parse_intel_gpu_top_stream(stream, 1).unwrap();
        // Last *complete* object wins; the truncated tail is ignored.
        assert_eq!(gpu.gpu_util_percent, Some(40.0));
        assert_eq!(gpu.index, 1);
    }

    #[test]
    fn intel_gpu_top_permission_error_when_no_samples() {
        let err = parse_intel_gpu_top_stream("Permission denied\n", 0).unwrap_err();
        assert!(err.contains("CAP_PERFMON"));
        assert!(err.contains("Permission denied"));
    }
}
