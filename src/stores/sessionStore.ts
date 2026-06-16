import { create } from "zustand";
import type { AppMessage, SessionProfile } from "../types/session";
import type { RemoteTelemetry, TelemetrySettings } from "../types/gpu";

const defaultTelemetrySettings: TelemetrySettings = {
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
  ],
};

type SessionStore = {
  sessions: SessionProfile[];
  activeSessionId: string | null;
  connected: boolean;
  message: AppMessage | null;
  remoteTelemetry: RemoteTelemetry | null;
  telemetrySettings: TelemetrySettings;
  setSessions: (sessions: SessionProfile[]) => void;
  setActiveSession: (sessionId: string | null) => void;
  setConnected: (connected: boolean) => void;
  setMessage: (message: AppMessage | null) => void;
  setRemoteTelemetry: (payload: RemoteTelemetry | null) => void;
  setTelemetrySettings: (settings: TelemetrySettings) => void;
};

export const useSessionStore = create<SessionStore>((set) => ({
  sessions: [],
  activeSessionId: null,
  connected: false,
  message: null,
  remoteTelemetry: null,
  telemetrySettings: defaultTelemetrySettings,
  setSessions: (sessions) => set({ sessions }),
  setActiveSession: (activeSessionId) => set({ activeSessionId }),
  setConnected: (connected) => set({ connected }),
  setMessage: (message) => set({ message }),
  setRemoteTelemetry: (remoteTelemetry) => set({ remoteTelemetry }),
  setTelemetrySettings: (telemetrySettings) => set({ telemetrySettings }),
}));
