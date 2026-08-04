import { formatBytes } from "./formatBytes";

export type UsageLevel = "normal" | "warning" | "critical" | "unknown";

export function formatPercent(value: number | null | undefined, digits = 0) {
  if (value == null) return "n/a";
  return digits === 0 ? `${Math.round(value)}%` : `${value.toFixed(digits)}%`;
}

export function formatNumber(value: number | null | undefined, digits = 2) {
  return value == null ? "n/a" : value.toFixed(digits);
}

export function formatGhz(
  value: number | null | undefined,
  digits = 1,
  fallback = "n/a GHz",
) {
  return value == null ? fallback : `${value.toFixed(digits)} GHz`;
}

export function formatWatts(value: number | null | undefined) {
  return value == null ? "n/a" : `${value.toFixed(0)} W`;
}

export function formatTemperature(value: number | null | undefined) {
  return value == null ? "n/a" : `${value.toFixed(0)} C`;
}

export function formatClock(value: number | null | undefined) {
  return value == null ? "n/a" : `${value.toFixed(0)} MHz`;
}

export function formatMiB(value: number | null | undefined) {
  return value == null ? "n/a" : formatBytes(value * 1024 * 1024);
}

export function formatGiBFromMiB(value: number | null | undefined) {
  if (value == null) return "n/a";
  return `${(value / 1024).toFixed(value >= 10 * 1024 ? 1 : 2)} GiB`;
}

export function formatUptime(value: number | null | undefined) {
  if (value == null) return "n/a";
  const days = Math.floor(value / 86400);
  const hours = Math.floor((value % 86400) / 3600);
  const minutes = Math.floor((value % 3600) / 60);
  return `${days}d ${hours}h ${minutes}m`;
}

/** Compact style used by the telemetry bar, e.g. "8/16 cores". */
export function formatCoreCount(online: number | null, total: number | null) {
  if (online == null && total == null) return "cores n/a";
  if (online != null && total != null && online !== total) return `${online}/${total} cores`;
  return `${total ?? online} cores`;
}

/** Ratio style used by the CPU popover, e.g. "8 / 16". */
export function formatCoreRatio(online: number | null, total: number | null) {
  if (online == null && total == null) return "n/a";
  return online != null && total != null ? `${online} / ${total}` : String(online ?? total);
}

export function ratio(used: number | null, total: number | null) {
  return used != null && total != null && total > 0 ? (used / total) * 100 : null;
}

export function cpuLevel(value: number | null | undefined): UsageLevel {
  if (value == null) return "unknown";
  if (value >= 95) return "critical";
  if (value >= 80) return "warning";
  return "normal";
}

export function memoryLevel(value: number | null | undefined): UsageLevel {
  if (value == null) return "unknown";
  if (value >= 95) return "critical";
  if (value >= 85) return "warning";
  return "normal";
}

export function temperatureLevel(value: number | null | undefined): UsageLevel {
  if (value == null) return "unknown";
  if (value >= 90) return "critical";
  if (value >= 80) return "warning";
  return "normal";
}

/**
 * Level for a CPU, DIMM, or drive temperature.
 *
 * Vendor thresholds win when the sensor exports them (`tempN_max`/`tempN_crit`,
 * NVMe WCTEMP/CCTEMP, Intel TjMax, SMART TemperatureMax) — the hardware knows
 * its own limits better than any constant here.
 *
 * Class defaults otherwise, because the GPU-tuned 80/90 pair is wrong for both
 * other classes: it would paint a healthy Zen 4 desktop CPU critical (those
 * parts are designed to sit at 95 C under all-core boost) while missing an NVMe
 * that is already thermally throttling at 75 C.
 */
export function thermalLevel(
  value: number | null | undefined,
  kind: "cpu" | "memory" | "disk",
  highC?: number | null,
  criticalC?: number | null,
): UsageLevel {
  if (value == null) return "unknown";

  // Intel TjMax is 100 C on modern Core parts; AMD Zen 3/4/5 desktop parts
  // throttle at a 95 C Tctl limit and routinely operate there by design.
  // JEDEC DDR4/DDR5 maximum operating case temperature is 85 C.
  // Consumer NVMe drives begin throttling around 70-75 C.
  const defaults = {
    cpu: { warning: 90, critical: 100 },
    memory: { warning: 80, critical: 90 },
    disk: { warning: 70, critical: 80 },
  }[kind];

  const critical = criticalC ?? defaults.critical;
  const warning = highC ?? defaults.warning;
  if (value >= critical) return "critical";
  if (value >= warning) return "warning";
  return "normal";
}

export function vramLevel(usedMiB: number | null, totalMiB: number | null): UsageLevel {
  const value = ratio(usedMiB, totalMiB);
  if (value == null) return "unknown";
  if (value >= 98) return "critical";
  if (value >= 90) return "warning";
  return "normal";
}

export function powerLevel(drawW: number | null, limitW: number | null): UsageLevel {
  const value = ratio(drawW, limitW);
  if (value == null) return "unknown";
  return value >= 90 ? "warning" : "normal";
}
