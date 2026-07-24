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
    /// Linux DRM sysfs is the zero-install fallback used to enumerate GPUs
    /// that do not have a vendor CLI (most commonly an Intel/AMD iGPU next to
    /// an NVIDIA dGPU).
    pub linux_drm: bool,
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
            || self.linux_drm
            || self.apple
            || !self.windows_adapters.is_empty()
    }
}

/// Shell snippet that lists which GPU tools exist. Each tool is probed with a
/// single-argument `command -v` inside a loop: dash's `command -v` (Ubuntu's
/// /bin/sh, used by run_remote_command's `sh -lc`) only inspects its first
/// operand, so a multi-argument form would silently ignore every tool after
/// the first. The trailing `true` keeps the overall exit status zero.
pub const GPU_PROBE_COMMAND: &str = "uname -s 2>/dev/null; for t in nvidia-smi rocm-smi amd-smi xpu-smi intel_gpu_top; do command -v \"$t\" 2>/dev/null; done; for p in /opt/rocm/bin/rocm-smi /opt/rocm/bin/amd-smi; do [ -x \"$p\" ] && echo \"$p\"; done; [ -d /sys/class/drm ] && echo drm-sysfs; true";

/// Emits one key/value section per physical Linux DRM card. Connector entries
/// do not have a readable PCI vendor file and are skipped. Every read is
/// best-effort so an unsupported metric becomes `None` instead of hiding the
/// integrated adapter.
pub const LINUX_DRM_GPU_COMMAND: &str = r#"for card in /sys/class/drm/card[0-9]*; do
  dev="$card/device"
  [ -r "$dev/vendor" ] || continue
  printf '__DRM_GPU__\n'
  printf 'card=%s\n' "$(basename "$card")"
  printf 'vendor=%s\n' "$(cat "$dev/vendor" 2>/dev/null)"
  printf 'device=%s\n' "$(cat "$dev/device" 2>/dev/null)"
  slot="$(sed -n 's/^PCI_SLOT_NAME=//p' "$dev/uevent" 2>/dev/null | head -n 1)"
  printf 'slot=%s\n' "$slot"
  driver="$(basename "$(readlink -f "$dev/driver" 2>/dev/null)" 2>/dev/null)"
  printf 'driver=%s\n' "$driver"
  name=''
  if [ -n "$slot" ] && command -v lspci >/dev/null 2>&1; then
    name="$(lspci -s "$slot" 2>/dev/null | sed 's/^[^ ]* //')"
  fi
  printf 'name=%s\n' "$name"
  printf 'gpu_busy_percent=%s\n' "$(cat "$dev/gpu_busy_percent" 2>/dev/null)"
  printf 'mem_busy_percent=%s\n' "$(cat "$dev/mem_busy_percent" 2>/dev/null)"
  printf 'vram_total=%s\n' "$(cat "$dev/mem_info_vram_total" 2>/dev/null)"
  printf 'vram_used=%s\n' "$(cat "$dev/mem_info_vram_used" 2>/dev/null)"
done
true"#;

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
            "drm-sysfs" => probe.linux_drm = true,
            // From the leading `uname -s`: a Darwin host exposes its Apple GPU
            // through ioreg without any dedicated tool.
            "Darwin" => probe.apple = true,
            _ => {}
        }
    }
    probe
}

fn linux_vendor(vendor_id: &str) -> Option<&'static str> {
    match vendor_id.trim().to_ascii_lowercase().as_str() {
        "0x10de" => Some("nvidia"),
        "0x1002" | "0x1022" => Some("amd"),
        "0x8086" => Some("intel"),
        _ => None,
    }
}

