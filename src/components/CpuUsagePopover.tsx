import { Cpu } from "lucide-react";
import type { RefObject } from "react";
import type { CpuDetailMetric } from "../types/resourceDetails";
import {
  DetailUsageBar,
  MetricsUnavailable,
  ResourceDetailPopover,
} from "./ResourceDetailPopover";

type CpuUsagePopoverProps = {
  metric: CpuDetailMetric | null;
  error?: string | null;
  loading: boolean;
  anchorRef: RefObject<HTMLElement | null>;
  onClose: () => void;
};

export function CpuUsagePopover({
  metric,
  error,
  loading,
  anchorRef,
  onClose,
}: CpuUsagePopoverProps) {
  const level = cpuLevel(metric?.usagePercent);
  return (
    <ResourceDetailPopover
      anchorRef={anchorRef}
      ariaLabel="CPU details"
      title="CPU details"
      icon={<Cpu size={16} />}
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
              <span>{metric.modelName ?? "CPU model unavailable"}</span>
              <DetailUsageBar value={metric.usagePercent} level={level} />
            </div>
            <div className="resource-metric-grid">
              <Metric label="Load 1m" value={formatNumber(metric.loadAvg1)} />
              <Metric label="Load 5m" value={formatNumber(metric.loadAvg5)} />
              <Metric label="Load 15m" value={formatNumber(metric.loadAvg15)} />
              <Metric label="Cores" value={formatCores(metric.onlineCores, metric.totalCores)} />
              <Metric label="Clock" value={metric.avgClockGhz == null ? "n/a" : `${metric.avgClockGhz.toFixed(2)} GHz`} />
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
                    <strong>{formatPercent(value)}</strong>
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
                <span>{formatPercent(process.cpuPercent)}</span>
                <span>{formatPercent(process.memoryPercent)}</span>
                <span>{process.elapsedTime ?? "-"}</span>
                <span title={process.command ?? undefined}>{process.command ?? "-"}</span>
              </div>
            ))}
          </div>
        </>
      )}
    </ResourceDetailPopover>
  );
}

function Metric({ label, value }: { label: string; value: string }) {
  return <div><span>{label}</span><strong>{value}</strong></div>;
}

function cpuLevel(value: number | null | undefined) {
  if (value == null) return "unknown" as const;
  if (value >= 95) return "critical" as const;
  if (value >= 80) return "warning" as const;
  return "normal" as const;
}

function formatPercent(value: number | null | undefined) {
  return value == null ? "n/a" : `${value.toFixed(1)}%`;
}

function formatNumber(value: number | null | undefined) {
  return value == null ? "n/a" : value.toFixed(2);
}

function formatCores(online: number | null, total: number | null) {
  if (online == null && total == null) return "n/a";
  return online != null && total != null ? `${online} / ${total}` : String(online ?? total);
}

function formatUptime(value: number | null) {
  if (value == null) return "n/a";
  const days = Math.floor(value / 86400);
  const hours = Math.floor((value % 86400) / 3600);
  const minutes = Math.floor((value % 3600) / 60);
  return `${days}d ${hours}h ${minutes}m`;
}
