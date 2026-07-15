import { HardDrive } from "lucide-react";
import { useMemo, useState, type RefObject } from "react";
import type { DiskMetric } from "../types/gpu";
import { formatGiBOrTiB } from "../utils/formatBytes";
import {
  diskUsageLevel,
  filterDisksByFsType,
  formatDiskUsagePercent,
  sortDisksByPriority,
} from "../utils/diskPriority";
import { ResourceDetailPopover } from "./ResourceDetailPopover";

type DiskUsagePopoverProps = {
  disks: DiskMetric[];
  ignoredFsTypes: string[];
  anchorRef: RefObject<HTMLElement | null>;
  onClose: () => void;
  onPopOut?: () => void;
};

export function DiskDetailContent({
  disks,
  ignoredFsTypes,
}: {
  disks: DiskMetric[];
  ignoredFsTypes: string[];
}) {
  const [showHidden, setShowHidden] = useState(false);
  const visibleDisks = useMemo(
    () =>
      sortDisksByPriority(
        showHidden ? disks : filterDisksByFsType(disks, ignoredFsTypes),
      ),
    [disks, ignoredFsTypes, showHidden],
  );

  return (
    <>
      <label className="toggle-row disk-hidden-toggle">
        <input
          type="checkbox"
          checked={showHidden}
          onChange={(event) => setShowHidden(event.target.checked)}
        />
        <span>Show hidden filesystems</span>
      </label>
      {visibleDisks.length === 0 ? (
        <div className="empty-list">Disk metrics unavailable</div>
      ) : (
        <div className="disk-detail-table">
          <div className="disk-detail-row head">
            <span>Mount</span>
            <span>Filesystem</span>
            <span>Type</span>
            <span>Used</span>
            <span>Available</span>
            <span>Total</span>
            <span>Usage</span>
          </div>
          {visibleDisks.map((disk) => (
            <div
              className={`disk-detail-row ${diskUsageLevel(disk.usagePercent)}`}
              key={`${disk.filesystem}:${disk.mountPoint}`}
              data-usage-level={diskUsageLevel(disk.usagePercent)}
            >
              <span title={disk.mountPoint}>{disk.mountPoint}</span>
              <span title={disk.filesystem}>{disk.filesystem}</span>
              <span>{disk.fsType ?? "-"}</span>
              <span>{formatGiBOrTiB(disk.usedBytes)}</span>
              <span>{formatGiBOrTiB(disk.availableBytes)}</span>
              <span>{formatGiBOrTiB(disk.totalBytes)}</span>
              <span className="disk-usage-cell">
                <strong>{formatDiskUsagePercent(disk.usagePercent)}</strong>
                <DiskUsageBar value={disk.usagePercent} />
              </span>
            </div>
          ))}
        </div>
      )}
    </>
  );
}

export function DiskUsagePopover({
  disks,
  ignoredFsTypes,
  anchorRef,
  onClose,
  onPopOut,
}: DiskUsagePopoverProps) {
  return (
    <ResourceDetailPopover
      anchorRef={anchorRef}
      ariaLabel="Disk details"
      title="Disks"
      icon={<HardDrive size={16} />}
      onClose={onClose}
      onPopOut={onPopOut}
    >
      <DiskDetailContent disks={disks} ignoredFsTypes={ignoredFsTypes} />
    </ResourceDetailPopover>
  );
}

function DiskUsageBar({ value }: { value: number | null }) {
  const width = value == null ? 0 : Math.max(0, Math.min(100, value));
  return (
    <div className={`disk-usage-bar ${diskUsageLevel(value)}`}>
      <div style={{ width: `${width}%` }} />
    </div>
  );
}
