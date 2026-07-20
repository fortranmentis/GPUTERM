import { useEffect, useMemo, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Cpu, Gauge, HardDrive, MemoryStick, Users } from "lucide-react";
import { CpuDetailContent } from "./CpuUsagePopover";
import { DiskDetailContent } from "./DiskUsagePopover";
import { GpuDetailContent } from "./GpuUsagePopover";
import { MemoryDetailContent } from "./MemoryUsagePopover";
import { UsersDetailContent } from "./UsersPopover";
import type { RemoteTelemetry, TelemetrySettings } from "../types/gpu";
import type { ResourceDetails } from "../types/resourceDetails";

type DetailResource = "cpu" | "memory" | "gpu" | "disk" | "users";

type TerminalClosedPayload = {
  sessionId: string;
  sessionClosed?: boolean;
};

const RESOURCE_META: Record<DetailResource, { title: string; icon: ReactNode }> = {
  cpu: { title: "CPU details", icon: <Cpu size={16} /> },
  memory: { title: "Memory details", icon: <MemoryStick size={16} /> },
  gpu: { title: "GPU details", icon: <Gauge size={16} /> },
  disk: { title: "Disks", icon: <HardDrive size={16} /> },
  users: { title: "Logged-in users", icon: <Users size={16} /> },
};

const DEFAULT_SETTINGS: TelemetrySettings = {
  telemetryIntervalSecs: 2,
  displayMode: "gpu-system",
  diskIgnoreFsTypes: [
    "tmpfs",
    "devtmpfs",
    "squashfs",
    "proc",
    "sysfs",
    "cgroup",
    "cgroup2",
    "overlay",
    "devfs",
    "autofs",
  ],
};

function parseQuery(): { sessionId: string | null; resource: DetailResource | null } {
  const params = new URLSearchParams(window.location.search);
  const resource = params.get("resource");
  const isResource = (value: string | null): value is DetailResource =>
    value === "cpu" || value === "memory" || value === "gpu" || value === "disk" || value === "users";
  return {
    sessionId: params.get("session"),
    resource: isResource(resource) ? resource : null,
  };
}

/**
 * Standalone page rendered in a detached "detail-*" OS window. It talks to
 * the backend directly (invoke/listen) and never touches the session store.
 */
export function DetailWindow() {
  const { sessionId, resource } = useMemo(parseQuery, []);
  const [settings, setSettings] = useState<TelemetrySettings>(DEFAULT_SETTINGS);
  const [details, setDetails] = useState<ResourceDetails | null>(null);
  const [detailsError, setDetailsError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [telemetry, setTelemetry] = useState<RemoteTelemetry | null>(null);
  const [selectedGpuUuid, setSelectedGpuUuid] = useState<string | null>(null);

  useEffect(() => {
    invoke<TelemetrySettings>("get_telemetry_settings")
      .then(setSettings)
      .catch(() => undefined);
  }, []);

  // The window belongs to one session; when that session ends, close with it.
  useEffect(() => {
    if (!sessionId) {
      return;
    }
    let disposed = false;
    let unlisten: (() => void) | null = null;
    listen<TerminalClosedPayload>("terminal-closed", (event) => {
      if (
        event.payload.sessionId === sessionId &&
        event.payload.sessionClosed !== false
      ) {
        void getCurrentWindow().close();
      }
    }).then((next) => {
      if (disposed) {
        next();
      } else {
        unlisten = next;
      }
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [sessionId]);

  // disk/users render straight from the broadcast telemetry stream.
  useEffect(() => {
    if (!sessionId || !(resource === "disk" || resource === "users")) {
      return;
    }
    let disposed = false;
    let unlisten: (() => void) | null = null;
    listen<RemoteTelemetry>("remote-telemetry", (event) => {
      if (event.payload.sessionId === sessionId) {
        setTelemetry(event.payload);
      }
    }).then((next) => {
      if (disposed) {
        next();
      } else {
        unlisten = next;
      }
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [resource, sessionId]);

  // cpu/memory/gpu poll the detail command on the telemetry interval.
  useEffect(() => {
    if (!sessionId || !(resource === "cpu" || resource === "memory" || resource === "gpu")) {
      return;
    }
    let disposed = false;
    let inFlight = false;

    const loadDetails = async () => {
      if (inFlight) {
        return;
      }
      inFlight = true;
      setLoading(true);
      try {
        const next = await invoke<ResourceDetails>("get_resource_details", {
          sessionId,
          resourceType: resource,
        });
        if (!disposed) {
          setDetails(next);
          setDetailsError(null);
        }
      } catch (error) {
        if (!disposed) {
          setDetailsError(String(error));
        }
      } finally {
        inFlight = false;
        if (!disposed) {
          setLoading(false);
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
  }, [resource, sessionId, settings.telemetryIntervalSecs]);

  if (!sessionId || !resource) {
    return (
      <div className="detail-window">
        <div className="empty-list">Invalid detail window parameters</div>
      </div>
    );
  }

  const meta = RESOURCE_META[resource];
  const needsTelemetry = resource === "disk" || resource === "users";

  return (
    <div className="detail-window">
      <header className="detail-window-header">
        {meta.icon}
        <strong>{meta.title}</strong>
        {loading && <span className="detail-refreshing">Refreshing</span>}
      </header>
      <div className="resource-detail-content detail-window-content">
        {needsTelemetry && !telemetry ? (
          <div className="empty-list">Waiting for telemetry</div>
        ) : resource === "cpu" ? (
          <CpuDetailContent
            metric={details?.cpu ?? null}
            error={details?.errors.cpu ?? detailsError}
          />
        ) : resource === "memory" ? (
          <MemoryDetailContent
            metric={details?.memory ?? null}
            error={details?.errors.memory ?? detailsError}
          />
        ) : resource === "gpu" ? (
          <GpuDetailContent
            metrics={details?.gpus ?? []}
            selectedGpuUuid={selectedGpuUuid}
            onSelectedGpuUuidChange={setSelectedGpuUuid}
            error={details?.errors.gpu ?? detailsError}
          />
        ) : resource === "disk" ? (
          <DiskDetailContent
            disks={telemetry?.disks ?? []}
            ignoredFsTypes={settings.diskIgnoreFsTypes}
          />
        ) : (
          <UsersDetailContent
            users={telemetry?.users ?? []}
            error={telemetry?.errors.users}
          />
        )}
      </div>
    </div>
  );
}
