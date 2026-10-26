import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState } from "react";
import type { ContainerOverview } from "../types/containers";

/** The card's summary is refreshed slowly; the open popover more often. */
export const CONTAINER_CARD_REFRESH_MS = 60_000;
export const CONTAINER_POPOVER_REFRESH_MS = 15_000;

/** The backend's answer for a host it does not read containers on. */
const UNSUPPORTED_HOST = "Linux hosts only";

/**
 * Containers and Kubernetes on one connected remote session: read on start,
 * then every minute, or every 15 seconds while `detailed` (the popover is
 * open). A host that is not Linux is asked once and then left alone.
 */
export function useContainerOverview(sessionId: string | null, enabled: boolean, detailed: boolean) {
  const [overview, setOverview] = useState<ContainerOverview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [unsupported, setUnsupported] = useState(false);
  const inFlightRef = useRef(false);
  const generationRef = useRef(0);

  useEffect(() => {
    generationRef.current += 1;
    setOverview(null);
    setError(null);
    setUnsupported(false);
  }, [sessionId]);

  const refresh = useCallback(async () => {
    if (!sessionId || inFlightRef.current) return;
    const generation = generationRef.current;
    inFlightRef.current = true;
    setLoading(true);
    try {
      const next = await invoke<ContainerOverview>("container_overview", { sessionId });
      if (generation === generationRef.current) {
        setOverview(next ?? null);
        setError(null);
      }
    } catch (requestError) {
      if (generation === generationRef.current) {
        const message = String(requestError);
        setError(message);
        if (message.includes(UNSUPPORTED_HOST)) setUnsupported(true);
      }
    } finally {
      inFlightRef.current = false;
      if (generation === generationRef.current) setLoading(false);
    }
  }, [sessionId]);

  const active = enabled && !!sessionId && !unsupported;
  const wasDetailedRef = useRef(detailed);
  useEffect(() => {
    if (!active) return;
    // Opening the popover reads now; closing it only slows the timer down.
    const closing = wasDetailedRef.current && !detailed;
    wasDetailedRef.current = detailed;
    if (!closing) void refresh();
    const timer = window.setInterval(
      () => void refresh(),
      detailed ? CONTAINER_POPOVER_REFRESH_MS : CONTAINER_CARD_REFRESH_MS,
    );
    return () => window.clearInterval(timer);
  }, [active, detailed, refresh]);

  return { overview, error, loading, unsupported, refresh };
}
