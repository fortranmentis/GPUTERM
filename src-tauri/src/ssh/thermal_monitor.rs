//! Temperature monitoring for CPU packages/dies, DIMMs, and NVMe/SATA drives.
//!
//! Availability is wildly uneven across platforms, and that unevenness is why
//! this module has the shape it does. Linux exposes everything unprivileged
//! through hwmon; Windows exposes an ambiguous ACPI zone plus SMART drive
//! temperatures; macOS exposes nothing at all without root. A category the host
//! cannot report is named in `ThermalMetric::unsupported` **with the reason**,
//! so the UI can say "not supported" instead of showing a blank that reads like
//! a bug — and so it never has to invent a number.
//!
//! Three states must stay distinguishable all the way to the screen:
//!   * `RemoteTelemetry::thermal == None` — not read yet.
//!   * a `ThermalGroup` with readings — measured.
//!   * a `ThermalUnsupported` entry — this host genuinely cannot report it.

use serde::Serialize;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::parse_util::split_sections;

/// Readings outside this range are sensor artifacts rather than measurements:
/// hwmon reports 0 for a present-but-uninitialized sensor, NVMe reports 0 K for
/// an unimplemented thermal sensor, and stubbed ACPI firmware returns exactly
/// 2732 tenths-of-Kelvin (0.05 C). Nothing under power measures below 1 C.
///
/// This does discard a genuine sub-1 C reading from a cold room. That trade is
/// deliberate: a fabricated "0 C" is worse than an honest `n/a`.
const PLAUSIBLE_CELSIUS: std::ops::RangeInclusive<f64> = 1.0..=150.0;

/// Shown next to any reading that is not a die/junction temperature.
///
/// Windows `MSAcpi_ThermalZoneTemperature` and Linux `acpitz` frequently
/// measure chassis or skin rather than the die, so this is rendered beside the
/// value and such readings are never colored.
pub const ACPI_ZONE_CAVEAT: &str =
    "ACPI thermal zone, not the CPU die — often a chassis or skin sensor";

pub const CATEGORY_CPU: &str = "cpu";
pub const CATEGORY_MEMORY: &str = "memory";
pub const CATEGORY_DISK: &str = "disk";

// ---------------------------------------------------------------------------
// Serialized shapes
// ---------------------------------------------------------------------------

/// One temperature-reporting device.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThermalSensor {
    /// Shown in the UI: "Package id 0", "Core 3", "Tdie", "DIMM 0x18",
    /// "nvme0 Composite", "ACPI\\ThermalZone\\TZ00_0".
    pub label: String,
    /// The kernel driver or provider, so the UI and a bug report can name the
    /// source exactly rather than implying every reading is equally trustworthy.
    pub source: String,
    /// `None` when the sensor exists but produced no plausible value. Never a
    /// sentinel and never zero-filled — the same rule as
    /// `GpuMetric::temperature_c`.
    pub temperature_c: Option<f64>,
    /// Vendor "high" threshold (`tempN_max`, NVMe WCTEMP, SMART
    /// TemperatureMax). `None` is normal, not an error.
    pub high_c: Option<f64>,
    /// Vendor critical threshold (`tempN_crit`, Intel TjMax, NVMe CCTEMP).
    pub critical_c: Option<f64>,
}

/// Every sensor in one category on one host, plus the single reading the
/// compact bar card shows.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThermalGroup {
    pub headline_c: Option<f64>,
    /// Which sensor the headline came from, or how it was chosen ("hottest of
    /// 16 cores"), so the number is never unattributed.
    pub headline_label: Option<String>,
    /// Set only when the reading is not a die temperature. See
    /// `ACPI_ZONE_CAVEAT`.
    pub caveat: Option<String>,
    pub sensors: Vec<ThermalSensor>,
}

/// A category this host genuinely cannot report, with the reason named.
///
/// The reason travels inline rather than as a code the UI maps, because the
/// *why* differs per OS and per host (no driver loaded vs. no interface in the
/// OS vs. needs root) and a static UI table could not know which applies.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThermalUnsupported {
    /// `cpu` | `memory` | `disk`.
    pub category: String,
    pub reason: String,
}

impl ThermalUnsupported {
    fn new(category: &str, reason: impl Into<String>) -> Self {
        Self {
            category: category.to_string(),
            reason: reason.into(),
        }
    }
}

/// Temperatures for one host.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThermalMetric {
    pub cpu: Option<ThermalGroup>,
    pub memory: Option<ThermalGroup>,
    pub disk: Option<ThermalGroup>,
    /// Always serialized, even when empty: the telemetry TypeScript mirror uses
    /// non-optional keys with nullable or array values.
    pub unsupported: Vec<ThermalUnsupported>,
}

impl ThermalMetric {
    fn push_unsupported(&mut self, category: &str, reason: impl Into<String>) {
        self.unsupported
            .push(ThermalUnsupported::new(category, reason));
    }
}

// ---------------------------------------------------------------------------
// Classification
// ---------------------------------------------------------------------------

/// Which category a temperature source belongs to.
///
/// `Other` is a real answer, not a failure. GPU dies are already covered by
/// `gpu_monitor` and must not leak into the CPU group; chipset, WiFi, battery,
/// and super-I/O board sensors are excluded because attributing them would be a
/// guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThermalCategory {
    Cpu,
    Memory,
    Disk,
    /// An ACPI thermal zone. Only promoted to the CPU headline when no true CPU
    /// sensor exists, and then it keeps its own `source` tag plus a caveat.
    AcpiZone,
    Other,
}

/// hwmon driver names that report a CPU package or die temperature.
const CPU_HWMON_NAMES: &[&str] = &[
    "coretemp",
    "k10temp",
    "zenpower",
    "k8temp",
    "via_cputemp",
];

/// hwmon driver names for JEDEC DIMM thermal sensors.
const MEMORY_HWMON_NAMES: &[&str] = &["jc42", "spd5118"];

/// hwmon driver names for drive thermal sensors.
const DISK_HWMON_NAMES: &[&str] = &["nvme", "drivetemp"];

/// `/sys/class/thermal/thermal_zone*/type` values that name a CPU.
const CPU_ZONE_TYPES: &[&str] = &["x86_pkg_temp", "cpu-thermal", "cpu_thermal", "soc_thermal"];

/// Maps an hwmon `name` or a thermal-zone `type` to a category.
///
/// Pure, total, and label-independent: every `tempN_*` under one hwmon device
/// belongs to that device, so the driver name alone decides the category, and
/// labels only decide which reading becomes the headline.
pub(crate) fn classify_thermal_source(source_name: &str) -> ThermalCategory {
    let name = source_name.trim();
    if name.is_empty() {
        return ThermalCategory::Other;
    }
    let matches = |candidates: &[&str]| {
        candidates
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(name))
    };

    if matches(CPU_HWMON_NAMES) || matches(CPU_ZONE_TYPES) {
        return ThermalCategory::Cpu;
    }
    if matches(MEMORY_HWMON_NAMES) {
        return ThermalCategory::Memory;
    }
    if matches(DISK_HWMON_NAMES) {
        return ThermalCategory::Disk;
    }
    if name.eq_ignore_ascii_case("acpitz") {
        return ThermalCategory::AcpiZone;
    }

    // Per-core zone types on ARM and some x86: cpu0-thermal, cpu-thermal-0.
    let lowered = name.to_ascii_lowercase();
    if lowered.starts_with("cpu") && (lowered.contains("-thermal") || lowered.contains("_thermal"))
    {
        return ThermalCategory::Cpu;
    }

    ThermalCategory::Other
}

