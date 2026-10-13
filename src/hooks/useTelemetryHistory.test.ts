import { describe, expect, it } from "vitest";
import { mergeHistory } from "./useTelemetryHistory";
import type { SystemHistoryPoint, TelemetryHistory } from "../types/telemetryHistory";

function point(capturedAt: number, cpuPercent: number): SystemHistoryPoint {
  return {
    capturedAt,
    cpuPercent,
    memoryPercent: null,
    swapPercent: null,
    diskMaxPercent: null,
  };
}

function history(system: SystemHistoryPoint[], sessionId = "s"): TelemetryHistory {
  return { sessionId, bucketSecs: 5, windowSecs: 60, system, gpus: [] };
}

describe("mergeHistory", () => {
  it("replaces the bucket the last poll ended on and appends the rest", () => {
    const held = history([point(100, 1), point(105, 2)]);
    const delta = history([point(105, 3), point(110, 4)]);
    expect(mergeHistory(held, delta).system).toEqual([
      point(100, 1),
      point(105, 3),
      point(110, 4),
    ]);
  });

  it("drops what has aged out of the window", () => {
    const held = history([point(0, 1), point(50, 2)]);
    const delta = history([point(100, 3)]);
    expect(mergeHistory(held, delta).system.map((p) => p.capturedAt)).toEqual([50, 100]);
  });

  it("starts over for a different session", () => {
    const delta = history([point(10, 9)], "other");
    expect(mergeHistory(history([point(5, 1)]), delta)).toBe(delta);
  });

  it("keeps a GPU's points when the delta has none for it yet", () => {
    const gpu = {
      key: "GPU-a",
      index: 0,
      name: "A100",
      memoryTotalMiB: 80_000,
      powerLimitW: 400,
    };
    const gpuPoint = {
      capturedAt: 100,
      utilPercent: 50,
      vramPercent: 10,
      vramUsedMiB: 8_000,
      temperatureC: 60,
      powerW: 200,
    };
    const held = { ...history([point(100, 1)]), gpus: [{ ...gpu, points: [gpuPoint] }] };
    const delta = { ...history([point(105, 2)]), gpus: [{ ...gpu, points: [] }] };
    expect(mergeHistory(held, delta).gpus[0].points).toEqual([gpuPoint]);
  });
});
