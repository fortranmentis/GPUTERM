import { invoke } from "@tauri-apps/api/core";
import {
  Activity,
  Cpu,
  Gauge,
  HardDrive,
  MemoryStick,
  Thermometer,
  Zap,
} from "lucide-react";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { useSessionStore } from "../stores/sessionStore";
import type {
  DiskMetric,
  GpuMetric,
  TelemetryDisplayMode,
  TelemetrySettings,
} from "../types/gpu";

export function RemoteTelemetryBar() {
  const connected = useSessionStore((state) => state.connected);
  const telemetry = useSessionStore((state) => state.remoteTelemetry);
  const settings = useSessionStore((state) => state.telemetrySettings);
  const setTelemetrySettings = useSessionStore(
    (state) => state.setTelemetrySettings,
  );
  const setMessage = useSessionStore((state) => state.setMessage);
  const [diskDetailsOpen, setDiskDetailsOpen] = useState(false);
  const [ignoreDraft, setIgnoreDraft] = useState(
    settings.diskIgnoreFsTypes.join(", "),
  );

  useEffect(() => {
    setIgnoreDraft(settings.diskIgnoreFsTypes.join(", "));
  }, [settings.diskIgnoreFsTypes]);

  const showSystem = settings.displayMode !== "gpu-only";
  const showGpu = settings.displayMode !== "system-only";
  const primaryDisks = useMemo(
    () => telemetry?.disks.slice(0, 3) ?? [],
    [telemetry?.disks],
  );

  const updateSettings = async (nextSettings: TelemetrySettings) => {
    setTelemetrySettings(nextSettings);
    try {
      const saved = await invoke<TelemetrySettings>("update_telemetry_settings", {
        settings: nextSettings,
      });
      setTelemetrySettings(saved);
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
    }
  };

  const applyIgnoredFsTypes = () => {
    updateSettings({
      ...settings,
      diskIgnoreFsTypes: ignoreDraft
        .split(",")
        .map((item) => item.trim())
        .filter(Boolean),
    });
  };

  return (
    <footer className="telemetry-bar">
      <div className="telemetry-settings">
        <div className="telemetry-status-label">
          <Activity size={16} />
          <span>{telemetry?.hostname ?? "Telemetry"}</span>
        </div>
        <label className="mini-control">
          <span>Interval</span>
          <select
            value={settings.telemetryIntervalSecs}
            onChange={(event) =>
              updateSettings({
                ...settings,
                telemetryIntervalSecs: Number(event.target.value) as 1 | 2 | 5 | 10,
              })
            }
          >
            {[1, 2, 5, 10].map((value) => (
              <option value={value} key={value}>
                {value}s
              </option>
            ))}
          </select>
        </label>
        <label className="mini-control">
          <span>Mode</span>
          <select
            value={settings.displayMode}
            onChange={(event) =>
              updateSettings({
                ...settings,
                displayMode: event.target.value as TelemetryDisplayMode,
              })
            }
          >
            <option value="gpu-system">GPU + System</option>
            <option value="gpu-only">GPU only</option>
            <option value="system-only">System only</option>
          </select>
        </label>
        <label className="mini-control ignore-control">
          <span>Ignore FS</span>
          <input
            value={ignoreDraft}
            onChange={(event) => setIgnoreDraft(event.target.value)}
            onBlur={applyIgnoredFsTypes}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                applyIgnoredFsTypes();
              }
            }}
          />
        </label>
      </div>

      {!connected && (
        <div className="telemetry-unavailable">Remote telemetry unavailable</div>
      )}
      {connected && !telemetry && (
        <div className="telemetry-unavailable">Waiting for remote telemetry</div>
      )}

      {connected && telemetry && showSystem && (
        <>
          <TelemetrySection title="CPU" icon={<Cpu size={16} />}>
            {telemetry.cpu ? (
              <>
                <strong>{formatPercent(telemetry.cpu.usagePercent)}</strong>
                <span>
                  Load {formatNumber(telemetry.cpu.loadAvg1)} /{" "}
                  {formatNumber(telemetry.cpu.loadAvg5)} /{" "}
                  {formatNumber(telemetry.cpu.loadAvg15)}
                </span>
                <span>
                  {formatCores(telemetry.cpu.onlineCores, telemetry.cpu.totalCores)} |{" "}
                  {formatGhz(telemetry.cpu.avgClockGhz)}
                </span>
                <small title={telemetry.cpu.modelName ?? undefined}>
                  {telemetry.cpu.modelName ?? "CPU model unavailable"}
                </small>
              </>
            ) : (
              <span>{telemetry.errors.cpu ?? "CPU unavailable"}</span>
            )}
          </TelemetrySection>

          <TelemetrySection title="RAM" icon={<MemoryStick size={16} />}>
            {telemetry.memory ? (
              <>
                <strong>
                  {formatGiBFromMiB(telemetry.memory.usedMiB)} /{" "}
                  {formatGiBFromMiB(telemetry.memory.totalMiB)}
                </strong>
                <span>
                  Available {formatGiBFromMiB(telemetry.memory.availableMiB)}
                </span>
                <span>
                  Swap {formatGiBFromMiB(telemetry.memory.swapUsedMiB)} /{" "}
                  {formatGiBFromMiB(telemetry.memory.swapTotalMiB)}
                </span>
                <MiniBar value={telemetry.memory.usagePercent} />
              </>
            ) : (
              <span>{telemetry.errors.memory ?? "Memory unavailable"}</span>
            )}
          </TelemetrySection>

          <button
            className="telemetry-section disk-section"
            type="button"
            onClick={() => setDiskDetailsOpen((open) => !open)}
          >
            <div className="telemetry-section-title">
              <HardDrive size={16} />
              <span>Disk</span>
            </div>
            {primaryDisks.length > 0 ? (
              <div className="disk-summary-list">
                {primaryDisks.map((disk) => (
                  <DiskSummary disk={disk} key={`${disk.filesystem}:${disk.mountPoint}`} />
                ))}
              </div>
            ) : (
              <span>{telemetry.errors.disk ?? "Disk unavailable"}</span>
            )}
          </button>
        </>
      )}

      {connected && telemetry && showGpu && (
        <div className="gpu-telemetry-group">
          {telemetry.gpu.length > 0 ? (
            telemetry.gpu.map((metric) => (
              <GpuTelemetryCard metric={metric} key={metric.uuid || metric.index} />
            ))
          ) : (
            <TelemetrySection title="GPU" icon={<Gauge size={16} />}>
              <span>{telemetry.errors.gpu ?? "GPU metrics unavailable"}</span>
            </TelemetrySection>
          )}
        </div>
      )}

      {diskDetailsOpen && telemetry && (
        <div className="disk-detail-popover">
          <div className="disk-detail-title">
            <HardDrive size={16} />
            <strong>Disks</strong>
            <span>{telemetry.disks.length}</span>
          </div>
          <div className="disk-detail-table">
            <div className="disk-detail-row head">
              <span>Mount</span>
              <span>Type</span>
              <span>Used</span>
              <span>Total</span>
              <span>Use</span>
            </div>
            {telemetry.disks.map((disk) => (
              <div
                className="disk-detail-row"
                key={`${disk.filesystem}:${disk.mountPoint}`}
              >
                <span title={disk.mountPoint}>{disk.mountPoint}</span>
                <span>{disk.fsType ?? "-"}</span>
                <span>{formatBytes(disk.usedBytes)}</span>
                <span>{formatBytes(disk.totalBytes)}</span>
                <span>{formatPercent(disk.usagePercent)}</span>
              </div>
            ))}
          </div>
        </div>
      )}
    </footer>
  );
}