/// CPU package/die labels, most authoritative first.
///
/// `Tdie` precedes `Tctl` deliberately: `Tctl` carries a vendor control offset
/// (+27 C on several Ryzen and Threadripper SKUs) while `Tdie` is the real
/// junction temperature, so preferring `Tctl` would over-report by up to 27 C
/// on the hosts that expose both.
const CPU_HEADLINE_TIERS: &[&str] = &["Package id", "Tdie", "Tctl", "Tccd"];

/// Chooses the one reading the compact bar card shows.
///
/// CPU walks `CPU_HEADLINE_TIERS` and takes the hottest within the first tier
/// that has a reading (a multi-socket host has `Package id 0` and
/// `Package id 1`), then falls back to the hottest `Core *`, then to the hottest
/// sensor in the group. Memory and disk take the hottest device outright: one
/// hot DIMM or one throttling drive is the fact that matters, and an average
/// would hide it.
///
/// `None` when no sensor had a plausible reading — the caller turns that into an
/// `unsupported` entry rather than a group with a blank.
pub(crate) fn choose_headline(
    category: ThermalCategory,
    sensors: &[ThermalSensor],
) -> Option<(f64, String)> {
    let hottest = |subset: Vec<&ThermalSensor>| -> Option<(f64, String)> {
        subset
            .into_iter()
            .filter_map(|sensor| sensor.temperature_c.map(|value| (value, sensor)))
            .max_by(|left, right| left.0.total_cmp(&right.0))
            .map(|(value, sensor)| (value, sensor.label.clone()))
    };

    if category == ThermalCategory::Cpu {
        for tier in CPU_HEADLINE_TIERS {
            let tier_sensors: Vec<&ThermalSensor> = sensors
                .iter()
                .filter(|sensor| sensor.label.starts_with(tier))
                .collect();
            if let Some(headline) = hottest(tier_sensors) {
                return Some(headline);
            }
        }

        let cores: Vec<&ThermalSensor> = sensors
            .iter()
            .filter(|sensor| sensor.label.starts_with("Core "))
            .collect();
        let core_count = cores
            .iter()
            .filter(|sensor| sensor.temperature_c.is_some())
            .count();
        if let Some((value, _)) = hottest(cores) {
            // The label describes the choice, not one sensor, so nobody reads
            // "Core 7" as though only that core were being reported.
            return Some((value, format!("hottest of {} cores", core_count)));
        }
    }

    hottest(sensors.iter().collect())
}

/// Turns a category's sensors into a group, or into a named unsupported reason.
///
/// A group exists only when at least one sensor produced a reading. An
/// interface that is present but silent becomes `Err`, so it reads differently
/// on screen from a category that was never read at all.
fn finalize_group(
    category: ThermalCategory,
    sensors: Vec<ThermalSensor>,
    caveat: Option<String>,
    silent_reason: &str,
    absent_reason: &str,
) -> Result<ThermalGroup, String> {
    if sensors.is_empty() {
        return Err(absent_reason.to_string());
    }
    let Some((headline_c, headline_label)) = choose_headline(category, &sensors) else {
        return Err(silent_reason.to_string());
    };
    Ok(ThermalGroup {
        headline_c: Some(headline_c),
        headline_label: Some(headline_label),
        caveat,
        sensors,
    })
}

fn plausible(celsius: f64) -> Option<f64> {
    PLAUSIBLE_CELSIUS.contains(&celsius).then_some(celsius)
}

/// hwmon reports millidegrees.
fn millidegrees_to_celsius(raw: &str) -> Option<f64> {
    plausible(raw.trim().parse::<f64>().ok()? / 1000.0)
}

// ---------------------------------------------------------------------------
// Reason strings
// ---------------------------------------------------------------------------

/// The whole feature on the majority of hosts is this text, so it names the
/// missing interface and, where there is one, the action that would fix it.
const NO_SYSFS_AT_ALL: &str = "This host exposes no /sys/class/hwmon or /sys/class/thermal entries at all — typical of WSL2 and most containers — so no temperature of any kind can be read.";
const NO_CPU_SOURCE: &str = "No CPU temperature source: this host has no coretemp/k10temp/zenpower hwmon device and no CPU thermal zone. Typical of WSL2, containers, and most VMs, which do not pass hardware sensors through.";
const CPU_SILENT: &str = "A CPU thermal sensor exists but every reading was implausible, so there is no usable value.";
const NO_MEMORY_SOURCE: &str = "No DIMM temperature source: needs the jc42 module (DDR3/DDR4 ECC) or spd5118 (DDR5, kernel 6.10+), and the platform must expose the SPD bus. Most non-ECC consumer boards never do.";
const MEMORY_SILENT: &str = "A DIMM thermal sensor exists but every reading was implausible, so there is no usable value.";
const NO_DISK_SOURCE: &str = "No drive temperature source: needs the nvme driver's hwmon sensor, or `modprobe drivetemp` for SATA/SAS SMART.";
const DISK_SILENT: &str = "A drive thermal sensor exists but every reading was implausible, so there is no usable value.";

// ---------------------------------------------------------------------------
// Linux
// ---------------------------------------------------------------------------

/// Sensor inventory, read once per connection.
///
/// `grep -H ''` prints `path:value` for every match in one process, which is
/// cheaper and far easier to parse than `head -v`'s banner format. Each glob is
/// followed by `|| true` because grep exits 2 when a glob matches nothing —
/// which is the normal WSL2 and container case, not a failure.
pub(crate) const LINUX_THERMAL_PROBE_COMMAND: &str = "printf '__HWMON_NAME__\\n'; grep -H '' /sys/class/hwmon/hwmon*/name 2>/dev/null || true; printf '\\n__HWMON_LABEL__\\n'; grep -H '' /sys/class/hwmon/hwmon*/temp*_label 2>/dev/null || true; printf '\\n__HWMON_MAX__\\n'; grep -H '' /sys/class/hwmon/hwmon*/temp*_max 2>/dev/null || true; printf '\\n__HWMON_CRIT__\\n'; grep -H '' /sys/class/hwmon/hwmon*/temp*_crit 2>/dev/null || true; printf '\\n__HWMON_INPUT__\\n'; ls -1 /sys/class/hwmon/hwmon*/temp*_input 2>/dev/null || true; printf '\\n__HWMON_REALPATH__\\n'; for dir in /sys/class/hwmon/hwmon*; do [ -d \"$dir\" ] && printf '%s:%s\\n' \"$dir\" \"$(readlink -f \"$dir\" 2>/dev/null)\"; done 2>/dev/null || true; printf '\\n__ZONE_TYPE__\\n'; grep -H '' /sys/class/thermal/thermal_zone*/type 2>/dev/null || true";

/// Values only, read every tick.
pub(crate) const LINUX_THERMAL_VALUE_COMMAND: &str = "printf '__HWMON_INPUT__\\n'; grep -H '' /sys/class/hwmon/hwmon*/temp*_input 2>/dev/null || true; printf '\\n__ZONE_TEMP__\\n'; grep -H '' /sys/class/thermal/thermal_zone*/temp 2>/dev/null || true";

/// One discovered sensor file and everything static about it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ThermalSensorPath {
    /// The `tempN_input` (hwmon) or zone `temp` path the value command reports.
    pub(crate) value_path: String,
    pub(crate) label: String,
    pub(crate) source: String,
    pub(crate) category: ThermalCategory,
    pub(crate) high_c: Option<f64>,
    pub(crate) critical_c: Option<f64>,
}

