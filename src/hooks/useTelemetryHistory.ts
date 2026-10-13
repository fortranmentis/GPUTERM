import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type {
  GpuHistorySeries,
  TelemetryHistory,
} from "../types/telemetryHistory";

/** How often the open chart asks for new buckets. */
const POLL_MS = 5_000;

function mergePoints<T extends { capturedAt: number }>(
  previous: T[],
  next: T[],
  cutoff: number,
): T[] {
  if (next.length === 0) {
    return previous.filter((point) => point.capturedAt >= cutoff);
  }
  // The delta starts with the bucket the last poll ended on, which may have
  // gained readings since; it replaces that bucket rather than repeating it.
  const firstNew = next[0].capturedAt;
  return [
    ...previous.filter((point) => point.capturedAt < firstNew && point.capturedAt >= cutoff),
    ...next,
  ];
}

/**
 * Folds a delta (everything since the last bucket already held) into the
 * history held so far, dropping what has aged out of the window.
 */
export function mergeHistory(
  previous: TelemetryHistory | null,
  delta: TelemetryHistory,
): TelemetryHistory {
  if (!previous || previous.sessionId !== delta.sessionId) {
    return delta;
  }
  const newest = Math.max(
    delta.system.at(-1)?.capturedAt ?? 0,
    ...delta.gpus.map((series) => series.points.at(-1)?.capturedAt ?? 0),
  );
  const cutoff = newest - delta.windowSecs;
  const previousGpus = new Map(previous.gpus.map((series) => [series.key, series]));
  const gpus: GpuHistorySeries[] = delta.gpus.map((series) => ({
    ...series,
    points: mergePoints(previousGpus.get(series.key)?.points ?? [], series.points, cutoff),
  }));
  return {
    ...delta,
    system: mergePoints(previous.system, delta.system, cutoff),
    gpus,
  };
}

function lastCapturedAt(history: TelemetryHistory | null): number | undefined {
  if (!history) {
    return undefined;
  }
  const times = [
    history.system.at(-1)?.capturedAt,
    ...history.gpus.map((series) => series.points.at(-1)?.capturedAt),
  ].filter((time): time is number => time != null);
  return times.length > 0 ? Math.min(...times) : undefined;
}

/**
 * The session's usage history while `enabled`, kept current by fetching only
 * what is new. Works in any window: it talks to the backend directly.
 */
export function useTelemetryHistory(
  sessionId: string | null,
  enabled: boolean,
): TelemetryHistory | null {
  const [history, setHistory] = useState<TelemetryHistory | null>(null);

  useEffect(() => {
    if (!sessionId || !enabled) {
      setHistory(null);
      return;
    }
    let disposed = false;
    let held: TelemetryHistory | null = null;
    const poll = async () => {
      try {
        const delta = await invoke<TelemetryHistory | null>("get_telemetry_history", {
          sessionId,
          since: lastCapturedAt(held),
        });
        if (disposed || !delta) {
          return;
        }
        held = mergeHistory(held, delta);
        setHistory(held);
      } catch {
        // The chart keeps what it has; the next poll retries.
      }
    };
    void poll();
    const timer = window.setInterval(() => void poll(), POLL_MS);
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, [enabled, sessionId]);

  return history;
}
