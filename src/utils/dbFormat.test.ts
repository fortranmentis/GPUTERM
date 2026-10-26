import { describe, expect, it } from "vitest";
import type { DbReading, DbStatus } from "../types/databases";
import { connectionsLabel, dbCardLine, dbRate, needsAttention } from "./dbFormat";

const EMPTY: DbStatus = {
  instanceId: "",
  version: null,
  connections: null,
  maxConnections: null,
  active: null,
  idleInTransaction: null,
  waitingOnLocks: null,
  longestQuerySecs: null,
  commits: null,
  rollbacks: null,
  queries: null,
  slowQueries: null,
  cacheHitPercent: null,
  deadlocks: null,
  sizeBytes: null,
  uptimeSecs: null,
  replicationLagSecs: null,
  isReplica: false,
};

const reading = (status: DbStatus | null, at: number): DbReading => ({
  status,
  error: null,
  at,
  rate: null,
});

describe("dbFormat", () => {
  it("formats connections and flags a nearly full server", () => {
    const status = { ...EMPTY, connections: 190, maxConnections: 200 };
    expect(connectionsLabel(status)).toBe("190/200 connections");
    expect(needsAttention(status)).toBe(true);
    expect(needsAttention({ ...status, connections: 42 })).toBe(false);
    expect(connectionsLabel({ ...EMPTY, connections: 3 })).toBe("3 connections");
    expect(needsAttention({ ...EMPTY, isReplica: true, replicationLagSecs: 75 })).toBe(true);
    expect(needsAttention(null)).toBe(false);
  });

  it("turns successive counters into rates", () => {
    const start = 1_000_000;
    const first = { ...EMPTY, commits: 1_000 };
    const second = { ...EMPTY, commits: 1_500 };
    expect(dbRate("postgres", reading(first, start), second, start + 10_000)).toBe(50);
    // No earlier reading, or a counter that went back (a restart), gives no rate.
    expect(dbRate("postgres", null, second, start)).toBeNull();
    expect(dbRate("postgres", reading(second, start), first, start + 10_000)).toBeNull();
    expect(dbRate("postgres", reading(null, start), second, start + 10_000)).toBeNull();
    expect(dbRate("postgres", reading(first, start), second, start)).toBeNull();
    // MySQL counts queries instead.
    expect(
      dbRate(
        "mysql",
        reading({ ...EMPTY, queries: 100 }, start),
        { ...EMPTY, queries: 200 },
        start + 4_000,
      ),
    ).toBe(25);
  });

  it("writes the card line", () => {
    const status = { ...EMPTY, connections: 42, maxConnections: 200 };
    expect(dbCardLine("main", "postgres", undefined)).toBe("main: reading…");
    expect(
      dbCardLine("main", "postgres", { status, error: null, at: 0, rate: 12.34 }),
    ).toBe("main: 42/200 connections, 12.3 commits/s");
    expect(
      dbCardLine("main", "mysql", { status: null, error: "refused", at: 0, rate: null }),
    ).toBe("main: refused");
  });
});
