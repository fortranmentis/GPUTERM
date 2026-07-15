import { MemoryStick } from "lucide-react";
import { useEffect, useRef, useState, type RefObject } from "react";
import type { MemoryDetailMetric } from "../types/resourceDetails";
import { formatBytes } from "../utils/formatBytes";
import { formatMiB, formatPercent, memoryLevel } from "../utils/format";
import {
  DetailUsageBar,
  Metric,
  MetricsUnavailable,
  ResourceDetailPopover,
} from "./ResourceDetailPopover";

type MemoryUsagePopoverProps = {
  metric: MemoryDetailMetric | null;
  error?: string | null;
  loading: boolean;
  anchorRef: RefObject<HTMLElement | null>;
  onClose: () => void;
  onPopOut?: () => void;
};

export function MemoryDetailContent({
  metric,
  error,
}: {
  metric: MemoryDetailMetric | null;
  error?: string | null;
}) {
  const previousSwap = useRef<number | null>(null);
  const [swapIncreasing, setSwapIncreasing] = useState(false);

  useEffect(() => {
    const current = metric?.swapUsedMiB ?? null;
    if (current != null) {
      setSwapIncreasing(previousSwap.current != null && current > previousSwap.current && current > 0);
      previousSwap.current = current;
    }
  }, [metric?.swapUsedMiB]);

  if (!metric) {
    return <MetricsUnavailable error={error} />;
  }
  const level = memoryLevel(metric.usagePercent);
  return (
    <>
      <div className="resource-detail-summary">
        <div className="resource-primary-metric">
          <strong className={level}>{formatPercent(metric.usagePercent, 1)}</strong>
          <span>{formatMiB(metric.usedMiB)} / {formatMiB(metric.totalMiB)}</span>
          <DetailUsageBar value={metric.usagePercent} level={level} />
        </div>
        <div className="resource-metric-grid">
          <Metric label="Available" value={formatMiB(metric.availableMiB)} />
          <Metric label="Free" value={formatMiB(metric.freeMiB)} />
          <Metric label="Buffers" value={formatMiB(metric.buffersMiB)} title="Linux buffers are generally reclaimable memory." />
          <Metric label="Cached" value={formatMiB(metric.cachedMiB)} title="Linux page cache can generally be reclaimed when applications need memory." />
          <Metric label="Swap used" value={formatMiB(metric.swapUsedMiB)} warning={swapIncreasing} />
          <Metric label="Swap free" value={formatMiB(metric.swapFreeMiB)} />
        </div>
      </div>
      <div className="swap-summary">
        <span>Swap {formatMiB(metric.swapUsedMiB)} / {formatMiB(metric.swapTotalMiB)}</span>
        {swapIncreasing && <strong>Increasing</strong>}
      </div>
      <div className="process-table memory-process-table">
        <div className="process-row head">
          <span>PID</span><span>User</span><span>RSS</span><span>VSZ</span><span>MEM</span><span>Command</span>
        </div>
        {metric.topProcesses.map((process) => (
          <div className="process-row" key={process.pid}>
            <span>{process.pid}</span>
            <span title={process.user ?? undefined}>{process.user ?? "-"}</span>
            <span>{formatBytes(process.rssBytes)}</span>
            <span>{formatBytes(process.vszBytes)}</span>
            <span>{formatPercent(process.memoryPercent, 1)}</span>
            <span title={process.command ?? undefined}>{process.command ?? "-"}</span>
          </div>
        ))}
      </div>
    </>
  );
}

export function MemoryUsagePopover({
  metric,
  error,
  loading,
  anchorRef,
  onClose,
  onPopOut,
}: MemoryUsagePopoverProps) {
  return (
    <ResourceDetailPopover
      anchorRef={anchorRef}
      ariaLabel="Memory details"
      title="Memory details"
      icon={<MemoryStick size={16} />}
      headerActions={loading ? <span className="detail-refreshing">Refreshing</span> : null}
      onClose={onClose}
      onPopOut={onPopOut}
    >
      <MemoryDetailContent metric={metric} error={error} />
    </ResourceDetailPopover>
  );
}
