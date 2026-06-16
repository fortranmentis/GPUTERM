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

export function createDiskSummary(disks: DiskMetric[], maxVisible = 2) {
  const sorted = sortDisksByPriority(disks);
  const visible = sorted.slice(0, maxVisible);
  const hiddenCount = Math.max(0, sorted.length - visible.length);
  return { visible, hiddenCount };
}

export function formatDiskUsagePercent(value: number | null | undefined) {
  return value == null ? "?" : `${Math.round(value)}%`;
}