function TelemetrySection({
  title,
  icon,
  children,
}: {
  title: string;
  icon: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="telemetry-section">
      <div className="telemetry-section-title">
        {icon}
        <span>{title}</span>
      </div>
      <div className="telemetry-section-body">{children}</div>
    </section>
  );
}

function DiskSummary({ disk }: { disk: DiskMetric }) {
  return (
    <div className="disk-summary">
      <strong>{disk.mountPoint}</strong>
      <span>
        {formatBytes(disk.usedBytes)} / {formatBytes(disk.totalBytes)}
      </span>
      <MiniBar value={disk.usagePercent} />
    </div>
  );
}

function GpuTelemetryCard({ metric }: { metric: GpuMetric }) {
  const memoryPercent =
    metric.memoryTotalMiB && metric.memoryUsedMiB != null
      ? (metric.memoryUsedMiB / metric.memoryTotalMiB) * 100
      : null;

  return (
    <section className="telemetry-section gpu-section">
      <div className="telemetry-section-title">
        <Gauge size={16} />
        <span>GPU{metric.index}</span>
      </div>
      <div className="telemetry-section-body">
        <strong title={metric.name}>{metric.name}</strong>
        <span>
          {formatPercent(metric.gpuUtilPercent)} | VRAM{" "}
          {formatGiBFromMiB(metric.memoryUsedMiB)} /{" "}
          {formatGiBFromMiB(metric.memoryTotalMiB)}
        </span>
        <span>
          <Zap size={13} /> {formatWatts(metric.powerDrawW)} /{" "}
          {formatWatts(metric.powerLimitW)}
          <Thermometer size={13} /> {formatTemperature(metric.temperatureC)}
        </span>
        <MiniBar value={memoryPercent} />
      </div>
    </section>
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

function formatPercent(value: number | null | undefined) {
  return value == null ? "n/a" : `${Math.round(value)}%`;
}

function formatNumber(value: number | null | undefined) {
  return value == null ? "n/a" : value.toFixed(2);
}

function formatGhz(value: number | null | undefined) {
  return value == null ? "n/a GHz" : `${value.toFixed(1)} GHz`;
}

function formatCores(online: number | null, total: number | null) {
  if (online == null && total == null) {
    return "cores n/a";
  }
  if (online != null && total != null && online !== total) {
    return `${online}/${total} cores`;
  }
  return `${total ?? online} cores`;
}

function formatWatts(value: number | null) {
  return value == null ? "n/a" : `${value.toFixed(0)} W`;
}

function formatTemperature(value: number | null) {
  return value == null ? "n/a" : `${value.toFixed(0)} C`;
}

function formatGiBFromMiB(value: number | null) {
  if (value == null) {
    return "n/a";
  }
  return `${(value / 1024).toFixed(value >= 10 * 1024 ? 1 : 2)} GiB`;
}

function formatBytes(value: number | null) {
  if (value == null) {
    return "n/a";
  }
  const tib = 1024 ** 4;
  const gib = 1024 ** 3;
  if (value >= tib) {
    return `${(value / tib).toFixed(1)} TiB`;
  }
  return `${(value / gib).toFixed(value >= 10 * gib ? 1 : 2)} GiB`;
}