/// Discovered temperature sources for one host, cached per connection.
///
/// Also holds the Windows drive-temperature throttle so a single
/// `&mut Option<ThermalProbe>` threads through every collector regardless of OS.
#[derive(Debug, Clone, Default)]
pub(crate) struct ThermalProbe {
    pub(crate) sensors: Vec<ThermalSensorPath>,
    /// True when the host has neither hwmon nor thermal-zone entries, which
    /// deserves a different message from "the driver is not loaded".
    pub(crate) sysfs_empty: bool,
    /// Windows only: when the drive-temperature command last ran.
    disk_read_at: Option<Instant>,
    /// Windows only: the last drive outcome, reused between refreshes. A failed
    /// refresh keeps the previous reading rather than zeroing it.
    disk_outcome: Option<Result<ThermalGroup, String>>,
}

/// Splits `grep -H ''` output into `(path, value)` pairs.
///
/// A sysfs value can itself contain a colon, so this splits on the first one
/// only.
fn grep_pairs(section: &str) -> Vec<(String, String)> {
    section
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let (path, value) = line.split_once(':')?;
            Some((path.trim().to_string(), value.trim().to_string()))
        })
        .collect()
}

/// `/sys/class/hwmon/hwmon3/temp1_input` -> `/sys/class/hwmon/hwmon3`
fn parent_dir(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(parent, _)| parent.to_string())
        .unwrap_or_default()
}

/// `/sys/class/hwmon/hwmon3/temp1_input` -> `temp1`
fn temp_prefix(path: &str) -> Option<String> {
    let file = path.rsplit('/').next()?;
    let stem = file.strip_suffix("_input")?;
    Some(stem.to_string())
}

/// Derives a DIMM label from the i2c bus address in the device's realpath.
///
/// `jc42` and `spd5118` export no `tempN_label`. The realpath contains the
/// `<bus>-<addr>` component (`…/i2c-0/0-0018/hwmon/hwmon4`), so this yields
/// `DIMM 0x18`. Honest but limited: it is the SPD address, not a silkscreened
/// slot name, and the docs say so.
fn dimm_label(realpath: &str) -> Option<String> {
    for component in realpath.split('/') {
        // `continue`, not `?`: most components have no `-` at all (starting with
        // the empty one before the leading slash), and bailing on the first of
        // those would never reach the `<bus>-<addr>` component.
        let Some((_, addr)) = component.split_once('-') else {
            continue;
        };
        if addr.len() == 4 && addr.chars().all(|c| c.is_ascii_hexdigit()) {
            let trimmed = addr.trim_start_matches('0');
            return Some(format!(
                "DIMM 0x{}",
                if trimmed.is_empty() { "0" } else { trimmed }
            ));
        }
    }
    None
}

/// Derives a drive label from the device name in the realpath.
fn drive_label(realpath: &str, hwmon_dir: &str) -> String {
    for component in realpath.split('/') {
        let is_nvme = component.starts_with("nvme")
            && component.len() > 4
            && component[4..].chars().all(|c| c.is_ascii_digit());
        let is_sd = component.starts_with("sd")
            && component.len() > 2
            && component[2..].chars().all(|c| c.is_ascii_lowercase());
        if is_nvme || is_sd {
            return component.to_string();
        }
    }
    // The common `drivetemp` case: the realpath ends at the SCSI target with no
    // block-device name. Ugly but true — no invented device name.
    format!(
        "drive ({})",
        hwmon_dir.rsplit('/').next().unwrap_or(hwmon_dir)
    )
}

/// Builds the sensor inventory. Never fails: a host with no sensors yields an
/// empty list plus `sysfs_empty`, which is what lets the UI say "not supported"
/// rather than "n/a".
pub(crate) fn parse_linux_thermal_probe(output: &str) -> ThermalProbe {
    let sections = split_sections(output);
    let get = |name: &str| sections.get(name).map(String::as_str).unwrap_or_default();

    let names: HashMap<String, String> = grep_pairs(get("HWMON_NAME"))
        .into_iter()
        .map(|(path, value)| (parent_dir(&path), value))
        .collect();
    let realpaths: HashMap<String, String> = grep_pairs(get("HWMON_REALPATH"))
        .into_iter()
        .collect();
    let zone_types: Vec<(String, String)> = grep_pairs(get("ZONE_TYPE"));

    // Keyed by `<hwmon dir>/<tempN>` so a label, max, and crit find each other.
    let keyed = |section: &str| -> HashMap<String, String> {
        grep_pairs(section)
            .into_iter()
            .filter_map(|(path, value)| {
                let dir = parent_dir(&path);
                let file = path.rsplit('/').next()?;
                let stem = file
                    .strip_suffix("_label")
                    .or_else(|| file.strip_suffix("_max"))
                    .or_else(|| file.strip_suffix("_crit"))?;
                Some((format!("{}/{}", dir, stem), value))
            })
            .collect()
    };
    let labels = keyed(get("HWMON_LABEL"));
    let maxima = keyed(get("HWMON_MAX"));
    let criticals = keyed(get("HWMON_CRIT"));

    let mut sensors = Vec::new();

    for line in get("HWMON_INPUT").lines() {
        let value_path = line.trim();
        if value_path.is_empty() {
            continue;
        }
        let dir = parent_dir(value_path);
        let Some(prefix) = temp_prefix(value_path) else {
            continue;
        };
        let key = format!("{}/{}", dir, prefix);
        let Some(name) = names.get(&dir) else {
            continue;
        };
        let category = classify_thermal_source(name);
        if category == ThermalCategory::Other {
            continue;
        }
        let realpath = realpaths.get(&dir).cloned().unwrap_or_default();

        let label = match labels.get(&key) {
            Some(label) if !label.is_empty() => label.clone(),
            _ => match category {
                ThermalCategory::Memory => {
                    dimm_label(&realpath).unwrap_or_else(|| format!("DIMM ({})", prefix))
                }
                ThermalCategory::Disk => drive_label(&realpath, &dir),
                // An unlabeled CPU sensor is rare; name the file so the value
                // is still attributable.
                _ => format!("{} {}", name, prefix),
            },
        };

        sensors.push(ThermalSensorPath {
            value_path: value_path.to_string(),
            label,
            source: if category == ThermalCategory::AcpiZone {
                "acpi_thermal_zone".to_string()
            } else {
                name.clone()
            },
            category,
            high_c: maxima.get(&key).and_then(|v| millidegrees_to_celsius(v)),
            critical_c: criticals.get(&key).and_then(|v| millidegrees_to_celsius(v)),
        });
    }

    for (path, zone_type) in zone_types {
        let category = classify_thermal_source(&zone_type);
        if !matches!(category, ThermalCategory::Cpu | ThermalCategory::AcpiZone) {
            continue;
        }
        sensors.push(ThermalSensorPath {
            value_path: format!("{}/temp", parent_dir(&path)),
            label: zone_type.clone(),
            source: if category == ThermalCategory::AcpiZone {
                "acpi_thermal_zone".to_string()
            } else {
                "thermal_zone".to_string()
            },
            category,
            high_c: None,
            critical_c: None,
        });
    }

    let sysfs_empty = names.is_empty() && zone_types_empty(get("ZONE_TYPE"));
    ThermalProbe {
        sensors,
        sysfs_empty,
        ..ThermalProbe::default()
    }
}

fn zone_types_empty(section: &str) -> bool {
    grep_pairs(section).is_empty()
}