/// Parses [`LINUX_DRM_GPU_COMMAND`]. The caller merges these physical cards
/// with richer vendor metrics through [`append_uncovered_linux_drm`].
pub fn parse_linux_drm_gpus(output: &str, next_index: u32) -> Vec<GpuMetric> {
    output
        .split("__DRM_GPU__")
        .skip(1)
        .filter_map(|section| {
            let fields = section
                .lines()
                .filter_map(|line| line.trim().split_once('='))
                .collect::<std::collections::HashMap<_, _>>();
            let vendor = linux_vendor(fields.get("vendor").copied().unwrap_or(""))?;
            let card = fields
                .get("card")
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
                .unwrap_or("card");
            let slot = fields
                .get("slot")
                .map(|value| value.trim())
                .filter(|value| !value.is_empty());
            let device_id = fields
                .get("device")
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
                .unwrap_or("unknown");
            let name = fields
                .get("name")
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| {
                    format!(
                        "{} GPU ({})",
                        match vendor {
                            "nvidia" => "NVIDIA",
                            "amd" => "AMD",
                            "intel" => "Intel",
                            _ => "DRM",
                        },
                        device_id
                    )
                });
            let total_bytes = fields
                .get("vram_total")
                .and_then(|value| value.trim().parse::<u64>().ok());
            let used_bytes = fields
                .get("vram_used")
                .and_then(|value| value.trim().parse::<u64>().ok());
            let memory_total_mi_b = total_bytes.map(|value| value / 1024 / 1024);
            let memory_used_mi_b = used_bytes.map(|value| value / 1024 / 1024);

            Some(GpuMetric {
                index: next_index,
                name,
                uuid: format!("linux-drm-{}", slot.unwrap_or(card)),
                vendor: vendor.to_string(),
                driver_version: fields
                    .get("driver")
                    .map(|value| value.trim().to_string())
                    .unwrap_or_default(),
                power_draw_w: None,
                power_limit_w: None,
                temperature_c: None,
                gpu_util_percent: fields
                    .get("gpu_busy_percent")
                    .and_then(|value| parse_optional_f64(value)),
                mem_util_percent: fields
                    .get("mem_busy_percent")
                    .and_then(|value| parse_optional_f64(value)),
                memory_total_mi_b,
                memory_used_mi_b,
                memory_free_mi_b: match (memory_total_mi_b, memory_used_mi_b) {
                    (Some(total), Some(used)) => Some(total.saturating_sub(used)),
                    _ => None,
                },
            })
        })
        .enumerate()
        .map(|(offset, mut metric)| {
            metric.index = next_index + offset as u32;
            metric
        })
        .collect()
}

