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
    "devfs",
    "autofs",
  ],
};

type SessionStore = {
  sessions: SessionProfile[];
  activeSessionId: string | null;
  terminalViewRevision: number;
  connectedSessionIds: string[];
  terminalIdsBySession: Record<string, string[]>;
  telemetryBySession: Record<string, RemoteTelemetry>;
  message: AppMessage | null;
  telemetrySettings: TelemetrySettings;
  setSessions: (sessions: SessionProfile[]) => void;
  setActiveSession: (sessionId: string | null) => void;
  showSession: (sessionId: string | null) => void;
  addConnectedSession: (sessionId: string, terminalId?: string) => void;
  addTerminalPane: (sessionId: string, terminalId: string) => void;
  removeTerminalPane: (sessionId: string, terminalId: string) => void;
  removeConnectedSession: (sessionId: string) => void;
  setSessionTelemetry: (sessionId: string, payload: RemoteTelemetry | null) => void;
  setMessage: (message: AppMessage | null) => void;
  setTelemetrySettings: (settings: TelemetrySettings) => void;
};

export const useSessionStore = create<SessionStore>((set) => ({
  sessions: [],
  activeSessionId: null,
  terminalViewRevision: 0,
  connectedSessionIds: [],
  terminalIdsBySession: {},
  telemetryBySession: {},
  message: null,
  telemetrySettings: defaultTelemetrySettings,
  setSessions: (sessions) => set({ sessions }),
  setActiveSession: (activeSessionId) => set({ activeSessionId }),
  showSession: (activeSessionId) =>
    set((state) => ({
      activeSessionId,
      terminalViewRevision: state.terminalViewRevision + 1,
    })),
  addConnectedSession: (sessionId, terminalId = sessionId) =>
    set((state) => ({
      connectedSessionIds: state.connectedSessionIds.includes(sessionId)
        ? state.connectedSessionIds
        : [...state.connectedSessionIds, sessionId],
      terminalIdsBySession: {
        ...state.terminalIdsBySession,
        [sessionId]: [terminalId],
      },
    })),
  addTerminalPane: (sessionId, terminalId) =>
    set((state) => {
      const current = state.terminalIdsBySession[sessionId] ?? [];
      if (current.includes(terminalId)) {
        return state;
      }
      return {
        terminalIdsBySession: {
          ...state.terminalIdsBySession,
          [sessionId]: [...current, terminalId],
        },
      };
    }),
  removeTerminalPane: (sessionId, terminalId) =>
    set((state) => {
      const remainingPanes = (state.terminalIdsBySession[sessionId] ?? []).filter(
        (id) => id !== terminalId,
      );
      const terminalIdsBySession = { ...state.terminalIdsBySession };
      if (remainingPanes.length > 0) {
        terminalIdsBySession[sessionId] = remainingPanes;
        return { terminalIdsBySession };
      }
      delete terminalIdsBySession[sessionId];
      const { [sessionId]: _removed, ...telemetryBySession } =
        state.telemetryBySession;
      return {
        terminalIdsBySession,
        connectedSessionIds: state.connectedSessionIds.filter(
          (id) => id !== sessionId,
        ),
        telemetryBySession,
      };
    }),
  removeConnectedSession: (sessionId) =>
    set((state) => {
      const { [sessionId]: _removed, ...telemetryBySession } = state.telemetryBySession;
      const terminalIdsBySession = { ...state.terminalIdsBySession };
      delete terminalIdsBySession[sessionId];
      return {
        connectedSessionIds: state.connectedSessionIds.filter((id) => id !== sessionId),
        terminalIdsBySession,
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
