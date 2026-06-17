import type { DiskMetric } from "../types/gpu";

export function diskPriority(mountPoint: string) {
  if (mountPoint === "/") {
    return 0;
  }
  if (mountPoint === "/home" || mountPoint.startsWith("/home/")) {
    return 1;
  }
  if (mountPoint === "/data" || mountPoint.startsWith("/data/")) {
    return 2;
  }
  if (mountPoint === "/mnt" || mountPoint.startsWith("/mnt/")) {
    return 3;
  }
  if (mountPoint === "/media" || mountPoint.startsWith("/media/")) {
    return 4;
  }
  return 5;
}

export function sortDisksByPriority(disks: DiskMetric[]) {
  return [...disks].sort((a, b) => {
    const priorityDelta = diskPriority(a.mountPoint) - diskPriority(b.mountPoint);
    if (priorityDelta !== 0) {
      return priorityDelta;
    }
    return a.mountPoint.localeCompare(b.mountPoint);
  });
}

export function filterDisksByFsType(
  disks: DiskMetric[],
  ignoredFsTypes: string[] = [],
) {
  const ignored = new Set(ignoredFsTypes.map((item) => item.toLowerCase()));
  return disks.filter((disk) => {
    if (!disk.fsType) {
      return true;
    }
    return !ignored.has(disk.fsType.toLowerCase());
  });
}

export function createDiskSummary(
  disks: DiskMetric[],
  maxVisible = 2,
  ignoredFsTypes: string[] = [],
) {
  const sorted = sortDisksByPriority(filterDisksByFsType(disks, ignoredFsTypes));
  const visible = sorted.slice(0, maxVisible);
  const hiddenCount = Math.max(0, sorted.length - visible.length);
  return { visible, hiddenCount };
}

export function formatDiskUsagePercent(value: number | null | undefined) {
  return value == null ? "?" : `${Math.round(value)}%`;
}

export function diskUsageLevel(value: number | null | undefined) {
  if (value == null) {
    return "unknown";
  }
  if (value >= 90) {
    return "critical";
  }
  if (value >= 80) {
    return "warning";
  }
  return "normal";
}
