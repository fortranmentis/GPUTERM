import { invoke } from "@tauri-apps/api/core";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import {
  Activity,
  Cpu,
  Gauge,
  HardDrive,
  MemoryStick,
  PanelBottomClose,
  Thermometer,
  Users,
  Zap,
} from "lucide-react";
import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
  type RefObject,
} from "react";
import { CpuUsagePopover } from "./CpuUsagePopover";
import { DiskUsagePopover } from "./DiskUsagePopover";
import { GpuUsagePopover } from "./GpuUsagePopover";
import { MemoryUsagePopover } from "./MemoryUsagePopover";
import { UsersPopover } from "./UsersPopover";
import {
  selectActiveTelemetry,
  selectIsActiveConnected,
  useSessionStore,
} from "../stores/sessionStore";
import type {
  GpuMetric,
  TelemetryDisplayMode,
  TelemetrySettings,
} from "../types/gpu";
import type {
  ResourceDetails,
  ResourceDetailType,
} from "../types/resourceDetails";
import {
  createDiskSummary,
  formatDiskUsagePercent,
} from "../utils/diskPriority";
import {
  formatCoreCount,
  formatGhz,
  formatGiBFromMiB,
  formatNumber,
  formatPercent,
  formatTemperature,
  formatWatts,
} from "../utils/format";

type OpenResource = ResourceDetailType | "disk" | "users" | null;

const DETAIL_TITLES: Record<Exclude<OpenResource, null>, string> = {
  cpu: "CPU details",
  memory: "Memory details",
  gpu: "GPU details",
  disk: "Disks",
  users: "Logged-in users",
};

type RemoteTelemetryBarProps = {
  onClose?: () => void;
};

