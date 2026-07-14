import { describe, expect, it } from "vitest";
import {
  cpuLevel,
  formatCoreCount,
  formatCoreRatio,
  formatGhz,
  formatGiBFromMiB,
  formatMiB,
  formatNumber,
  formatPercent,
  formatUptime,
  formatWatts,
  memoryLevel,
  powerLevel,
  ratio,
  temperatureLevel,
  vramLevel,
} from "./format";

describe("format helpers", () => {
  it("formats percents with configurable precision", () => {
    expect(formatPercent(42.6)).toBe("43%");
    expect(formatPercent(42.64, 1)).toBe("42.6%");
    expect(formatPercent(null)).toBe("n/a");
  });

  it("formats numbers, GHz, and watts", () => {
    expect(formatNumber(1.234)).toBe("1.23");
    expect(formatGhz(3.456)).toBe("3.5 GHz");
    expect(formatGhz(null)).toBe("n/a GHz");
    expect(formatGhz(null, 2, "n/a")).toBe("n/a");
    expect(formatWatts(250.4)).toBe("250 W");
  });

  it("formats MiB values via byte units", () => {
    expect(formatMiB(null)).toBe("n/a");
    expect(formatMiB(1024)).toContain("GiB");
  });

  it("formats GiB from MiB with precision by magnitude", () => {
    expect(formatGiBFromMiB(512)).toBe("0.50 GiB");
    expect(formatGiBFromMiB(20480)).toBe("20.0 GiB");
    expect(formatGiBFromMiB(null)).toBe("n/a");
  });

  it("formats uptime as days/hours/minutes", () => {
    expect(formatUptime(90061)).toBe("1d 1h 1m");
    expect(formatUptime(null)).toBe("n/a");
  });

  it("formats core counts in bar and ratio styles", () => {
    expect(formatCoreCount(8, 16)).toBe("8/16 cores");
    expect(formatCoreCount(16, 16)).toBe("16 cores");
    expect(formatCoreCount(null, null)).toBe("cores n/a");
    expect(formatCoreRatio(8, 16)).toBe("8 / 16");
    expect(formatCoreRatio(null, null)).toBe("n/a");
  });

  it("computes usage ratio percentages", () => {
    expect(ratio(50, 200)).toBe(25);
    expect(ratio(1, 0)).toBeNull();
    expect(ratio(null, 100)).toBeNull();
  });

  it("maps usage values to levels with per-resource thresholds", () => {
    expect(cpuLevel(96)).toBe("critical");
    expect(cpuLevel(85)).toBe("warning");
    expect(cpuLevel(10)).toBe("normal");
    expect(cpuLevel(null)).toBe("unknown");

    expect(memoryLevel(90)).toBe("warning");
    expect(temperatureLevel(91)).toBe("critical");
    expect(vramLevel(99, 100)).toBe("critical");
    expect(vramLevel(50, 100)).toBe("normal");
    expect(powerLevel(95, 100)).toBe("warning");
    expect(powerLevel(null, 100)).toBe("unknown");
  });
});
