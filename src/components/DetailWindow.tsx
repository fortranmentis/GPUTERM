import { useEffect, useMemo, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Bot, Boxes, Cpu, Gauge, HardDrive, MemoryStick, Users } from "lucide-react";
import { AgentDetailContent } from "./AgentUsagePopover";
import { LlmRuntimeDetailContent } from "./LlmRuntimePopover";
import { CpuDetailContent } from "./CpuUsagePopover";
import { DiskDetailContent } from "./DiskUsagePopover";
import { GpuDetailContent } from "./GpuUsagePopover";
import { MemoryDetailContent } from "./MemoryUsagePopover";
import { UsersDetailContent } from "./UsersPopover";
import type { RemoteTelemetry, TelemetrySettings } from "../types/gpu";
import type { LlmInstance, LlmTelemetry } from "../types/llm";
import type { ResourceDetails } from "../types/resourceDetails";
import type { TerminalClosedPayload } from "../types/session";

type DetailResource =
  | "cpu"
  | "memory"
  | "gpu"
  | "disk"
  | "users"
  | "agents"
  | "llm";

const RESOURCE_META: Record<DetailResource, { title: string; icon: ReactNode }> = {
  cpu: { title: "CPU details", icon: <Cpu size={16} /> },
  memory: { title: "Memory details", icon: <MemoryStick size={16} /> },
  gpu: { title: "GPU details", icon: <Gauge size={16} /> },
  disk: { title: "Disks", icon: <HardDrive size={16} /> },
  users: { title: "Logged-in users", icon: <Users size={16} /> },
  agents: { title: "AI DASH", icon: <Bot size={16} /> },
  llm: { title: "LLM runtimes", icon: <Boxes size={16} /> },
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
    value === "cpu" ||
    value === "memory" ||
    value === "gpu" ||
    value === "disk" ||
    value === "users" ||
    value === "agents" ||
    value === "llm";
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
  const [llmInstances, setLlmInstances] = useState<LlmInstance[]>([]);
  const [llmTelemetry, setLlmTelemetry] = useState<LlmTelemetry | null>(null);

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

  // The LLM window is not tied to a session: it reads the poller's own stream.
  useEffect(() => {
    if (resource !== "llm") {
      return;
    }
    let disposed = false;
    let unlisten: (() => void) | null = null;

    invoke<LlmInstance[]>("list_llm_instances")
      .then((next) => !disposed && setLlmInstances(next))
      .catch(() => undefined);
    invoke<LlmTelemetry | null>("get_llm_telemetry")
      .then((next) => !disposed && next && setLlmTelemetry(next))
      .catch(() => undefined);

    listen<LlmTelemetry>("llm-runtime-telemetry", (event) => {
      setLlmTelemetry(event.payload);
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
  }, [resource]);

  // disk/users/agents render straight from the broadcast telemetry stream, and
  // cpu/memory need it too now: temperature rides on the telemetry stream, not
  // on the per-resource detail command. Deliberately not added to
  // `needsTelemetry` below — that gate draws "Waiting for telemetry", and an
  // optional temperature must never block the CPU or memory detail itself.
  useEffect(() => {
    if (
      !sessionId ||
      !(
        resource === "disk" ||
        resource === "users" ||
        resource === "agents" ||
        resource === "cpu" ||
        resource === "memory"
      )
    ) {
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

  // Every resource but `llm` renders one SSH session's telemetry.
  if (!resource || (!sessionId && resource !== "llm")) {
    return (
      <div className="detail-window">
        <div className="empty-list">Invalid detail window parameters</div>
      </div>
    );
  }

  const meta = RESOURCE_META[resource];
  const needsTelemetry =
    resource === "disk" || resource === "users" || resource === "agents";

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
            thermal={telemetry?.thermal ?? null}
            thermalError={telemetry?.errors.thermal}
            error={details?.errors.cpu ?? detailsError}
          />
        ) : resource === "memory" ? (
          <MemoryDetailContent
            metric={details?.memory ?? null}
            thermal={telemetry?.thermal ?? null}
            thermalError={telemetry?.errors.thermal}
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
            thermal={telemetry?.thermal ?? null}
            thermalError={telemetry?.errors.thermal}
            ignoredFsTypes={settings.diskIgnoreFsTypes}
          />
        ) : resource === "users" ? (
          <UsersDetailContent
            users={telemetry?.users ?? []}
            error={telemetry?.errors.users}
          />
        ) : resource === "llm" ? (
          <LlmRuntimeDetailContent
            telemetry={llmTelemetry}
            instances={llmInstances}
            onInstancesChange={setLlmInstances}
          />
        ) : (
          <AgentDetailContent
            sessionId={sessionId ?? ""}
            agents={telemetry?.agents ?? []}
            error={telemetry?.errors.agents}
          />
        )}
      </div>
    </div>
  );
}
