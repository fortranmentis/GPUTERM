import { invoke } from "@tauri-apps/api/core";
import { useSessionStore } from "../stores/sessionStore";

/**
 * Returns an async handler that disconnects the active terminal session and
 * resets the related session-store state. No-ops when nothing is connected.
 */
export function useDisconnectSession() {
  const activeSessionId = useSessionStore((state) => state.activeSessionId);
  const setConnected = useSessionStore((state) => state.setConnected);
  const setActiveSession = useSessionStore((state) => state.setActiveSession);
  const setRemoteTelemetry = useSessionStore((state) => state.setRemoteTelemetry);
  const setMessage = useSessionStore((state) => state.setMessage);

  return async () => {
    if (!activeSessionId) {
      return;
    }
    try {
      await invoke("disconnect_terminal", { sessionId: activeSessionId });
      setConnected(false);
      setActiveSession(null);
      setRemoteTelemetry(null);
      setMessage({ kind: "info", text: "Disconnected" });
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
    }
  };
}
