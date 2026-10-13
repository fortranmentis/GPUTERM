import { useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useSessionStore } from "../stores/sessionStore";
import type { TerminalSessionInfo } from "../types/session";
import { reconnectRequest } from "../utils/reconnectRequest";

/** Seconds before each retry; the last step repeats until MAX_ATTEMPTS. */
const BACKOFF_SECS = [2, 5, 15, 30, 60];
const MAX_ATTEMPTS = 10;

/**
 * Errors a retry cannot fix. Host-key prompts in particular must never pop up
 * from a background retry, so those wait for the user to reconnect by hand.
 */
export function isPermanentReconnectError(error: string): boolean {
  const text = error.toLowerCase();
  return (
    text.includes("unknown_host_key:") ||
    text.includes("host_key_changed:") ||
    text.includes("interactive_auth_required:") ||
    text.includes("authentication") ||
    text.includes("no longer exists") ||
    text.includes("vault is locked")
  );
}

export function reconnectDelayMs(attempt: number): number {
  return BACKOFF_SECS[Math.min(attempt, BACKOFF_SECS.length - 1)] * 1000;
}

/**
 * Reconnects sessions whose connection dropped (network change, sleep) with
 * backoff, retrying at once when the network comes back or the window is
 * focused again. Sessions the user disconnected or whose shell exited are
 * never marked lost, so they stay closed.
 */
export function useAutoReconnect() {
  const lostSessionIds = useSessionStore((state) => state.lostSessionIds);
  const attemptsRef = useRef<Map<string, number>>(new Map());
  const timersRef = useRef<Map<string, number>>(new Map());
  const inFlightRef = useRef<Set<string>>(new Set());
  const attemptRef = useRef<(sessionId: string) => void>(() => undefined);

  attemptRef.current = (sessionId: string) => {
    const timers = timersRef.current;
    const pending = timers.get(sessionId);
    if (pending != null) {
      window.clearTimeout(pending);
      timers.delete(sessionId);
    }
    if (inFlightRef.current.has(sessionId)) {
      return;
    }
    const store = useSessionStore.getState();
    if (!store.lostSessionIds.includes(sessionId)) {
      return;
    }
    const finish = () => {
      attemptsRef.current.delete(sessionId);
      store.clearSessionLost(sessionId);
    };
    if (store.connectedSessionIds.includes(sessionId)) {
      finish();
      return;
    }
    const profile = store.sessions.find((session) => session.id === sessionId);
    if (!profile) {
      finish();
      return;
    }
    const scheduleRetry = () => {
      const attempt = attemptsRef.current.get(sessionId) ?? 0;
      timers.set(
        sessionId,
        window.setTimeout(() => attemptRef.current(sessionId), reconnectDelayMs(attempt)),
      );
    };
    if (typeof navigator !== "undefined" && navigator.onLine === false) {
      // The `online` event retries as soon as the network is back.
      scheduleRetry();
      return;
    }

    inFlightRef.current.add(sessionId);
    invoke<TerminalSessionInfo>("connect_terminal", {
      request: reconnectRequest(profile),
    })
      .then((info) => {
        const state = useSessionStore.getState();
        const stillWanted = state.lostSessionIds.includes(sessionId);
        finish();
        if (!stillWanted) {
          // Disconnected or deleted while the attempt was in flight.
          void invoke("disconnect_terminal", { sessionId: info.sessionId }).catch(
            () => undefined,
          );
          return;
        }
        state.addConnectedSession(info.sessionId, info.terminalId ?? info.sessionId);
        if (state.activeSessionId == null) {
          state.showSession(info.sessionId);
        }
        state.setMessage({ kind: "success", text: `${profile.name} reconnected` });
      })
      .catch((error) => {
        const text = String(error);
        const attempt = (attemptsRef.current.get(sessionId) ?? 0) + 1;
        attemptsRef.current.set(sessionId, attempt);
        if (isPermanentReconnectError(text) || attempt >= MAX_ATTEMPTS) {
          finish();
          useSessionStore.getState().setMessage({
            kind: "info",
            text: text.toLowerCase().includes("interactive_auth_required:")
              ? `${profile.name} needs a password or one-time code to sign in. Connect again from the session list.`
              : `${profile.name}: automatic reconnect stopped. Connect again from the session list.`,
          });
          return;
        }
        scheduleRetry();
      })
      .finally(() => {
        inFlightRef.current.delete(sessionId);
      });
  };

  // Schedule newly lost sessions; forget ones that are no longer lost.
  useEffect(() => {
    const timers = timersRef.current;
    for (const sessionId of lostSessionIds) {
      if (!timers.has(sessionId) && !inFlightRef.current.has(sessionId)) {
        const attempt = attemptsRef.current.get(sessionId) ?? 0;
        timers.set(
          sessionId,
          window.setTimeout(() => attemptRef.current(sessionId), reconnectDelayMs(attempt)),
        );
      }
    }
    for (const [sessionId, timer] of timers) {
      if (!lostSessionIds.includes(sessionId)) {
        window.clearTimeout(timer);
        timers.delete(sessionId);
        attemptsRef.current.delete(sessionId);
      }
    }
  }, [lostSessionIds]);

  // Network back or the user returned: retry right away.
  useEffect(() => {
    const retryAll = () => {
      if (document.visibilityState === "hidden") {
        return;
      }
      for (const sessionId of useSessionStore.getState().lostSessionIds) {
        attemptRef.current(sessionId);
      }
    };
    window.addEventListener("online", retryAll);
    window.addEventListener("focus", retryAll);
    document.addEventListener("visibilitychange", retryAll);
    const timers = timersRef.current;
    return () => {
      window.removeEventListener("online", retryAll);
      window.removeEventListener("focus", retryAll);
      document.removeEventListener("visibilitychange", retryAll);
      for (const timer of timers.values()) {
        window.clearTimeout(timer);
      }
      timers.clear();
    };
  }, []);
}
