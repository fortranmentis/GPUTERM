import { create } from "zustand";
import type { LlmInstance, LlmTelemetry } from "../types/llm";

/**
 * LLM runtime state lives in its own store rather than in `sessionStore`,
 * because none of it is keyed by SSH session: a runtime is a standalone HTTP
 * endpoint that is monitored whether or not any terminal is connected.
 */
type LlmStore = {
  instances: LlmInstance[];
  telemetry: LlmTelemetry | null;
  setInstances: (instances: LlmInstance[]) => void;
  setTelemetry: (telemetry: LlmTelemetry | null) => void;
};

export const useLlmStore = create<LlmStore>((set) => ({
  instances: [],
  telemetry: null,
  setInstances: (instances) => set({ instances }),
  setTelemetry: (telemetry) => set({ telemetry }),
}));

/**
 * The registered list joined with whatever the poller last reported.
 *
 * The instance list is authoritative for what exists: a freshly added instance
 * must appear immediately, before its first poll produces telemetry.
 */
export function selectInstanceRows(state: LlmStore) {
  const byId = new Map(
    (state.telemetry?.instances ?? []).map((entry) => [entry.instance.id, entry]),
  );
  return state.instances.map((instance) => ({
    instance,
    telemetry: byId.get(instance.id) ?? null,
  }));
}
