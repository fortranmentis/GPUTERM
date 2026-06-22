import { MemoryStick } from "lucide-react";
import { useEffect, useRef, useState, type RefObject } from "react";
import type { MemoryDetailMetric } from "../types/resourceDetails";
import { formatBytes } from "../utils/formatBytes";
import {
  DetailUsageBar,
  MetricsUnavailable,
  ResourceDetailPopover,
} from "./ResourceDetailPopover";

type MemoryUsagePopoverProps = {
  metric: MemoryDetailMetric | null;
  error?: string | null;
  loading: boolean;
  anchorRef: RefObject<HTMLElement | null>;
  onClose: () => void;
};

export function MemoryUsagePopover({
  metric,
  error,
  loading,
  anchorRef,
  onClose,
}: MemoryUsagePopoverProps) {
  const previousSwap = useRef<number | null>(null);
  const [swapIncreasing, setSwapIncreasing] = useState(false);

  useEffect(() => {
    const current = metric?.swapUsedMiB ?? null;
    if (current != null) {
      setSwapIncreasing(previousSwap.current != null && current > previousSwap.current && current > 0);
      previousSwap.current = current;
    }
  }, [metric?.swapUsedMiB]);

  const level = memoryLevel(metric?.usagePercent);
  return (
    <ResourceDetailPopover
      anchorRef={anchorRef}
      ariaLabel="Memory details"
      title="Memory details"
      icon={<MemoryStick size={16} />}
      headerActions={loading ? <span className="detail-refreshing">Refreshing</span> : null}
      onClose={onClose}
    >
      {!metric ? (
        <MetricsUnavailable error={error} />
      ) : (
        <>
          <div className="resource-detail-summary">
            <div className="resource-primary-metric">
              <strong className={level}>{formatPercent(metric.usagePercent)}</strong>
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
                <span>{formatPercent(process.memoryPercent)}</span>
                <span title={process.command ?? undefined}>{process.command ?? "-"}</span>
              </div>
            ))}
          </div>
        </>
      )}
    </ResourceDetailPopover>
  );
}

function Metric({ label, value, title, warning }: { label: string; value: string; title?: string; warning?: boolean }) {
  return <div className={warning ? "warning" : ""} title={title}><span>{label}</span><strong>{value}</strong></div>;
}

function memoryLevel(value: number | null | undefined) {
  if (value == null) return "unknown" as const;
  if (value >= 95) return "critical" as const;
  if (value >= 85) return "warning" as const;
  return "normal" as const;
}

function formatPercent(value: number | null | undefined) {
  return value == null ? "n/a" : `${value.toFixed(1)}%`;
}

function formatMiB(value: number | null | undefined) {
  return value == null ? "n/a" : formatBytes(value * 1024 * 1024);
}
