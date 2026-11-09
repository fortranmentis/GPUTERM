import { HardDrive } from "lucide-react";
import { useMemo, useState, type KeyboardEvent, type RefObject } from "react";
import type { DiskMetric, ThermalMetric } from "../types/gpu";
import { formatGiBOrTiB } from "../utils/formatBytes";
import {
  diskUsageLevel,
  filterDisksByFsType,
  formatDiskUsagePercent,
  sortDisksByPriority,
} from "../utils/diskPriority";
import { ResourceDetailPopover, ThermalSection } from "./ResourceDetailPopover";
import type { TelemetryHistory } from "../types/telemetryHistory";
import { DiskHistory } from "./UsageHistory";
import { DirectorySizesView } from "./DirectorySizesView";

type DiskUsagePopoverProps = {
  disks: DiskMetric[];
  thermal: ThermalMetric | null;
  thermalError?: string | null;
  ignoredFsTypes: string[];
  anchorRef: RefObject<HTMLElement | null>;
  onClose: () => void;
  onPopOut?: () => void;
  history?: TelemetryHistory | null;
  sessionId?: string;
  onOpenInSftp?: (path: string) => void;
};

export function DiskDetailContent({
  disks,
  thermal,
  thermalError,
  ignoredFsTypes,
  history,
  compactHistory = false,
  sessionId,
  onOpenInSftp,
}: {
  disks: DiskMetric[];
  thermal: ThermalMetric | null;
  thermalError?: string | null;
  ignoredFsTypes: string[];
  /** Usage over time; drawn when present. */
  history?: TelemetryHistory | null;
  /** The popover's small sparkline instead of the full charts. */
  compactHistory?: boolean;
  /** Enables clicking a mount to see the sizes of the folders in it. */
  sessionId?: string;
  onOpenInSftp?: (path: string) => void;
}) {
  const [showHidden, setShowHidden] = useState(false);
  const [openMount, setOpenMount] = useState<string | null>(null);
  const visibleDisks = useMemo(
    () =>
      sortDisksByPriority(
        showHidden ? disks : filterDisksByFsType(disks, ignoredFsTypes),
      ),
    [disks, ignoredFsTypes, showHidden],
  );

  if (sessionId && openMount) {
    return (
      <DirectorySizesView
        key={`${sessionId}:${openMount}`}
        sessionId={sessionId}
        root={openMount}
        onBack={() => setOpenMount(null)}
        onOpenInSftp={onOpenInSftp}
      />
    );
  }

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
      <DiskHistory history={history} compact={compactHistory} />
      <ThermalSection thermal={thermal} kind="disk" error={thermalError} />
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
              className={`disk-detail-row ${diskUsageLevel(disk.usagePercent)}${sessionId ? " clickable" : ""}`}
              key={`${disk.filesystem}:${disk.mountPoint}`}
              data-usage-level={diskUsageLevel(disk.usagePercent)}
              {...(sessionId
                ? {
                    role: "button",
                    tabIndex: 0,
                    "aria-label": `Show folder sizes in ${disk.mountPoint}`,
                    title: "Show the sizes of the folders in this mount",
                    onClick: () => setOpenMount(disk.mountPoint),
                    onKeyDown: (event: KeyboardEvent) => {
                      if (event.key === "Enter" || event.key === " ") {
                        event.preventDefault();
                        setOpenMount(disk.mountPoint);
                      }
                    },
                  }
                : {})}
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
  thermal,
  thermalError,
  ignoredFsTypes,
  anchorRef,
  onClose,
  onPopOut,
  history,
  sessionId,
  onOpenInSftp,
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
      <DiskDetailContent
        disks={disks}
        thermal={thermal}
        thermalError={thermalError}
        ignoredFsTypes={ignoredFsTypes}
        history={history}
        compactHistory
        sessionId={sessionId}
        onOpenInSftp={onOpenInSftp}
      />
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
