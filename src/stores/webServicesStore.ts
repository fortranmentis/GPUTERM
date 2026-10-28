import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import type { WebService, WebServiceScan } from "../types/webServices";

/** Whether a newly started web service shows a toast. */
export const WEB_SERVICE_NOTIFY_KEY = "gputerm.webServiceNotify";

export const WEB_SERVICE_LABELS: Record<WebService["kind"], string> = {
  tensorboard: "TensorBoard",
  jupyter: "Jupyter",
  mlflow: "MLflow",
  ray: "Ray Dashboard",
  streamlit: "Streamlit",
  gradio: "Gradio",
  "code-server": "code-server",
  other: "Port",
};

export const webServiceKey = (service: WebService) => `${service.host}:${service.port}`;

function readNotify() {
  try {
    return localStorage.getItem(WEB_SERVICE_NOTIFY_KEY) !== "off";
  } catch {
    return true;
  }
}

type WebServicesState = {
  scans: Record<string, WebServiceScan>;
  errors: Record<string, string>;
  scanning: Record<string, boolean>;
  notify: boolean;
  /**
   * Scans a session's host. Returns the services that were not there on the
   * previous scan (none on the first one), so callers can announce them.
   */
  refresh: (sessionId: string) => Promise<WebService[]>;
  forget: (sessionId: string) => void;
  setNotify: (notify: boolean) => void;
};

export const useWebServicesStore = create<WebServicesState>((set, get) => ({
  scans: {},
  errors: {},
  scanning: {},
  notify: readNotify(),
  refresh: async (sessionId) => {
    if (get().scanning[sessionId]) {
      return [];
    }
    set((state) => ({ scanning: { ...state.scanning, [sessionId]: true } }));
    try {
      const scan = await invoke<WebServiceScan>("scan_web_services", { sessionId });
      const previous = get().scans[sessionId];
      const known = new Set(previous?.services.map(webServiceKey));
      set((state) => {
        const { [sessionId]: _cleared, ...errors } = state.errors;
        return { scans: { ...state.scans, [sessionId]: scan }, errors };
      });
      return previous ? scan.services.filter((service) => !known.has(webServiceKey(service))) : [];
    } catch (error) {
      set((state) => ({ errors: { ...state.errors, [sessionId]: String(error) } }));
      return [];
    } finally {
      set((state) => ({ scanning: { ...state.scanning, [sessionId]: false } }));
    }
  },
  forget: (sessionId) =>
    set((state) => {
      const { [sessionId]: _scan, ...scans } = state.scans;
      const { [sessionId]: _error, ...errors } = state.errors;
      return { scans, errors };
    }),
  setNotify: (notify) => {
    try {
      localStorage.setItem(WEB_SERVICE_NOTIFY_KEY, notify ? "on" : "off");
    } catch {
      // Per-device convenience only.
    }
    set({ notify });
  },
}));

/** Opens a service in the browser, forwarding a local port when it is remote. */
export function openWebService(sessionId: string, service: WebService) {
  return invoke<string>("open_web_service", {
    sessionId,
    host: service.host,
    port: service.port,
    path: service.path,
  });
}
