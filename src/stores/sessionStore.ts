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
  connectedSessionIds: string[];
  telemetryBySession: Record<string, RemoteTelemetry>;
  message: AppMessage | null;
  telemetrySettings: TelemetrySettings;
  setSessions: (sessions: SessionProfile[]) => void;
  setActiveSession: (sessionId: string | null) => void;
  addConnectedSession: (sessionId: string) => void;
  removeConnectedSession: (sessionId: string) => void;
  setSessionTelemetry: (sessionId: string, payload: RemoteTelemetry | null) => void;
  setMessage: (message: AppMessage | null) => void;
  setTelemetrySettings: (settings: TelemetrySettings) => void;
};

export const useSessionStore = create<SessionStore>((set) => ({
  sessions: [],
  activeSessionId: null,
  connectedSessionIds: [],
  telemetryBySession: {},
  message: null,
  telemetrySettings: defaultTelemetrySettings,
  setSessions: (sessions) => set({ sessions }),
  setActiveSession: (activeSessionId) => set({ activeSessionId }),
  addConnectedSession: (sessionId) =>
    set((state) =>
      state.connectedSessionIds.includes(sessionId)
        ? state
        : { connectedSessionIds: [...state.connectedSessionIds, sessionId] },
    ),
  removeConnectedSession: (sessionId) =>
    set((state) => {
      const { [sessionId]: _removed, ...telemetryBySession } = state.telemetryBySession;
      return {
        connectedSessionIds: state.connectedSessionIds.filter((id) => id !== sessionId),
        telemetryBySession,
      };
    }),
  setSessionTelemetry: (sessionId, payload) =>
    set((state) => {
      if (payload == null) {
        const { [sessionId]: _removed, ...telemetryBySession } = state.telemetryBySession;
        return { telemetryBySession };
      }
      return {
        telemetryBySession: { ...state.telemetryBySession, [sessionId]: payload },
      };
    }),
  setMessage: (message) => set({ message }),
  setTelemetrySettings: (telemetrySettings) => set({ telemetrySettings }),
}));

export const selectIsActiveConnected = (state: {
  activeSessionId: string | null;
  connectedSessionIds: string[];
}) =>
  state.activeSessionId != null &&
  state.connectedSessionIds.includes(state.activeSessionId);

export const selectActiveTelemetry = (state: {
  activeSessionId: string | null;
  telemetryBySession: Record<string, RemoteTelemetry>;
}) =>
  state.activeSessionId != null
    ? state.telemetryBySession[state.activeSessionId] ?? null
    : null;