/// Folds per-tick values onto the probe.
pub(crate) fn parse_linux_thermal_output(probe: &ThermalProbe, output: &str) -> ThermalMetric {
    let sections = split_sections(output);
    let mut values: HashMap<String, String> = HashMap::new();
    for name in ["HWMON_INPUT", "ZONE_TEMP"] {
        if let Some(section) = sections.get(name) {
            values.extend(grep_pairs(section));
        }
    }

    let mut cpu = Vec::new();
    let mut acpi = Vec::new();
    let mut memory = Vec::new();
    let mut disk = Vec::new();

    for path in &probe.sensors {
        let sensor = ThermalSensor {
            label: path.label.clone(),
            source: path.source.clone(),
            temperature_c: values
                .get(&path.value_path)
                .and_then(|raw| millidegrees_to_celsius(raw)),
            high_c: path.high_c,
            critical_c: path.critical_c,
        };
        match path.category {
            ThermalCategory::Cpu => cpu.push(sensor),
            ThermalCategory::AcpiZone => acpi.push(sensor),
            ThermalCategory::Memory => memory.push(sensor),
            ThermalCategory::Disk => disk.push(sensor),
            ThermalCategory::Other => {}
        }
    }

    let mut metric = ThermalMetric::default();

    // An ACPI zone stands in for the CPU only when no true CPU sensor exists,
    // and then it carries the caveat so nobody reads a chassis sensor as a die
    // temperature.
    let (cpu_sensors, cpu_caveat) = if cpu.is_empty() {
        (acpi, Some(ACPI_ZONE_CAVEAT.to_string()))
    } else {
        (cpu, None)
    };

    let absent_cpu = if probe.sysfs_empty {
        NO_SYSFS_AT_ALL
    } else {
        NO_CPU_SOURCE
    };
    match finalize_group(
        ThermalCategory::Cpu,
        cpu_sensors,
        cpu_caveat,
        CPU_SILENT,
        absent_cpu,
    ) {
        Ok(group) => metric.cpu = Some(group),
        Err(reason) => metric.push_unsupported(CATEGORY_CPU, reason),
    }

    let absent_memory = if probe.sysfs_empty {
        NO_SYSFS_AT_ALL
    } else {
        NO_MEMORY_SOURCE
    };
    match finalize_group(
        ThermalCategory::Memory,
        memory,
        None,
        MEMORY_SILENT,
        absent_memory,
    ) {
        Ok(group) => metric.memory = Some(group),
        Err(reason) => metric.push_unsupported(CATEGORY_MEMORY, reason),
    }

    let absent_disk = if probe.sysfs_empty {
        NO_SYSFS_AT_ALL
    } else {
        NO_DISK_SOURCE
    };
    match finalize_group(ThermalCategory::Disk, disk, None, DISK_SILENT, absent_disk) {
        Ok(group) => metric.disk = Some(group),
        Err(reason) => metric.push_unsupported(CATEGORY_DISK, reason),
    }

    metric
}

/// Probes the host's sensor inventory once, then reads values every tick — the
/// same shape as `collect_gpu_metrics`.
///
/// `Err` is reserved for a command that actually failed or timed out. A host
/// with no sensors returns `Ok` with every category unsupported, because that is
/// an answer rather than a failure.
pub(crate) fn collect_linux_thermal<F>(
    probe: &mut Option<ThermalProbe>,
    run: F,
) -> Result<ThermalMetric, String>
where
    F: Fn(&str) -> Result<String, String>,
{
    if probe.is_none() {
        *probe = Some(parse_linux_thermal_probe(&run(
            LINUX_THERMAL_PROBE_COMMAND,
        )?));
    }
    let discovered = probe.as_ref().expect("probe populated above");

    // Nothing to read: skip the per-tick command entirely rather than spending
    // a round trip to confirm the host still has no sensors.
    if discovered.sensors.is_empty() {
        return Ok(parse_linux_thermal_output(discovered, ""));
    }

    let output = run(LINUX_THERMAL_VALUE_COMMAND)?;
    Ok(parse_linux_thermal_output(discovered, &output))
}

// ---------------------------------------------------------------------------
// macOS
// ---------------------------------------------------------------------------

