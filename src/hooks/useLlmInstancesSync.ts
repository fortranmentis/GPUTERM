import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import type { LlmInstance } from "../types/llm";

/** Mirrors `INSTANCES_CHANGED_EVENT` in src-tauri/src/llm/mod.rs. */
export const LLM_INSTANCES_CHANGED_EVENT = "llm-instances-changed";

/**
 * Applies every saved change to the LLM instance list, whichever window made
 * it. The window that saved also gets the list back from its own command;
 * applying the same list twice is harmless.
 */
export function useLlmInstancesSync(
  setInstances: (instances: LlmInstance[]) => void,
  enabled = true,
) {
  useEffect(() => {
    if (!enabled) {
      return;
    }
    let disposed = false;
    let unlisten: (() => void) | null = null;
    listen<LlmInstance[]>(LLM_INSTANCES_CHANGED_EVENT, (event) => {
      setInstances(event.payload);
    }).then((next) => {
      if (disposed) {
        next();
      } else {
        unlisten = next;
      }
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [enabled, setInstances]);
}
