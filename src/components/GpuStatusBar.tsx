import { Activity, Gauge, Thermometer, Zap } from "lucide-react";
import { useSessionStore } from "../stores/sessionStore";
import type { GpuMetric } from "../types/gpu";

export function GpuStatusBar() {
  const connected = useSessionStore((state) => state.connected);
  const gpuStatus = useSessionStore((state) => state.gpuStatus);

  return (
    <footer className="gpu-statusbar">
      <div className="gpu-status-label">
        <Activity size={16} />
        <span>GPU</span>
      </div>
      {!connected && <div className="gpu-unavailable">GPU metrics unavailable</div>}
      {connected && !gpuStatus && (
        <div className="gpu-unavailable">Waiting for GPU metrics</div>
      )}
      {connected && gpuStatus?.status === "unavailable" && (
        <div className="gpu-unavailable">
          {gpuStatus.message ?? "GPU metrics unavailable"}
        </div>
      )}
      {connected &&
        gpuStatus?.status === "available" &&
        gpuStatus.metrics.map((metric) => (
          <GpuCard metric={metric} key={metric.uuid || metric.index} />
        ))}
    </footer>
  );
}

function GpuCard({ metric }: { metric: GpuMetric }) {
  const memoryPercent =
    metric.memoryTotalMiB && metric.memoryUsedMiB != null
      ? Math.round((metric.memoryUsedMiB / metric.memoryTotalMiB) * 100)
      : null;

  return (
    <div className="gpu-card">
      <div className="gpu-card-head">
        <strong>
          GPU {metric.index} · {metric.name}
        </strong>
        <span>{metric.driverVersion}</span>
      </div>
      <div className="gpu-metric-row">
        <Gauge size={14} />
        <span>{formatPercent(metric.gpuUtilPercent)}</span>
        <MiniBar value={metric.gpuUtilPercent} />
      </div>
      <div className="gpu-metric-row">
        <span className="metric-dot memory" />
        <span>
          {formatMiB(metric.memoryUsedMiB)} / {formatMiB(metric.memoryTotalMiB)}
        </span>
        <MiniBar value={memoryPercent} />
      </div>
      <div className="gpu-metric-pair">
        <span>
          <Zap size={14} />
          {formatWatts(metric.powerDrawW)} / {formatWatts(metric.powerLimitW)}
        </span>
        <span>
          <Thermometer size={14} />
          {formatTemperature(metric.temperatureC)}
        </span>
      </div>
    </div>
  );
}

function MiniBar({ value }: { value: number | null }) {
  const width = value == null ? 0 : Math.max(0, Math.min(100, value));
  return (
    <div className="mini-bar">
      <div className="mini-bar-fill" style={{ width: `${width}%` }} />
    </div>
  );
}

function formatPercent(value: number | null) {
  return value == null ? "n/a" : `${Math.round(value)}%`;
}

function formatWatts(value: number | null) {
  return value == null ? "n/a" : `${value.toFixed(0)} W`;
}

function formatTemperature(value: number | null) {
  return value == null ? "n/a" : `${value.toFixed(0)} C`;
}

function formatMiB(value: number | null) {
  if (value == null) {
    return "n/a";
  }
  if (value >= 1024) {
    return `${(value / 1024).toFixed(value >= 10 * 1024 ? 1 : 2)} GiB`;
  }
  return `${value} MiB`;
}