export function RemoteTelemetryBar({ onClose }: RemoteTelemetryBarProps = {}) {
  const activeSessionId = useSessionStore((state) => state.activeSessionId);
  const sessionConnected = useSessionStore(selectIsActiveConnected);
  const connected = sessionConnected;
  const telemetry = useSessionStore(selectActiveTelemetry);
  const settings = useSessionStore((state) => state.telemetrySettings);
  const setTelemetrySettings = useSessionStore((state) => state.setTelemetrySettings);
  const setMessage = useSessionStore((state) => state.setMessage);
  const [openResource, setOpenResource] = useState<OpenResource>(null);
  const [resourceDetails, setResourceDetails] = useState<ResourceDetails | null>(null);
  const [detailsLoading, setDetailsLoading] = useState(false);
  const [detailsRequestError, setDetailsRequestError] = useState<string | null>(null);
  const [selectedGpuUuid, setSelectedGpuUuid] = useState<string | null>(null);
  const cpuButtonRef = useRef<HTMLButtonElement | null>(null);
  const memoryButtonRef = useRef<HTMLButtonElement | null>(null);
  const gpuButtonRef = useRef<HTMLElement | null>(null);
  const gpuAnchorUuidRef = useRef<string | null>(null);
  const diskButtonRef = useRef<HTMLButtonElement | null>(null);
  const usersButtonRef = useRef<HTMLButtonElement | null>(null);
  const [ignoreDraft, setIgnoreDraft] = useState(settings.diskIgnoreFsTypes.join(", "));

  useEffect(() => {
    setIgnoreDraft(settings.diskIgnoreFsTypes.join(", "));
  }, [settings.diskIgnoreFsTypes]);

  useEffect(() => {
    setSelectedGpuUuid(null);
    gpuAnchorUuidRef.current = null;
    setResourceDetails(null);
    setOpenResource(null);
  }, [activeSessionId]);

  const showSystem = settings.displayMode !== "gpu-only";
  const showGpu = settings.displayMode !== "system-only";
  const diskSummary = useMemo(
    () => createDiskSummary(telemetry?.disks ?? [], 2, settings.diskIgnoreFsTypes),
    [settings.diskIgnoreFsTypes, telemetry?.disks],
  );
  const uniqueUsers = useMemo(
    () => [...new Set((telemetry?.users ?? []).map((session) => session.user))],
    [telemetry?.users],
  );

  useEffect(() => {
    if (!connected) {
      setOpenResource(null);
    } else if (!showSystem && ["cpu", "memory", "disk", "users"].includes(openResource ?? "")) {
      setOpenResource(null);
    } else if (!showGpu && openResource === "gpu") {
      setOpenResource(null);
    }
  }, [connected, openResource, showGpu, showSystem]);

  useEffect(() => {
    if (
      !connected ||
      !activeSessionId ||
      !openResource ||
      openResource === "disk" ||
      openResource === "users"
    ) {
      setResourceDetails(null);
      setDetailsLoading(false);
      setDetailsRequestError(null);
      return;
    }

    let disposed = false;
    let inFlight = false;
    setResourceDetails(null);
    setDetailsRequestError(null);

    const loadDetails = async () => {
      if (inFlight) {
        return;
      }
      inFlight = true;
      setDetailsLoading(true);
      try {
        const details = await invoke<ResourceDetails>("get_resource_details", {
          sessionId: activeSessionId,
          resourceType: openResource,
        });
        if (!disposed) {
          setResourceDetails(details);
          setDetailsRequestError(null);
        }
      } catch (error) {
        if (!disposed) {
          setDetailsRequestError(String(error));
        }
      } finally {
        inFlight = false;
        if (!disposed) {
          setDetailsLoading(false);
        }
      }
    };

    void loadDetails();
    const intervalMs = Math.max(1, settings.telemetryIntervalSecs) * 1000;
    const timer = window.setInterval(loadDetails, intervalMs);
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, [activeSessionId, connected, openResource, settings.telemetryIntervalSecs]);

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
    void updateSettings({
      ...settings,
      diskIgnoreFsTypes: ignoreDraft
        .split(",")
        .map((item) => item.trim())
        .filter(Boolean),
    });
  };

  const openDetail = (resource: OpenResource) => {
    setOpenResource((current) => (current === resource ? null : resource));
  };

  const popOut = async (resource: Exclude<OpenResource, null>) => {
    if (!activeSessionId) {
      return;
    }
    const label = `detail-${resource}-${activeSessionId}`;
    try {
      const existing = await WebviewWindow.getByLabel(label);
      if (existing) {
        await existing.setFocus();
        setOpenResource(null);
        return;
      }
      const detailWindow = new WebviewWindow(label, {
        url: `/?window=detail&session=${encodeURIComponent(activeSessionId)}&resource=${resource}`,
        title: `${DETAIL_TITLES[resource]} — ${telemetry?.hostname ?? "GpuTerm"}`,
        width: 780,
        height: 600,
        minWidth: 420,
        minHeight: 320,
      });
      void detailWindow.once("tauri://error", (event) => {
        setMessage({ kind: "error", text: String(event.payload) });
      });
      setOpenResource(null);
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
    }
  };

  const openGpuDetail = (
    event: ReactMouseEvent<HTMLButtonElement>,
    gpuUuid: string | null,
  ) => {
    gpuButtonRef.current = event.currentTarget;
    const reopeningFromSameCard =
      openResource !== "gpu" && gpuAnchorUuidRef.current === gpuUuid;
    if (gpuUuid && !reopeningFromSameCard) {
      setSelectedGpuUuid(gpuUuid);
    }
    gpuAnchorUuidRef.current = gpuUuid;
    setOpenResource("gpu");
  };

  return (
    <footer className="telemetry-bar">
      <div className="telemetry-settings">
        <div className="telemetry-status-label">
          <Activity size={16} />
          <span>{telemetry?.hostname ?? "Telemetry"}</span>
          {onClose && (
            <button
              className="icon-button ghost telemetry-close-button"
              type="button"
              aria-label="Close monitoring panel"
              title="Close monitoring panel"
              onClick={onClose}
            >
              <PanelBottomClose size={17} />
            </button>
          )}
        </div>
        <label className="mini-control">
          <span>Interval</span>
          <select
            value={settings.telemetryIntervalSecs}
            onChange={(event) =>
              void updateSettings({
                ...settings,
                telemetryIntervalSecs: Number(event.target.value) as 1 | 2 | 5 | 10,
              })
            }
          >
            {[1, 2, 5, 10].map((value) => (
              <option value={value} key={value}>{value}s</option>
            ))}
          </select>
        </label>
        <label className="mini-control">
          <span>Mode</span>
          <select
            value={settings.displayMode}
            onChange={(event) =>
              void updateSettings({
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
              if (event.key === "Enter") applyIgnoredFsTypes();
            }}
          />
        </label>
      </div>

      {!connected && <div className="telemetry-unavailable">Telemetry unavailable</div>}
      {connected && !telemetry && <div className="telemetry-unavailable">Waiting for telemetry</div>}

      {connected && telemetry && showSystem && (
        <>
          <TelemetryButton
            buttonRef={cpuButtonRef}
            title="CPU"
            icon={<Cpu size={16} />}
            expanded={openResource === "cpu"}
            onClick={() => openDetail("cpu")}
          >
            {telemetry.cpu ? (
              <>
                <strong>{formatPercent(telemetry.cpu.usagePercent)}</strong>
                <span>Load {formatNumber(telemetry.cpu.loadAvg1)} / {formatNumber(telemetry.cpu.loadAvg5)} / {formatNumber(telemetry.cpu.loadAvg15)}</span>
                <span>{formatCoreCount(telemetry.cpu.onlineCores, telemetry.cpu.totalCores)} | {formatGhz(telemetry.cpu.avgClockGhz)}</span>
                <small title={telemetry.cpu.modelName ?? undefined}>{telemetry.cpu.modelName ?? "CPU model unavailable"}</small>
              </>
            ) : (
              <span>{telemetry.errors.cpu ?? "CPU unavailable"}</span>
            )}
          </TelemetryButton>

          <TelemetryButton
            buttonRef={memoryButtonRef}
            title="RAM"
            icon={<MemoryStick size={16} />}
            expanded={openResource === "memory"}
            onClick={() => openDetail("memory")}
          >
            {telemetry.memory ? (
              <>
                <strong>{formatGiBFromMiB(telemetry.memory.usedMiB)} / {formatGiBFromMiB(telemetry.memory.totalMiB)}</strong>
                <span>Available {formatGiBFromMiB(telemetry.memory.availableMiB)}</span>
                <span>Swap {formatGiBFromMiB(telemetry.memory.swapUsedMiB)} / {formatGiBFromMiB(telemetry.memory.swapTotalMiB)}</span>
                <MiniBar value={telemetry.memory.usagePercent} />
              </>
            ) : (
              <span>{telemetry.errors.memory ?? "Memory unavailable"}</span>
            )}
          </TelemetryButton>

          <button
            ref={diskButtonRef}
            className="telemetry-section disk-section"
            type="button"
            aria-expanded={openResource === "disk"}
            onClick={() => openDetail("disk")}
          >
            <div className="telemetry-section-title"><HardDrive size={16} /><span>Disk</span></div>
            {diskSummary.visible.length > 0 ? (
              <div className="disk-summary-compact">
                {diskSummary.visible.map((disk) => (
                  <span key={`${disk.filesystem}:${disk.mountPoint}`}><strong>{disk.mountPoint}</strong> {formatDiskUsagePercent(disk.usagePercent)}</span>
                ))}
                {diskSummary.hiddenCount > 0 && <span className="disk-hidden-count">+{diskSummary.hiddenCount}</span>}
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
              <GpuTelemetryCard
                metric={metric}
                expanded={openResource === "gpu" && selectedGpuUuid === metric.uuid}
                key={metric.uuid || metric.index}
                onClick={(event) => openGpuDetail(event, metric.uuid)}
              />
            ))
          ) : (
            <TelemetryButton
              buttonRef={gpuButtonRef as RefObject<HTMLButtonElement | null>}
              title="GPU"
              icon={<Gauge size={16} />}
              expanded={openResource === "gpu"}
              onClick={(event) => openGpuDetail(event, null)}
            >
              <span>{telemetry.errors.gpu ?? "GPU metrics unavailable"}</span>
            </TelemetryButton>
          )}
        </div>
      )}

      {connected && telemetry && showSystem && (
        <TelemetryButton
          buttonRef={usersButtonRef}
          title="Users"
          icon={<Users size={16} />}
          expanded={openResource === "users"}
          onClick={() => openDetail("users")}
        >
          {telemetry.errors.users && telemetry.users.length === 0 ? (
            <span>{telemetry.errors.users}</span>
          ) : (
            <>
              <strong>
                {uniqueUsers.length} {uniqueUsers.length === 1 ? "user" : "users"}
              </strong>
              <span>
                {uniqueUsers.length > 0
                  ? uniqueUsers.slice(0, 3).join(", ") +
                    (uniqueUsers.length > 3 ? ` +${uniqueUsers.length - 3}` : "")
                  : "No login sessions"}
              </span>
              <span>
                {telemetry.users.length}{" "}
                {telemetry.users.length === 1 ? "session" : "sessions"}
              </span>
            </>
          )}
        </TelemetryButton>
      )}

      {openResource === "cpu" && (
        <CpuUsagePopover
          metric={resourceDetails?.cpu ?? null}
          error={resourceDetails?.errors.cpu ?? detailsRequestError}
          loading={detailsLoading}
          anchorRef={cpuButtonRef}
          onClose={() => setOpenResource(null)}
          onPopOut={() => void popOut("cpu")}
        />
      )}
      {openResource === "memory" && (
        <MemoryUsagePopover
          metric={resourceDetails?.memory ?? null}
          error={resourceDetails?.errors.memory ?? detailsRequestError}
          loading={detailsLoading}
          anchorRef={memoryButtonRef}
          onClose={() => setOpenResource(null)}
          onPopOut={() => void popOut("memory")}
        />
      )}
      {openResource === "gpu" && (
        <GpuUsagePopover
          metrics={resourceDetails?.gpus ?? []}
          selectedGpuUuid={selectedGpuUuid}
          onSelectedGpuUuidChange={setSelectedGpuUuid}
          error={resourceDetails?.errors.gpu ?? detailsRequestError}
          loading={detailsLoading}
          anchorRef={gpuButtonRef}
          onClose={() => setOpenResource(null)}
          onPopOut={() => void popOut("gpu")}
        />
      )}
      {openResource === "disk" && telemetry && (
        <DiskUsagePopover
          disks={telemetry.disks}
          ignoredFsTypes={settings.diskIgnoreFsTypes}
          anchorRef={diskButtonRef}
          onClose={() => setOpenResource(null)}
          onPopOut={() => void popOut("disk")}
        />
      )}
      {openResource === "users" && telemetry && (
        <UsersPopover
          users={telemetry.users}
          error={telemetry.errors.users}
          anchorRef={usersButtonRef}
          onClose={() => setOpenResource(null)}
          onPopOut={() => void popOut("users")}
        />
      )}
    </footer>
  );
}