/// macOS has no unprivileged command-line temperature source.
///
/// Verified on Apple Silicon rather than assumed: `powermetrics` rejects the
/// `smc` sampler (an Intel-era sampler) and requires root regardless;
/// `machdep.xcpm.*` sysctl oids do not exist; and `ioreg -c AppleSMC` exposes
/// zero temperature keys to a normal user. The absence of an interface cannot
/// be asserted by a test, so this comment records what was tried.
///
/// Returns without running any command — there is nothing to ask.
pub(crate) fn macos_thermal_unsupported() -> ThermalMetric {
    const REASON: &str = "macOS exposes no unprivileged temperature interface: powermetrics requires root (and its smc sampler does not exist on Apple Silicon), and IOKit SMC sensors are not readable from a shell.";
    ThermalMetric {
        cpu: None,
        memory: None,
        disk: None,
        unsupported: vec![
            ThermalUnsupported::new(CATEGORY_CPU, REASON),
            ThermalUnsupported::new(CATEGORY_MEMORY, REASON),
            ThermalUnsupported::new(CATEGORY_DISK, REASON),
        ],
    }
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

/// Drive temperatures, deliberately **not** part of `WINDOWS_TELEMETRY_COMMAND`.
///
/// `GetReliabilityCounter` queries each device through the storage stack, and a
/// sleeping or unresponsive drive can take seconds. Inside the batched script
/// that latency would fail CPU, memory, disk, and users at once — which is
/// precisely the state the reconnect heuristic reads as a dead transport
/// (`system_monitor::telemetry_all_failed`). Kept separate, a slow storage stack
/// costs one nullable temperature and nothing else.
///
/// `MSFT_PhysicalDisk` + `GetReliabilityCounter` is the CIM path underneath the
/// `Get-PhysicalDisk | Get-StorageReliabilityCounter` cmdlet pair; it is used
/// here because it accepts `-OperationTimeoutSec`, giving a real per-call bound
/// the cmdlets do not offer.
pub(crate) const WINDOWS_DISK_TEMP_COMMAND: &str = r#"$ErrorActionPreference='SilentlyContinue'
Write-Output '__DISKTEMP__'
Get-CimInstance -Namespace root\Microsoft\Windows\Storage -ClassName MSFT_PhysicalDisk -OperationTimeoutSec 2 | ForEach-Object { $c = (Invoke-CimMethod -InputObject $_ -MethodName GetReliabilityCounter -OperationTimeoutSec 2).ReliabilityCounter; if ($c -and $c.Temperature) { [pscustomobject]@{ Device=$_.FriendlyName; Number=$_.DeviceId; Temperature=$c.Temperature; TemperatureMax=$c.TemperatureMax } } } | ConvertTo-Json -Compress
exit 0"#;

/// A SMART / storage-stack read on every telemetry tick would be gratuitous at
/// a one-to-two second interval, and on spinning media it defeats power
/// management by keeping the drive awake. Thirty seconds is far finer than any
/// thermal time constant that matters for a drive.
pub(crate) const DISK_THERMAL_MIN_INTERVAL_SECS: u64 = 30;

const WINDOWS_NO_CPU_SOURCE: &str = "Windows exposes no CPU temperature interface: root\\WMI MSAcpi_ThermalZoneTemperature is not implemented by this firmware, which is the common case on OEM desktops and laptops. There is no unprivileged way to read the CPU die on Windows.";
const WINDOWS_CPU_STUBBED: &str = "root\\WMI MSAcpi_ThermalZoneTemperature exists but every zone returned a stub value, so there is no usable reading.";
const WINDOWS_NO_MEMORY_SOURCE: &str = "Windows exposes no DIMM temperature interface: SPD/SMBus thermal sensors are not surfaced through WMI or CIM, and reading them directly requires a kernel-mode driver.";
const WINDOWS_NO_DISK_SOURCE: &str = "No drive temperature: GetReliabilityCounter returned no Temperature for any physical disk. Common for USB-bridged drives and RAID-backed volumes, which do not pass SMART through.";

/// `MSAcpi_ThermalZoneTemperature.CurrentTemperature` is in tenths of a Kelvin.
///
/// Firmware that stubs the class returns 0 or exactly 2732 (0.05 C); neither is
/// a measurement, so both fall outside `PLAUSIBLE_CELSIUS`.
pub(crate) fn acpi_tenths_kelvin_to_celsius(value: f64) -> Option<f64> {
    plausible(value / 10.0 - 273.15)
}

/// Builds the CPU and memory halves of a Windows `ThermalMetric`.
///
/// The reading is an ACPI thermal zone, so it is tagged `acpi_thermal_zone` and
/// always carries `ACPI_ZONE_CAVEAT`: it is frequently a chassis or skin sensor
/// and must never be presented as a die temperature.
pub(crate) fn windows_thermal_from_zones(zones: Vec<(String, f64)>) -> ThermalMetric {
    let mut metric = ThermalMetric::default();

    let sensors: Vec<ThermalSensor> = zones
        .into_iter()
        .map(|(instance_name, tenths_kelvin)| ThermalSensor {
            label: instance_name,
            source: "acpi_thermal_zone".to_string(),
            temperature_c: acpi_tenths_kelvin_to_celsius(tenths_kelvin),
            high_c: None,
            critical_c: None,
        })
        .collect();

    match finalize_group(
        ThermalCategory::AcpiZone,
        sensors,
        Some(ACPI_ZONE_CAVEAT.to_string()),
        WINDOWS_CPU_STUBBED,
        WINDOWS_NO_CPU_SOURCE,
    ) {
        Ok(group) => metric.cpu = Some(group),
        Err(reason) => metric.push_unsupported(CATEGORY_CPU, reason),
    }

    metric.push_unsupported(CATEGORY_MEMORY, WINDOWS_NO_MEMORY_SOURCE);
    metric
}

/// Folds drive temperatures into an existing Windows `ThermalMetric`.
///
/// `Temperature` from the storage stack is already in degrees Celsius;
/// `TemperatureMax` becomes the high threshold. SMART exposes no critical
/// threshold, so `critical_c` stays `None` rather than being guessed.
pub(crate) fn windows_apply_disk_thermal(
    metric: &mut ThermalMetric,
    drives: Vec<(String, Option<f64>, Option<f64>)>,
) {
    let sensors: Vec<ThermalSensor> = drives
        .into_iter()
        .map(|(label, celsius, high_c)| ThermalSensor {
            label,
            source: "storage_reliability_counter".to_string(),
            temperature_c: celsius.and_then(plausible),
            high_c,
            critical_c: None,
        })
        .collect();

    match finalize_group(
        ThermalCategory::Disk,
        sensors,
        None,
        WINDOWS_NO_DISK_SOURCE,
        WINDOWS_NO_DISK_SOURCE,
    ) {
        Ok(group) => {
            metric.disk = Some(group);
            metric
                .unsupported
                .retain(|entry| entry.category != CATEGORY_DISK);
        }
        Err(reason) => {
            if !metric
                .unsupported
                .iter()
                .any(|entry| entry.category == CATEGORY_DISK)
            {
                metric.push_unsupported(CATEGORY_DISK, reason);
            }
        }
    }
}

/// Builds a full Windows `ThermalMetric`, reading drive temperatures at most
/// once per `DISK_THERMAL_MIN_INTERVAL_SECS`.
///
/// `now` is injected so the throttle is unit-testable. Returns the metric plus
/// an optional error string for `TelemetryErrors::thermal` — set only when the
/// drive command itself failed, never merely because a host has no sensors.
pub(crate) fn collect_windows_thermal<F>(
    probe: &mut Option<ThermalProbe>,
    zones: Vec<(String, f64)>,
    now: Instant,
    run_disk: F,
) -> (ThermalMetric, Option<String>)
where
    F: Fn(&str) -> Result<String, String>,
{
    let state = probe.get_or_insert_with(ThermalProbe::default);
    let mut metric = windows_thermal_from_zones(zones);
    let mut error = None;

    let due = state.disk_read_at.is_none_or(|last| {
        now.duration_since(last) >= Duration::from_secs(DISK_THERMAL_MIN_INTERVAL_SECS)
    });

    if due {
        match run_disk(WINDOWS_DISK_TEMP_COMMAND) {
            Ok(output) => {
                let drives = super::windows_monitor::parse_windows_disk_temps(&output);
                let mut fresh = ThermalMetric::default();
                windows_apply_disk_thermal(&mut fresh, drives);
                state.disk_outcome = Some(match fresh.disk {
                    Some(group) => Ok(group),
                    None => Err(WINDOWS_NO_DISK_SOURCE.to_string()),
                });
                state.disk_read_at = Some(now);
            }
            Err(reason) => {
                // Keep whatever was last read: a slow storage stack must cost a
                // stale temperature, not a fabricated one. Retry next tick.
                error = Some(format!("Drive temperature read failed: {}", reason));
            }
        }
    }

    match &state.disk_outcome {
        Some(Ok(group)) => metric.disk = Some(group.clone()),
        Some(Err(reason)) => metric.push_unsupported(CATEGORY_DISK, reason.clone()),
        None => metric.push_unsupported(CATEGORY_DISK, WINDOWS_NO_DISK_SOURCE),
    }

    (metric, error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sensor(label: &str, celsius: Option<f64>) -> ThermalSensor {
        ThermalSensor {
            label: label.to_string(),
            source: "coretemp".to_string(),
            temperature_c: celsius,
            high_c: None,
            critical_c: None,
        }
    }

    #[test]
    fn classifies_each_driver_and_zone_type() {
        for name in ["coretemp", "k10temp", "zenpower", "k8temp", "via_cputemp"] {
            assert_eq!(classify_thermal_source(name), ThermalCategory::Cpu, "{name}");
        }
        for name in ["x86_pkg_temp", "cpu-thermal", "cpu_thermal", "soc_thermal"] {
            assert_eq!(classify_thermal_source(name), ThermalCategory::Cpu, "{name}");
        }
        // Per-core zone naming on ARM boards.
        for name in ["cpu0-thermal", "cpu-thermal-0", "CPU1_thermal"] {
            assert_eq!(classify_thermal_source(name), ThermalCategory::Cpu, "{name}");
        }
        for name in ["jc42", "spd5118"] {
            assert_eq!(
                classify_thermal_source(name),
                ThermalCategory::Memory,
                "{name}"
            );
        }
        for name in ["nvme", "drivetemp"] {
            assert_eq!(classify_thermal_source(name), ThermalCategory::Disk, "{name}");
        }
        assert_eq!(classify_thermal_source("acpitz"), ThermalCategory::AcpiZone);
        // Case and whitespace from a sysfs read must not matter.
        assert_eq!(classify_thermal_source("  CoreTemp "), ThermalCategory::Cpu);
        assert_eq!(classify_thermal_source(""), ThermalCategory::Other);
    }

    #[test]
    fn gpu_and_board_sensors_never_land_in_the_cpu_group() {
        // GPUs are gpu_monitor's job; leaking them here would double-report and
        // could make a hot GPU look like a hot CPU.
        for name in ["amdgpu", "i915", "nouveau", "radeon", "xe"] {
            assert_eq!(
                classify_thermal_source(name),
                ThermalCategory::Other,
                "{name}"
            );
        }
        // Super-I/O chips expose a `CPUTIN` label but sit on the motherboard and
        // are commonly mis-scaled, so attributing them to the CPU would be a
        // guess presented as a fact.
        for name in ["nct6775", "nct6683", "it87", "f71882fg"] {
            assert_eq!(
                classify_thermal_source(name),
                ThermalCategory::Other,
                "{name}"
            );
        }
        for name in ["pch_cannonlake", "iwlwifi_1", "BAT0", "thinkpad", "tpm"] {
            assert_eq!(
                classify_thermal_source(name),
                ThermalCategory::Other,
                "{name}"
            );
        }
    }

    #[test]
    fn cpu_headline_prefers_the_package_over_individual_cores() {
        let sensors = vec![
            sensor("Core 0", Some(61.0)),
            sensor("Package id 0", Some(58.0)),
            sensor("Core 1", Some(64.0)),
        ];
        let (value, label) = choose_headline(ThermalCategory::Cpu, &sensors).unwrap();
        // The package wins even though a core is hotter: it is the authoritative
        // die reading, not the maximum of everything.
        assert_eq!(value, 58.0);
        assert_eq!(label, "Package id 0");
    }

    #[test]
    fn tdie_beats_tctl_because_tctl_carries_a_vendor_offset() {
        // Several Ryzen and Threadripper parts report Tctl = Tdie + 27 C.
        // Preferring Tctl would over-report by that offset.
        let sensors = vec![sensor("Tctl", Some(85.0)), sensor("Tdie", Some(58.0))];
        let (value, label) = choose_headline(ThermalCategory::Cpu, &sensors).unwrap();
        assert_eq!(value, 58.0);
        assert_eq!(label, "Tdie");

        // With only Tctl present it is still used — better than nothing.
        let only_tctl = vec![sensor("Tctl", Some(85.0))];
        assert_eq!(
            choose_headline(ThermalCategory::Cpu, &only_tctl).unwrap().1,
            "Tctl"
        );
    }

    #[test]
    fn a_multi_socket_host_reports_the_hotter_package() {
        let sensors = vec![
            sensor("Package id 0", Some(52.0)),
            sensor("Package id 1", Some(67.0)),
        ];
        let (value, label) = choose_headline(ThermalCategory::Cpu, &sensors).unwrap();
        assert_eq!(value, 67.0);
        assert_eq!(label, "Package id 1");
    }

    #[test]
    fn without_a_package_the_hottest_core_is_used_and_labeled_as_such() {
        let sensors = vec![
            sensor("Core 0", Some(61.0)),
            sensor("Core 1", Some(72.0)),
            sensor("Core 2", None),
        ];
        let (value, label) = choose_headline(ThermalCategory::Cpu, &sensors).unwrap();
        assert_eq!(value, 72.0);
        // Labeling it "Core 1" would imply only that core is being reported.
        assert_eq!(label, "hottest of 2 cores");
    }

    #[test]
    fn memory_and_disk_take_the_hottest_device() {
        // One hot DIMM or one throttling drive is the fact that matters; an
        // average would hide it.
        let dimms = vec![
            sensor("DIMM 0x18", Some(41.0)),
            sensor("DIMM 0x1a", Some(58.0)),
        ];
        let (value, label) = choose_headline(ThermalCategory::Memory, &dimms).unwrap();
        assert_eq!(value, 58.0);
        assert_eq!(label, "DIMM 0x1a");

        let drives = vec![sensor("nvme0", Some(38.0)), sensor("nvme1", Some(71.0))];
        assert_eq!(
            choose_headline(ThermalCategory::Disk, &drives).unwrap().0,
            71.0
        );
    }

    #[test]
    fn a_group_with_no_readings_has_no_headline() {
        let sensors = vec![sensor("Package id 0", None), sensor("Core 0", None)];
        assert_eq!(choose_headline(ThermalCategory::Cpu, &sensors), None);
        assert_eq!(choose_headline(ThermalCategory::Cpu, &[]), None);
    }

    #[test]
    fn implausible_readings_are_rejected_rather_than_reported() {
        // hwmon uses 0 for a present-but-uninitialized sensor.
        assert_eq!(millidegrees_to_celsius("0"), None);
        // A stub NVMe sensor reports 0 K.
        assert_eq!(millidegrees_to_celsius("-273150"), None);
        assert_eq!(millidegrees_to_celsius("200000"), None);
        assert_eq!(millidegrees_to_celsius("not-a-number"), None);
        assert_eq!(millidegrees_to_celsius(" 58125 "), Some(58.125));
    }

    const INTEL_PROBE: &str = "\
__HWMON_NAME__
/sys/class/hwmon/hwmon0/name:acpitz
/sys/class/hwmon/hwmon2/name:coretemp
/sys/class/hwmon/hwmon4/name:nvme
__HWMON_LABEL__
/sys/class/hwmon/hwmon2/temp1_label:Package id 0
/sys/class/hwmon/hwmon2/temp2_label:Core 0
/sys/class/hwmon/hwmon2/temp3_label:Core 1
/sys/class/hwmon/hwmon4/temp1_label:Composite
__HWMON_MAX__
/sys/class/hwmon/hwmon4/temp1_max:84850
__HWMON_CRIT__
/sys/class/hwmon/hwmon2/temp1_crit:100000
/sys/class/hwmon/hwmon4/temp1_crit:89850
__HWMON_INPUT__
/sys/class/hwmon/hwmon0/temp1_input
/sys/class/hwmon/hwmon2/temp1_input
/sys/class/hwmon/hwmon2/temp2_input
/sys/class/hwmon/hwmon2/temp3_input
/sys/class/hwmon/hwmon4/temp1_input
__HWMON_REALPATH__
/sys/class/hwmon/hwmon0:/sys/devices/virtual/thermal/thermal_zone0/hwmon0
/sys/class/hwmon/hwmon2:/sys/devices/platform/coretemp.0/hwmon/hwmon2
/sys/class/hwmon/hwmon4:/sys/devices/pci0000:00/0000:00:1d.0/0000:04:00.0/nvme/nvme0/hwmon4
__ZONE_TYPE__
/sys/class/thermal/thermal_zone0/type:acpitz
";

    const INTEL_VALUES: &str = "\
__HWMON_INPUT__
/sys/class/hwmon/hwmon0/temp1_input:27800
/sys/class/hwmon/hwmon2/temp1_input:58000
/sys/class/hwmon/hwmon2/temp2_input:56000
/sys/class/hwmon/hwmon2/temp3_input:61000
/sys/class/hwmon/hwmon4/temp1_input:43850
__ZONE_TEMP__
/sys/class/thermal/thermal_zone0/temp:27800
";

    #[test]
    fn parses_an_intel_laptop_with_coretemp_and_nvme() {
        let probe = parse_linux_thermal_probe(INTEL_PROBE);
        assert!(!probe.sysfs_empty);

        let metric = parse_linux_thermal_output(&probe, INTEL_VALUES);

        let cpu = metric.cpu.expect("coretemp present");
        assert_eq!(cpu.headline_c, Some(58.0));
        assert_eq!(cpu.headline_label.as_deref(), Some("Package id 0"));
        // A real CPU sensor exists, so the ACPI zone must not add a caveat.
        assert_eq!(cpu.caveat, None);
        // Intel exports TjMax as temp1_crit, so the real limit is used rather
        // than our default constant.
        assert_eq!(cpu.sensors[0].critical_c, Some(100.0));

        let disk = metric.disk.expect("nvme present");
        assert_eq!(disk.headline_c, Some(43.85));
        assert_eq!(disk.headline_label.as_deref(), Some("Composite"));
        assert_eq!(disk.sensors[0].high_c, Some(84.85));

        // No DIMM sensor on this host, and the reason names what is missing.
        assert!(metric.memory.is_none());
        let memory_reason = &metric
            .unsupported
            .iter()
            .find(|entry| entry.category == CATEGORY_MEMORY)
            .expect("memory unsupported")
            .reason;
        assert!(memory_reason.contains("jc42"), "{memory_reason}");
        assert!(memory_reason.contains("spd5118"), "{memory_reason}");
    }

    #[test]
    fn an_acpi_zone_stands_in_for_the_cpu_only_with_a_caveat() {
        // A VM with nothing but an ACPI zone.
        let probe = parse_linux_thermal_probe(
            "__HWMON_NAME__\n/sys/class/hwmon/hwmon0/name:acpitz\n__HWMON_INPUT__\n/sys/class/hwmon/hwmon0/temp1_input\n__ZONE_TYPE__\n/sys/class/thermal/thermal_zone0/type:acpitz\n",
        );
        let metric = parse_linux_thermal_output(
            &probe,
            "__HWMON_INPUT__\n/sys/class/hwmon/hwmon0/temp1_input:44000\n",
        );

        let cpu = metric.cpu.expect("promoted acpi zone");
        assert_eq!(cpu.headline_c, Some(44.0));
        // The caveat is what stops this being read as a die temperature.
        assert_eq!(cpu.caveat.as_deref(), Some(ACPI_ZONE_CAVEAT));
        assert_eq!(cpu.sensors[0].source, "acpi_thermal_zone");
    }

    #[test]
    fn labels_dimms_by_spd_address_and_drives_by_device() {
        let probe = parse_linux_thermal_probe(
            "\
__HWMON_NAME__
/sys/class/hwmon/hwmon3/name:jc42
/sys/class/hwmon/hwmon5/name:drivetemp
__HWMON_INPUT__
/sys/class/hwmon/hwmon3/temp1_input
/sys/class/hwmon/hwmon5/temp1_input
__HWMON_REALPATH__
/sys/class/hwmon/hwmon3:/sys/devices/pci0000:00/0000:00:1f.4/i2c-0/0-0018/hwmon/hwmon3
/sys/class/hwmon/hwmon5:/sys/devices/pci0000:00/0000:00:17.0/ata3/host2/target2:0:0/2:0:0:0/hwmon/hwmon5
",
        );
        let metric = parse_linux_thermal_output(
            &probe,
            "__HWMON_INPUT__\n/sys/class/hwmon/hwmon3/temp1_input:39500\n/sys/class/hwmon/hwmon5/temp1_input:34000\n",
        );

        // jc42 exports no label, so the SPD i2c address is used.
        assert_eq!(
            metric.memory.unwrap().headline_label.as_deref(),
            Some("DIMM 0x18")
        );
        // drivetemp's realpath ends at the SCSI target with no block name, so
        // the hwmon directory is named rather than a device invented.
        assert_eq!(
            metric.disk.unwrap().headline_label.as_deref(),
            Some("drive (hwmon5)")
        );
    }

    #[test]
    fn an_arm_board_with_only_thermal_zones_still_reports_a_cpu() {
        let probe = parse_linux_thermal_probe(
            "__HWMON_NAME__\n__HWMON_INPUT__\n__ZONE_TYPE__\n/sys/class/thermal/thermal_zone0/type:cpu-thermal\n/sys/class/thermal/thermal_zone1/type:gpu-thermal\n",
        );
        let metric = parse_linux_thermal_output(
            &probe,
            "__ZONE_TEMP__\n/sys/class/thermal/thermal_zone0/temp:52000\n/sys/class/thermal/thermal_zone1/temp:49000\n",
        );

        let cpu = metric.cpu.expect("cpu-thermal zone");
        assert_eq!(cpu.headline_c, Some(52.0));
        assert_eq!(cpu.sensors[0].source, "thermal_zone");
        // The GPU zone is gpu_monitor's territory and must not appear here.
        assert_eq!(cpu.sensors.len(), 1);
    }

    #[test]
    fn a_host_with_no_sysfs_at_all_says_so_specifically() {
        // WSL2 and most containers. The message has to differ from "the driver
        // is not loaded", because nothing the user installs would help.
        //
        // This fixture is the verbatim output of LINUX_THERMAL_PROBE_COMMAND run
        // through /bin/sh on a host with no /sys at all — captured, not written
        // by hand, so it keeps the blank line each `printf '\n__SECTION__\n'`
        // leaves in front of its marker. A hand-tidied fixture without those
        // blank lines would pass while the real parse of a real host differed.
        let probe = parse_linux_thermal_probe(concat!(
            "__HWMON_NAME__\n\n",
            "__HWMON_LABEL__\n\n",
            "__HWMON_MAX__\n\n",
            "__HWMON_CRIT__\n\n",
            "__HWMON_INPUT__\n\n",
            "__HWMON_REALPATH__\n\n",
            "__ZONE_TYPE__\n",
        ));
        assert!(probe.sysfs_empty);
        assert!(probe.sensors.is_empty());

        let metric = parse_linux_thermal_output(&probe, "");
        assert!(metric.cpu.is_none() && metric.memory.is_none() && metric.disk.is_none());
        assert_eq!(metric.unsupported.len(), 3);
        for entry in &metric.unsupported {
            assert!(
                entry.reason.contains("no /sys/class/hwmon"),
                "{}: {}",
                entry.category,
                entry.reason
            );
            assert!(entry.reason.contains("WSL2"), "{}", entry.reason);
        }
    }

    #[test]
    fn a_present_but_silent_sensor_is_unsupported_not_a_blank_group() {
        let probe = parse_linux_thermal_probe(
            "__HWMON_NAME__\n/sys/class/hwmon/hwmon2/name:coretemp\n__HWMON_LABEL__\n/sys/class/hwmon/hwmon2/temp1_label:Package id 0\n__HWMON_INPUT__\n/sys/class/hwmon/hwmon2/temp1_input\n",
        );
        // The file exists but reports the uninitialized-sensor zero.
        let metric = parse_linux_thermal_output(
            &probe,
            "__HWMON_INPUT__\n/sys/class/hwmon/hwmon2/temp1_input:0\n",
        );

        assert!(metric.cpu.is_none(), "a blank group would render as n/a");
        let reason = &metric
            .unsupported
            .iter()
            .find(|entry| entry.category == CATEGORY_CPU)
            .expect("cpu unsupported")
            .reason;
        assert!(reason.contains("implausible"), "{reason}");
    }

    #[test]
    fn the_probe_runs_once_and_values_run_every_tick() {
        use std::cell::RefCell;
        let calls = RefCell::new(Vec::<String>::new());
        let run = |command: &str| -> Result<String, String> {
            calls.borrow_mut().push(command.to_string());
            Ok(if command == LINUX_THERMAL_PROBE_COMMAND {
                INTEL_PROBE.to_string()
            } else {
                INTEL_VALUES.to_string()
            })
        };

        let mut probe = None;
        for _ in 0..3 {
            let metric = collect_linux_thermal(&mut probe, run).unwrap();
            assert_eq!(metric.cpu.as_ref().unwrap().headline_c, Some(58.0));
        }

        let recorded = calls.borrow();
        assert_eq!(
            recorded
                .iter()
                .filter(|c| *c == LINUX_THERMAL_PROBE_COMMAND)
                .count(),
            1,
            "the sensor inventory does not change while the host is up"
        );
        assert_eq!(
            recorded
                .iter()
                .filter(|c| *c == LINUX_THERMAL_VALUE_COMMAND)
                .count(),
            3
        );
    }

    #[test]
    fn a_sensorless_host_skips_the_per_tick_command_entirely() {
        use std::cell::RefCell;
        let calls = RefCell::new(0usize);
        let run = |_: &str| -> Result<String, String> {
            *calls.borrow_mut() += 1;
            Ok("__HWMON_NAME__\n__HWMON_INPUT__\n__ZONE_TYPE__\n".to_string())
        };

        let mut probe = None;
        for _ in 0..5 {
            let metric = collect_linux_thermal(&mut probe, run).unwrap();
            assert_eq!(metric.unsupported.len(), 3);
        }
        // One probe, then no round trips at all — re-confirming "still no
        // sensors" every two seconds would be pure waste.
        assert_eq!(*calls.borrow(), 1);
    }

    #[test]
    fn a_failed_command_is_an_error_but_a_sensorless_host_is_not() {
        let mut probe = None;
        let failing = |_: &str| -> Result<String, String> { Err("timed out".to_string()) };
        assert_eq!(
            collect_linux_thermal(&mut probe, failing),
            Err("timed out".to_string())
        );
        // The probe must not be cached from a failed read.
        assert!(probe.is_none());

        let mut probe = None;
        let empty = |_: &str| -> Result<String, String> { Ok(String::new()) };
        let metric = collect_linux_thermal(&mut probe, empty).unwrap();
        assert_eq!(metric.unsupported.len(), 3);
    }

    #[test]
    fn acpi_tenths_of_kelvin_convert_and_stubs_are_rejected() {
        // 3132 tenths-K = 40.05 C.
        assert_eq!(
            acpi_tenths_kelvin_to_celsius(3132.0).map(|c| (c * 100.0).round() / 100.0),
            Some(40.05)
        );
        // Firmware that stubs the class returns exactly 2732 (0.05 C) or 0.
        assert_eq!(acpi_tenths_kelvin_to_celsius(2732.0), None);
        assert_eq!(acpi_tenths_kelvin_to_celsius(0.0), None);
    }

    #[test]
    fn a_windows_zone_reading_is_never_presented_as_a_die_temperature() {
        let metric = windows_thermal_from_zones(vec![
            ("ACPI\\ThermalZone\\TZ00_0".to_string(), 3132.0),
            ("ACPI\\ThermalZone\\TZ01_0".to_string(), 3232.0),
        ]);

        let cpu = metric.cpu.expect("zones present");
        // Hottest zone wins, and all three honesty signals are set.
        assert_eq!(cpu.headline_c.map(|c| c.round()), Some(50.0));
        assert_eq!(cpu.caveat.as_deref(), Some(ACPI_ZONE_CAVEAT));
        assert!(cpu.sensors.iter().all(|s| s.source == "acpi_thermal_zone"));
        assert_eq!(cpu.headline_label.as_deref(), Some("ACPI\\ThermalZone\\TZ01_0"));

        // Windows never has a DIMM temperature, and the reason says why.
        assert!(metric.memory.is_none());
        let reason = &metric
            .unsupported
            .iter()
            .find(|e| e.category == CATEGORY_MEMORY)
            .unwrap()
            .reason;
        assert!(reason.contains("kernel-mode driver"), "{reason}");
    }

    #[test]
    fn firmware_without_the_wmi_class_is_unsupported_with_an_explanation() {
        // The common OEM case: the section comes back empty.
        let metric = windows_thermal_from_zones(Vec::new());
        assert!(metric.cpu.is_none());
        let reason = &metric
            .unsupported
            .iter()
            .find(|e| e.category == CATEGORY_CPU)
            .unwrap()
            .reason;
        assert!(reason.contains("MSAcpi_ThermalZoneTemperature"), "{reason}");
        assert!(reason.contains("no unprivileged way"), "{reason}");

        // Firmware that answers with stub values is a different message: the
        // interface exists, the values do not.
        let stubbed = windows_thermal_from_zones(vec![("TZ00".to_string(), 2732.0)]);
        assert!(stubbed.cpu.is_none());
        let stub_reason = &stubbed
            .unsupported
            .iter()
            .find(|e| e.category == CATEGORY_CPU)
            .unwrap()
            .reason;
        assert!(stub_reason.contains("stub value"), "{stub_reason}");
        assert_ne!(stub_reason, reason, "the two causes must read differently");
    }

    #[test]
    fn windows_drive_temperatures_fold_in_and_clear_the_unsupported_entry() {
        let mut metric = windows_thermal_from_zones(Vec::new());
        assert_eq!(metric.unsupported.len(), 2); // cpu + memory

        windows_apply_disk_thermal(
            &mut metric,
            vec![
                ("Samsung SSD 990 PRO".to_string(), Some(41.0), Some(82.0)),
                ("WDC WD40EFRX".to_string(), Some(38.0), None),
            ],
        );

        let disk = metric.disk.expect("drives reported a temperature");
        assert_eq!(disk.headline_c, Some(41.0));
        assert_eq!(disk.headline_label.as_deref(), Some("Samsung SSD 990 PRO"));
        assert_eq!(disk.sensors[0].high_c, Some(82.0));
        // SMART exposes no critical threshold, so it is left unset rather than
        // guessed.
        assert_eq!(disk.sensors[0].critical_c, None);
        assert!(!metric
            .unsupported
            .iter()
            .any(|e| e.category == CATEGORY_DISK));
    }

    #[test]
    fn a_usb_bridge_that_passes_no_smart_is_unsupported_not_zero() {
        let mut metric = windows_thermal_from_zones(Vec::new());
        // The cmdlet returns a row with no Temperature for USB-bridged drives.
        windows_apply_disk_thermal(&mut metric, vec![("USB Mass Storage".to_string(), None, None)]);

        assert!(metric.disk.is_none(), "a 0 C reading would be a fabrication");
        let reason = &metric
            .unsupported
            .iter()
            .find(|e| e.category == CATEGORY_DISK)
            .unwrap()
            .reason;
        assert!(reason.contains("USB-bridged"), "{reason}");
    }

    #[test]
    fn macos_reports_every_category_unsupported_with_the_reason() {
        let metric = macos_thermal_unsupported();
        assert!(metric.cpu.is_none() && metric.memory.is_none() && metric.disk.is_none());

        let categories: Vec<&str> = metric
            .unsupported
            .iter()
            .map(|entry| entry.category.as_str())
            .collect();
        assert_eq!(categories, vec![CATEGORY_CPU, CATEGORY_MEMORY, CATEGORY_DISK]);
        for entry in &metric.unsupported {
            // Naming powermetrics and root is what makes this actionable rather
            // than just blank.
            assert!(entry.reason.contains("powermetrics"), "{}", entry.reason);
            assert!(entry.reason.contains("root"), "{}", entry.reason);
        }
    }
}
