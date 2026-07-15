import { invoke } from "@tauri-apps/api/core";
import { useSessionStore } from "../stores/sessionStore";

/**
 * Returns an async handler that disconnects a terminal session (the active
 * one by default) and updates the session store. When the active session is
 * disconnected, the view falls back to the most recently connected session.
 */
export function useDisconnectSession() {
  const removeConnectedSession = useSessionStore(
    (state) => state.removeConnectedSession,
  );
  const setActiveSession = useSessionStore((state) => state.setActiveSession);
  const setMessage = useSessionStore((state) => state.setMessage);

  return async (sessionId?: string) => {
    const state = useSessionStore.getState();
    const id = sessionId ?? state.activeSessionId;
    if (!id || !state.connectedSessionIds.includes(id)) {
      return;
    }
    try {
      await invoke("disconnect_terminal", { sessionId: id });
      removeConnectedSession(id);
      if (state.activeSessionId === id) {
        const remaining = state.connectedSessionIds.filter(
          (connectedId) => connectedId !== id,
        );
        setActiveSession(remaining[remaining.length - 1] ?? null);
      }
      setMessage({ kind: "info", text: "Disconnected" });
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
    }
  };
}
