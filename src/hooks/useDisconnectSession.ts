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
  const showSession = useSessionStore((state) => state.showSession);
  const setMessage = useSessionStore((state) => state.setMessage);

  return async (sessionId?: string) => {
    const id = sessionId ?? useSessionStore.getState().activeSessionId;
    if (!id || !useSessionStore.getState().connectedSessionIds.includes(id)) {
      return;
    }
    try {
      await invoke("disconnect_terminal", { sessionId: id });
      // Re-read after the round trip: the user can switch or connect sessions
      // while the IPC call is in flight, and acting on the pre-await snapshot
      // would show a session that is no longer connected.
      const state = useSessionStore.getState();
      removeConnectedSession(id);
      if (state.activeSessionId === id) {
        const remaining = state.connectedSessionIds.filter(
          (connectedId) => connectedId !== id,
        );
        showSession(remaining[remaining.length - 1] ?? null);
      }
      setMessage({ kind: "info", text: "Disconnected" });
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
    }
  };
}