/// Appends only physical DRM cards that are not already represented by a
/// richer vendor collector. Card counts handle same-vendor hybrid systems
/// (for example an AMD iGPU plus AMD dGPU); when only some same-vendor cards
/// are uncovered, the shared/smaller-memory cards are preferred because
/// vendor CLIs generally cover the discrete adapter.
pub fn append_uncovered_linux_drm(metrics: &mut Vec<GpuMetric>, drm: Vec<GpuMetric>) {
    let mut appended = Vec::new();
    for vendor in ["nvidia", "amd", "intel"] {
        let covered_count = metrics
            .iter()
            .filter(|metric| metric.vendor == vendor)
            .count();
        let mut cards = drm
            .iter()
            .filter(|metric| metric.vendor == vendor)
            .cloned()
            .collect::<Vec<_>>();
        let uncovered_count = cards.len().saturating_sub(covered_count);
        cards.sort_by_key(|metric| metric.memory_total_mi_b.unwrap_or(0));
        appended.extend(cards.into_iter().take(uncovered_count));
    }

    let next_index = metrics
        .iter()
        .map(|metric| metric.index + 1)
        .max()
        .unwrap_or(0);
    for (offset, mut metric) in appended.into_iter().enumerate() {
        metric.index = next_index + offset as u32;
        metrics.push(metric);
    }
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
    fn probe_marks_linux_drm_fallback() {
        let probe = parse_gpu_probe("Linux\ndrm-sysfs\n");
        assert!(probe.linux_drm);
        assert!(probe.any());
    }

    #[test]
    fn parses_uncovered_integrated_gpu_from_linux_drm() {
        let output = r#"__DRM_GPU__
card=card0
vendor=0x8086
device=0x46a6
slot=0000:00:02.0
driver=i915
name=VGA compatible controller: Intel Corporation Alder Lake-P Integrated Graphics
gpu_busy_percent=17
mem_busy_percent=
vram_total=
vram_used=
__DRM_GPU__
card=card1
vendor=0x10de
device=0x2684
slot=0000:01:00.0
driver=nvidia
name=3D controller: NVIDIA Corporation AD104
gpu_busy_percent=42
mem_busy_percent=
vram_total=8589934592
vram_used=2147483648
"#;
        let mut gpus = vec![GpuMetric {
            index: 0,
            name: "NVIDIA AD104".to_string(),
            uuid: "GPU-rich".to_string(),
            vendor: "nvidia".to_string(),
            driver_version: "555".to_string(),
            power_draw_w: None,
            power_limit_w: None,
            temperature_c: None,
            gpu_util_percent: Some(42.0),
            mem_util_percent: None,
            memory_total_mi_b: Some(8192),
            memory_used_mi_b: Some(2048),
            memory_free_mi_b: Some(6144),
        }];
        append_uncovered_linux_drm(&mut gpus, parse_linux_drm_gpus(output, 0));
        assert_eq!(gpus.len(), 2);
        let integrated = &gpus[1];
        assert_eq!(integrated.index, 1);
        assert_eq!(integrated.vendor, "intel");
        assert_eq!(integrated.uuid, "linux-drm-0000:00:02.0");
        assert_eq!(integrated.gpu_util_percent, Some(17.0));
        assert_eq!(integrated.driver_version, "i915");
    }

    #[test]
    fn keeps_same_vendor_integrated_card_missing_from_vendor_tool() {
        let rich_output = r#"{"card1":{"Card series":"AMD Radeon RX 7900","Unique ID":"dGPU","GPU use (%)":"20","VRAM Total Memory (B)":"8589934592","VRAM Total Used Memory (B)":"1073741824"}}"#;
        let mut gpus = parse_rocm_smi_json(rich_output).unwrap();
        let drm_output = r#"__DRM_GPU__
card=card0
vendor=0x1002
device=0x164e
slot=0000:05:00.0
driver=amdgpu
name=AMD Radeon Integrated Graphics
gpu_busy_percent=7
mem_busy_percent=
vram_total=536870912
vram_used=67108864
__DRM_GPU__
card=card1
vendor=0x1002
device=0x744c
slot=0000:03:00.0
driver=amdgpu
name=AMD Radeon RX 7900
gpu_busy_percent=20
mem_busy_percent=
vram_total=8589934592
vram_used=1073741824
"#;
        append_uncovered_linux_drm(&mut gpus, parse_linux_drm_gpus(drm_output, 0));
        assert_eq!(gpus.len(), 2);
        assert_eq!(gpus[1].name, "AMD Radeon Integrated Graphics");
        assert_eq!(gpus[1].memory_total_mi_b, Some(512));
    }

    #[test]
    fn linux_drm_reports_amd_vram_in_mib() {
        let output = "__DRM_GPU__\ncard=card0\nvendor=0x1002\ndevice=0x164e\nslot=0000:05:00.0\ndriver=amdgpu\nname=\ngpu_busy_percent=9\nmem_busy_percent=3\nvram_total=4294967296\nvram_used=1073741824\n";
        let gpus = parse_linux_drm_gpus(output, 0);
        assert_eq!(gpus[0].vendor, "amd");
        assert_eq!(gpus[0].memory_total_mi_b, Some(4096));
        assert_eq!(gpus[0].memory_used_mi_b, Some(1024));
        assert_eq!(gpus[0].memory_free_mi_b, Some(3072));
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
