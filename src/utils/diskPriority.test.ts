import { describe, expect, it } from "vitest";
import type { DiskMetric } from "../types/gpu";
import {
  createDiskSummary,
  formatDiskUsagePercent,
  sortDisksByPriority,
} from "./diskPriority";

function disk(mountPoint: string, usagePercent: number | null = 50): DiskMetric {
  return {
    filesystem: `/dev/${mountPoint.replace(/\W/g, "") || "root"}`,
    fsType: "ext4",
    mountPoint,
    totalBytes: 100,
    usedBytes: 50,
    availableBytes: 50,
    usagePercent,
  };
}

describe("disk priority utilities", () => {
  it("sorts disks by /, /home, /data priority", () => {
    const sorted = sortDisksByPriority([
      disk("/data"),
      disk("/var"),
      disk("/home"),
      disk("/"),
    ]);

    expect(sorted.map((item) => item.mountPoint)).toEqual([
      "/",
      "/home",
      "/data",
      "/var",
    ]);
  });

  it("keeps at most two visible disks and counts hidden disks", () => {
    const summary = createDiskSummary([
      disk("/"),
      disk("/data"),
      disk("/mnt/storage"),
      disk("/media/backup"),
    ]);

    expect(summary.visible.map((item) => item.mountPoint)).toEqual(["/", "/data"]);
    expect(summary.hiddenCount).toBe(2);
  });

  it("formats null usage percent as ?", () => {
    expect(formatDiskUsagePercent(null)).toBe("?");
  });
});
