import { create } from "zustand";
import type { AppMessage, SessionProfile } from "../types/session";
import type { GpuMetricsPayload } from "../types/gpu";

type SessionStore = {
  sessions: SessionProfile[];
  activeSessionId: string | null;
  connected: boolean;
  message: AppMessage | null;
  gpuStatus: GpuMetricsPayload | null;
  setSessions: (sessions: SessionProfile[]) => void;
  setActiveSession: (sessionId: string | null) => void;
  setConnected: (connected: boolean) => void;
  setMessage: (message: AppMessage | null) => void;
  setGpuStatus: (payload: GpuMetricsPayload | null) => void;
};

export const useSessionStore = create<SessionStore>((set) => ({
  sessions: [],
  activeSessionId: null,
  connected: false,
  message: null,
  gpuStatus: null,
  setSessions: (sessions) => set({ sessions }),
  setActiveSession: (activeSessionId) => set({ activeSessionId }),
  setConnected: (connected) => set({ connected }),
  setMessage: (message) => set({ message }),
  setGpuStatus: (gpuStatus) => set({ gpuStatus }),
}));
