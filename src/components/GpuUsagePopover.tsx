import { Gauge } from "lucide-react";
import { useEffect, useMemo, type RefObject } from "react";
import type { GpuDetailMetric } from "../types/resourceDetails";
import {
  formatClock,
  formatMiB,
  formatPercent,
  formatWatts,
  powerLevel,
  ratio,
  temperatureLevel,
  vramLevel,
  type UsageLevel,
} from "../utils/format";
import { GpuSelector } from "./GpuSelector";
import {
  DetailUsageBar,
  Metric,
  ResourceDetailPopover,
} from "./ResourceDetailPopover";

type GpuUsagePopoverProps = {
  metrics: GpuDetailMetric[];
  selectedGpuUuid: string | null;
  onSelectedGpuUuidChange: (gpuUuid: string | null) => void;
  error?: string | null;
  loading: boolean;
  anchorRef: RefObject<HTMLElement | null>;
  onClose: () => void;
};

export function GpuUsagePopover({
  metrics,
  selectedGpuUuid,
  onSelectedGpuUuidChange,
  error,
  loading,
  anchorRef,
  onClose,
}: GpuUsagePopoverProps) {
  useEffect(() => {
    if (metrics.length === 0) {
      return;
    }

    if (!metrics.some((gpu) => gpu.uuid === selectedGpuUuid)) {
      onSelectedGpuUuidChange(metrics[0].uuid);
    }
  }, [metrics, onSelectedGpuUuidChange, selectedGpuUuid]);

  const selectedGpu = useMemo(
    () => metrics.find((gpu) => gpu.uuid === selectedGpuUuid) ?? metrics[0] ?? null,
    [metrics, selectedGpuUuid],
  );
  const selectedProcesses = useMemo(
    () => selectedGpu?.processes.filter((process) => {
      if (process.gpuUuid) {
        return process.gpuUuid === selectedGpu.uuid;
      }
      if (process.gpuIndex != null) {
        return process.gpuIndex === selectedGpu.index;
      }
      return true;
    }) ?? [],
    [selectedGpu],
  );

  return (
    <ResourceDetailPopover
      anchorRef={anchorRef}
      ariaLabel="GPU details"
      title="GPU details"
      icon={<Gauge size={16} />}
      headerActions={loading ? <span className="detail-refreshing">Refreshing</span> : null}
      onClose={onClose}
    >
      {!selectedGpu ? (
        <div className="resource-unavailable">
          <strong>No NVIDIA GPU detected</strong>
          {error && <span>{error}</span>}
        </div>
      ) : (
        <>
          <GpuSelector
            gpus={metrics}
            selectedGpuUuid={selectedGpu.uuid}
            onSelect={onSelectedGpuUuidChange}
          />
          <div className="gpu-detail-heading">
            <strong>{selectedGpu.name}</strong>
            <span title={selectedGpu.uuid}>{selectedGpu.uuid}</span>
          </div>
          <div className="gpu-gauge-grid">
            <GpuGauge label="GPU" value={selectedGpu.gpuUtilPercent} level="normal" />
            <GpuGauge label="VRAM" value={ratio(selectedGpu.memoryUsedMiB, selectedGpu.memoryTotalMiB)} level={vramLevel(selectedGpu.memoryUsedMiB, selectedGpu.memoryTotalMiB)} />
            <GpuGauge label="Power" value={ratio(selectedGpu.powerDrawW, selectedGpu.powerLimitW)} level={powerLevel(selectedGpu.powerDrawW, selectedGpu.powerLimitW)} />
            <GpuGauge label="Temperature" value={selectedGpu.temperatureC} level={temperatureLevel(selectedGpu.temperatureC)} suffix=" C" />
          </div>
          <div className="resource-metric-grid gpu-metric-grid">
            <Metric label="Driver" value={selectedGpu.driverVersion ?? "n/a"} />
            <Metric label="VRAM" value={`${formatMiB(selectedGpu.memoryUsedMiB)} / ${formatMiB(selectedGpu.memoryTotalMiB)}`} />
            <Metric label="VRAM free" value={formatMiB(selectedGpu.memoryFreeMiB)} />
            <Metric label="Power" value={`${formatWatts(selectedGpu.powerDrawW)} / ${formatWatts(selectedGpu.powerLimitW)}`} />
            <Metric label="Fan" value={formatPercent(selectedGpu.fanSpeedPercent)} />
            <Metric label="Graphics clock" value={formatClock(selectedGpu.graphicsClockMHz)} />
            <Metric label="Memory clock" value={formatClock(selectedGpu.memoryClockMHz)} />
            <Metric label="PCI bus" value={selectedGpu.pciBusId ?? "n/a"} />
            <Metric label="Persistence" value={selectedGpu.persistenceMode ?? "n/a"} />
            <Metric label="MIG mode" value={selectedGpu.migMode ?? "n/a"} />
          </div>
          <div className="process-table gpu-process-table">
            <div className="process-row head">
              <span>GPU</span><span>PID</span><span>User</span><span>GPU memory</span><span>Process</span><span>Command</span>
            </div>
            {selectedProcesses.length === 0 && <div className="empty-list compact">No compute processes</div>}
            {selectedProcesses.map((process, index) => (
              <div
                className="process-row"
                key={`${process.gpuUuid ?? process.gpuIndex ?? "unknown"}:${process.pid}:${index}`}
              >
                <span>{process.gpuIndex ?? "-"}</span>
                <span>{process.pid}</span>
                <span>{process.user ?? "-"}</span>
                <span>{formatMiB(process.usedMemoryMiB)}</span>
                <span title={process.processName ?? undefined}>{process.processName ?? "-"}</span>
                <span title={process.command ?? undefined}>{process.command ?? "-"}</span>
              </div>
            ))}
          </div>
        </>
      )}
    </ResourceDetailPopover>
  );
}

function GpuGauge({ label, value, level, suffix = "%" }: { label: string; value: number | null; level: UsageLevel; suffix?: string }) {
  return <div><span>{label}</span><strong className={level}>{value == null ? "n/a" : `${value.toFixed(0)}${suffix}`}</strong><DetailUsageBar value={value} level={level} /></div>;
}
