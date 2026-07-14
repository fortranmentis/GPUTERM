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
