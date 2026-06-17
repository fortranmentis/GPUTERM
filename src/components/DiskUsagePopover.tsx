import { HardDrive } from "lucide-react";
import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type RefObject,
} from "react";
import { createPortal } from "react-dom";
import type { DiskMetric } from "../types/gpu";
import { formatGiBOrTiB } from "../utils/formatBytes";
import {
  diskUsageLevel,
  filterDisksByFsType,
  formatDiskUsagePercent,
  sortDisksByPriority,
} from "../utils/diskPriority";

type DiskUsagePopoverProps = {
  disks: DiskMetric[];
  ignoredFsTypes: string[];
  anchorRef: RefObject<HTMLElement | null>;
  onClose: () => void;
};

export function DiskUsagePopover({
  disks,
  ignoredFsTypes,
  anchorRef,
  onClose,
}: DiskUsagePopoverProps) {
  const [showHidden, setShowHidden] = useState(false);
  const [style, setStyle] = useState<CSSProperties>({});
  const popoverRef = useRef<HTMLDivElement | null>(null);
  const visibleDisks = useMemo(
    () =>
      sortDisksByPriority(
        showHidden ? disks : filterDisksByFsType(disks, ignoredFsTypes),
      ),
    [disks, ignoredFsTypes, showHidden],
  );

  useEffect(() => {
    const handlePointerDown = (event: MouseEvent) => {
      if (
        popoverRef.current?.contains(event.target as Node) ||
        anchorRef.current?.contains(event.target as Node)
      ) {
        return;
      }
      onClose();
    };
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        onClose();
      }
    };
    document.addEventListener("mousedown", handlePointerDown);
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("mousedown", handlePointerDown);
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [anchorRef, onClose]);

  useLayoutEffect(() => {
    const placePopover = () => {
      const anchor = anchorRef.current;
      if (!anchor) {
        return;
      }
      const rect = anchor.getBoundingClientRect();
      const margin = 16;
      const maxHeight = Math.min(420, Math.max(260, window.innerHeight - margin * 2));
      const width = Math.min(920, Math.max(360, window.innerWidth - margin * 2));
      const left = Math.min(
        Math.max(margin, rect.right - width),
        window.innerWidth - width - margin,
      );
      const preferredTop = rect.top - maxHeight - 10;
      const top =
        preferredTop >= margin
          ? preferredTop
          : Math.min(rect.bottom + 10, window.innerHeight - maxHeight - margin);
      setStyle({
        left,
        top: Math.max(margin, top),
        width,
        maxHeight,
      });
    };

    placePopover();
    window.addEventListener("resize", placePopover);
    window.addEventListener("scroll", placePopover, true);
    return () => {
      window.removeEventListener("resize", placePopover);
      window.removeEventListener("scroll", placePopover, true);
    };
  }, [anchorRef]);

  const popover = (
    <div
      className="disk-detail-popover"
      ref={popoverRef}
      role="dialog"
      aria-label="Disk details"
      style={style}
    >
      <div className="disk-detail-title">
        <HardDrive size={16} />
        <strong>Disks</strong>
        <label className="toggle-row">
          <input
            type="checkbox"
            checked={showHidden}
            onChange={(event) => setShowHidden(event.target.checked)}
          />
          <span>Show hidden filesystems</span>
        </label>
        <span>{visibleDisks.length}</span>
      </div>
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
    </div>
  );

  return createPortal(popover, document.body);
}

function DiskUsageBar({ value }: { value: number | null }) {
  const width = value == null ? 0 : Math.max(0, Math.min(100, value));
  return (
    <div className={`disk-usage-bar ${diskUsageLevel(value)}`}>
      <div style={{ width: `${width}%` }} />
    </div>
  );
}
