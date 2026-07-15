import { Cpu } from "lucide-react";
import type { RefObject } from "react";
import type { CpuDetailMetric } from "../types/resourceDetails";
import {
  cpuLevel,
  formatCoreRatio,
  formatGhz,
  formatNumber,
  formatPercent,
  formatUptime,
} from "../utils/format";
import {
  DetailUsageBar,
  Metric,
  MetricsUnavailable,
  ResourceDetailPopover,
} from "./ResourceDetailPopover";

type CpuUsagePopoverProps = {
  metric: CpuDetailMetric | null;
  error?: string | null;
  loading: boolean;
  anchorRef: RefObject<HTMLElement | null>;
  onClose: () => void;
  onPopOut?: () => void;
};

export function CpuDetailContent({
  metric,
  error,
}: {
  metric: CpuDetailMetric | null;
  error?: string | null;
}) {
  if (!metric) {
    return <MetricsUnavailable error={error} />;
  }
  const level = cpuLevel(metric.usagePercent);
  return (
    <>
      <div className="resource-detail-summary">
        <div className="resource-primary-metric">
          <strong className={level}>{formatPercent(metric.usagePercent, 1)}</strong>
          <span>{metric.modelName ?? "CPU model unavailable"}</span>
          <DetailUsageBar value={metric.usagePercent} level={level} />
        </div>
        <div className="resource-metric-grid">
          <Metric label="Load 1m" value={formatNumber(metric.loadAvg1)} />
          <Metric label="Load 5m" value={formatNumber(metric.loadAvg5)} />
          <Metric label="Load 15m" value={formatNumber(metric.loadAvg15)} />
          <Metric label="Cores" value={formatCoreRatio(metric.onlineCores, metric.totalCores)} />
          <Metric label="Clock" value={formatGhz(metric.avgClockGhz, 2, "n/a")} />
          <Metric label="Uptime" value={formatUptime(metric.uptimeSeconds)} />
        </div>
      </div>

      {metric.logicalCoreUsagePercent.length > 0 && (
        <details className="logical-core-details">
          <summary>Logical CPU usage</summary>
          <div className="logical-core-grid">
            {metric.logicalCoreUsagePercent.map((value, index) => (
              <div key={index}>
                <span>CPU{index}</span>
                <strong>{formatPercent(value, 1)}</strong>
                <DetailUsageBar value={value} level={cpuLevel(value)} />
              </div>
            ))}
          </div>
        </details>
      )}

      <div className="process-table cpu-process-table">
        <div className="process-row head">
          <span>PID</span><span>User</span><span>CPU</span><span>MEM</span><span>Elapsed</span><span>Command</span>
        </div>
        {metric.topProcesses.map((process) => (
          <div className="process-row" key={process.pid}>
            <span>{process.pid}</span>
            <span title={process.user ?? undefined}>{process.user ?? "-"}</span>
            <span>{formatPercent(process.cpuPercent, 1)}</span>
            <span>{formatPercent(process.memoryPercent, 1)}</span>
            <span>{process.elapsedTime ?? "-"}</span>
            <span title={process.command ?? undefined}>{process.command ?? "-"}</span>
          </div>
        ))}
      </div>
    </>
  );
}

export function CpuUsagePopover({
  metric,
  error,
  loading,
  anchorRef,
  onClose,
  onPopOut,
}: CpuUsagePopoverProps) {
  return (
    <ResourceDetailPopover
      anchorRef={anchorRef}
      ariaLabel="CPU details"
      title="CPU details"
      icon={<Cpu size={16} />}
      headerActions={loading ? <span className="detail-refreshing">Refreshing</span> : null}
      onClose={onClose}
      onPopOut={onPopOut}
    >
      <CpuDetailContent metric={metric} error={error} />
    </ResourceDetailPopover>
  );
}
