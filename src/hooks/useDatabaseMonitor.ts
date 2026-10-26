import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { DbInstance, DbReading, DbStatus } from "../types/databases";
import { dbRate } from "../utils/dbFormat";

export const DB_REFRESH_MS = 10_000;

/**
 * Saved databases and their latest readings. Only databases whose SSH
 * profile is connected are read, every 10 seconds; successive readings give
 * the commit or query rate.
 */
export function useDatabaseMonitor(connectedProfileIds: string[]) {
  const [instances, setInstances] = useState<DbInstance[]>([]);
  const [readings, setReadings] = useState<Record<string, DbReading>>({});
  const [loadError, setLoadError] = useState<string | null>(null);
  const readingsRef = useRef<Record<string, DbReading>>({});
  const inFlightRef = useRef(false);

  useEffect(() => {
    let disposed = false;
    void (async () => {
      try {
        const loaded = await invoke<DbInstance[]>("list_db_instances");
        if (!disposed) setInstances(Array.isArray(loaded) ? loaded : []);
      } catch (error) {
        if (!disposed) setLoadError(String(error));
      }
    })();
    return () => {
      disposed = true;
    };
  }, []);

  const reachable = useMemo(
    () => instances.filter((instance) => connectedProfileIds.includes(instance.profileId)),
    [connectedProfileIds, instances],
  );
  const reachableKey = reachable.map((instance) => instance.id).join(",");

  const poll = useCallback(async () => {
    if (inFlightRef.current) return;
    inFlightRef.current = true;
    try {
      for (const instance of reachable) {
        const at = Date.now();
        let reading: DbReading;
        try {
          const status = await invoke<DbStatus>("db_status", { id: instance.id });
          reading = {
            status,
            error: null,
            at,
            rate: dbRate(instance.kind, readingsRef.current[instance.id], status, at),
          };
        } catch (error) {
          reading = { status: null, error: String(error), at, rate: null };
        }
        readingsRef.current = { ...readingsRef.current, [instance.id]: reading };
        setReadings(readingsRef.current);
      }
    } finally {
      inFlightRef.current = false;
    }
    // `reachableKey` stands for `reachable`, whose identity changes with every
    // unrelated session update.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [reachableKey]);

  useEffect(() => {
    if (reachableKey === "") return;
    void poll();
    const timer = window.setInterval(() => void poll(), DB_REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [poll, reachableKey]);

  /**
   * Replaces the list after a save or delete, dropping readings of removed
   * databases and of `changedId`, whose settings may no longer match them.
   */
  const replaceInstances = useCallback((next: DbInstance[], changedId?: string) => {
    const ids = new Set(next.map((instance) => instance.id));
    readingsRef.current = Object.fromEntries(
      Object.entries(readingsRef.current).filter(([id]) => ids.has(id) && id !== changedId),
    );
    setReadings(readingsRef.current);
    setInstances(next);
  }, []);

  return { instances, readings, reachable, loadError, replaceInstances, poll };
}