function TelemetryButton({
  buttonRef,
  title,
  icon,
  expanded,
  onClick,
  children,
}: {
  buttonRef: RefObject<HTMLButtonElement | null>;
  title: string;
  icon: ReactNode;
  expanded: boolean;
  onClick: (event: ReactMouseEvent<HTMLButtonElement>) => void;
  children: ReactNode;
}) {
  return (
    <button
      ref={buttonRef}
      className="telemetry-section"
      type="button"
      aria-expanded={expanded}
      onClick={onClick}
    >
      <div className="telemetry-section-title">{icon}<span>{title}</span></div>
      <div className="telemetry-section-body">{children}</div>
    </button>
  );
}

function GpuTelemetryCard({
  metric,
  expanded,
  onClick,
}: {
  metric: GpuMetric;
  expanded: boolean;
  onClick: (event: ReactMouseEvent<HTMLButtonElement>) => void;
}) {
  const memoryPercent = metric.memoryTotalMiB && metric.memoryUsedMiB != null
    ? metric.memoryUsedMiB / metric.memoryTotalMiB * 100
    : null;
  return (
    <button className="telemetry-section gpu-section" type="button" aria-expanded={expanded} onClick={onClick}>
      <div className="telemetry-section-title">
        <Gauge size={16} />
        <span>GPU{metric.index}</span>
        <span className={`gpu-vendor-tag ${metric.vendor}`}>{metric.vendor.toUpperCase()}</span>
      </div>
      <div className="telemetry-section-body">
        <strong title={metric.name}>{metric.name}</strong>
        <span>{formatPercent(metric.gpuUtilPercent)} | VRAM {formatGiBFromMiB(metric.memoryUsedMiB)} / {formatGiBFromMiB(metric.memoryTotalMiB)}</span>
        <span><Zap size={13} /> {formatWatts(metric.powerDrawW)} / {formatWatts(metric.powerLimitW)} <Thermometer size={13} /> {formatTemperature(metric.temperatureC)}</span>
        <MiniBar value={memoryPercent} />
      </div>
    </button>
  );
}

function MiniBar({ value }: { value: number | null }) {
  const width = value == null ? 0 : Math.max(0, Math.min(100, value));
  return <div className="mini-bar"><div className="mini-bar-fill" style={{ width: `${width}%` }} /></div>;
}
