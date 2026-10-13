import { create } from "zustand";
import type { AlertEvent, AlertSettings } from "../types/alerts";

/** How many past alerts the settings popover lists. */
const RECENT_LIMIT = 50;

type AlertStore = {
  settings: AlertSettings | null;
  /** Newest first. */
  recent: AlertEvent[];
  /** Alerts currently firing, per scope (session id or "llm"), by rule. */
  active: Record<string, Record<string, AlertEvent>>;
  setSettings: (settings: AlertSettings) => void;
  push: (event: AlertEvent) => void;
};

/** One firing alert per rule and subject. */
const ruleKey = (event: AlertEvent) => `${event.kind}:${event.subject}`;

export const useAlertStore = create<AlertStore>((set) => ({
  settings: null,
  recent: [],
  active: {},
  setSettings: (settings) => set({ settings }),
  push: (event) =>
    set((state) => {
      const scope = { ...(state.active[event.scope] ?? {}) };
      if (event.state === "fired") {
        scope[ruleKey(event)] = event;
      } else {
        delete scope[ruleKey(event)];
      }
      return {
        recent: [event, ...state.recent].slice(0, RECENT_LIMIT),
        active: { ...state.active, [event.scope]: scope },
      };
    }),
}));

/** How many alerts are firing for a scope. */
export function activeAlertCount(
  active: Record<string, Record<string, AlertEvent>>,
  scope: string | null,
) {
  return scope ? Object.keys(active[scope] ?? {}).length : 0;
}
