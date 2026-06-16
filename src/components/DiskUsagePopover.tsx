import { HardDrive } from "lucide-react";
import type { DiskMetric } from "../types/gpu";
import { formatGiBOrTiB } from "../utils/formatBytes";
import { formatDiskUsagePercent } from "../utils/diskPriority";

type DiskUsagePopoverProps = {
  disks: DiskMetric[];
};

export function DiskUsagePopover({ disks }: DiskUsagePopoverProps) {
  return (
    <div className="disk-detail-popover">
      <div className="disk-detail-title">
        <HardDrive size={16} />
        <strong>Disks</strong>
        <span>{disks.length}</span>
      </div>
      <div className="disk-detail-table">
        <div className="disk-detail-row head">
          <span>Mount</span>
          <span>Type</span>
          <span>Used</span>
          <span>Available</span>
          <span>Total</span>
          <span>Use</span>
        </div>
        {disks.map((disk) => (
          <div
            className="disk-detail-row"
            key={`${disk.filesystem}:${disk.mountPoint}`}
          >
            <span title={disk.mountPoint}>{disk.mountPoint}</span>
            <span>{disk.fsType ?? "-"}</span>
            <span>{formatGiBOrTiB(disk.usedBytes)}</span>
            <span>{formatGiBOrTiB(disk.availableBytes)}</span>
            <span>{formatGiBOrTiB(disk.totalBytes)}</span>
            <span>{formatDiskUsagePercent(disk.usagePercent)}</span>
          </div>
        ))}
      </div>
    </div>
  );
}
