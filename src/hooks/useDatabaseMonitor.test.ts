import { act, renderHook, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useDatabaseMonitor } from "./useDatabaseMonitor";
import type { DbInstance, DbStatus } from "../types/databases";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const mockInvoke = vi.mocked(invoke);

const pgInstance: DbInstance = {
  id: "db-pg",
  name: "Primary PG",
  kind: "postgres",
  profileId: "gpu-1",
  host: "127.0.0.1",
  port: 5432,
  user: "postgres",
  database: "app",
  hasPassword: false,
};

const mysqlInstance: DbInstance = {
  id: "db-mysql",
  name: "Analytics MySQL",
  kind: "mysql",
  profileId: "gpu-2",
  host: "127.0.0.1",
  port: 3306,
  user: "root",
  database: "",
  hasPassword: true,
};

function status(instanceId: string, commits: number): DbStatus {
  return {
    instanceId,
    version: "16.2",
    connections: 12,
    maxConnections: 100,
    active: 2,
    idleInTransaction: 0,
    waitingOnLocks: 0,
    longestQuerySecs: 1,
    commits,
    rollbacks: 0,
    queries: null,
    slowQueries: null,
    cacheHitPercent: 99.5,
    deadlocks: 0,
    sizeBytes: 1024 * 1024,
    uptimeSecs: 3600,
    replicationLagSecs: null,
    isReplica: false,
  };
}

describe("useDatabaseMonitor", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
  });

  it("loads saved instances, polls only reachable ones, and computes rates", async () => {
    const nowSpy = vi.spyOn(Date, "now").mockReturnValue(10_000);
    let pollCount = 0;
    mockInvoke.mockImplementation(async (command, args) => {
      if (command === "list_db_instances") return [pgInstance, mysqlInstance];
      if (command === "db_status") {
        const { id } = args as { id: string };
        pollCount += 1;
        return status(id, pollCount === 1 ? 100 : 150);
      }
      return null;
    });

    try {
      const { result } = renderHook(() => useDatabaseMonitor(["gpu-1"]));
      await waitFor(() => expect(result.current.instances).toHaveLength(2));
      expect(result.current.reachable.map((item) => item.id)).toEqual(["db-pg"]);

      await waitFor(() => expect(result.current.readings["db-pg"]?.status?.commits).toBe(100));
      expect(result.current.readings["db-pg"]?.rate).toBeNull();
      expect(result.current.readings["db-mysql"]).toBeUndefined();

      nowSpy.mockReturnValue(20_000);
      await act(async () => {
        await result.current.poll();
      });
      expect(result.current.readings["db-pg"]?.status?.commits).toBe(150);
      expect(result.current.readings["db-pg"]?.rate).toBe(5);
    } finally {
      nowSpy.mockRestore();
    }
  });

  it("records load errors and per-instance poll errors", async () => {
    mockInvoke.mockRejectedValueOnce("disk unreadable");
    const failed = renderHook(() => useDatabaseMonitor(["gpu-1"]));
    await waitFor(() => expect(failed.result.current.loadError).toBe("disk unreadable"));
    failed.unmount();

    mockInvoke.mockImplementation(async (command) => {
      if (command === "list_db_instances") return [pgInstance];
      if (command === "db_status") throw "connection refused";
      return null;
    });
    const { result } = renderHook(() => useDatabaseMonitor(["gpu-1"]));
    await waitFor(() =>
      expect(result.current.readings["db-pg"]?.error).toBe("connection refused"),
    );
    expect(result.current.readings["db-pg"]?.status).toBeNull();
  });

  it("drops readings for removed or edited instances in replaceInstances", async () => {
    mockInvoke.mockImplementation(async (command, args) => {
      if (command === "list_db_instances") return [pgInstance, mysqlInstance];
      if (command === "db_status") return status((args as { id: string }).id, 10);
      return null;
    });

    const { result } = renderHook(() => useDatabaseMonitor(["gpu-1", "gpu-2"]));
    await waitFor(() => {
      expect(result.current.readings["db-pg"]).toBeDefined();
      expect(result.current.readings["db-mysql"]).toBeDefined();
    });

    act(() => {
      result.current.replaceInstances([pgInstance], "db-pg");
    });
    expect(result.current.instances).toEqual([pgInstance]);
    expect(result.current.readings["db-pg"]).toBeUndefined();
    expect(result.current.readings["db-mysql"]).toBeUndefined();
  });

  it("stops polling and skips state updates if unmounted while db_status is in flight", async () => {
    let resolveFirst!: (value: DbStatus) => void;
    const secondInstance: DbInstance = { ...mysqlInstance, profileId: "gpu-1" };
    mockInvoke.mockImplementation((command, args) => {
      if (command === "list_db_instances") {
        return Promise.resolve([pgInstance, secondInstance]);
      }
      if (command === "db_status" && (args as { id: string }).id === "db-pg") {
        return new Promise<DbStatus>((resolve) => {
          resolveFirst = resolve;
        });
      }
      return Promise.resolve(status("db-mysql", 1));
    });

    const { unmount } = renderHook(() => useDatabaseMonitor(["gpu-1"]));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("db_status", { id: "db-pg" }),
    );

    unmount();
    await act(async () => {
      resolveFirst(status("db-pg", 42));
      await Promise.resolve();
    });

    expect(mockInvoke).not.toHaveBeenCalledWith("db_status", { id: "db-mysql" });
  });
});
