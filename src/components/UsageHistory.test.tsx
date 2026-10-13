import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { CpuHistory, GpuHistory } from "./UsageHistory";
import type { TelemetryHistory } from "../types/telemetryHistory";

const NOW = 1_900_000_000;

const history: TelemetryHistory = {
  sessionId: "s",
  bucketSecs: 5,
  windowSecs: 3600,
  system: [
    { capturedAt: NOW - 5, cpuPercent: 30, memoryPercent: 50, swapPercent: 0, diskMaxPercent: 70 },
    { capturedAt: NOW, cpuPercent: 40, memoryPercent: 51, swapPercent: 0, diskMaxPercent: 70 },
  ],
  gpus: [
    {
      key: "GPU-a",
      index: 0,
      name: "A100",
      memoryTotalMiB: 81_920,
      powerLimitW: 400,
      points: [
        { capturedAt: NOW, utilPercent: 90, vramPercent: 50, vramUsedMiB: 40_960, temperatureC: 70, powerW: 300 },
      ],
    },
  ],
};

describe("usage history", () => {
  it("draws nothing before the first reading", () => {
    const { container } = render(<CpuHistory history={null} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("offers the 1h range only in the full view", () => {
    const { unmount } = render(<CpuHistory history={history} compact />);
    expect(screen.queryByRole("button", { name: "1h" })).toBeNull();
    unmount();

    render(<CpuHistory history={history} />);
    fireEvent.click(screen.getByRole("button", { name: "1h" }));
    expect(screen.getByRole("img", { name: "CPU usage (%) over the last hour" })).toBeInTheDocument();
  });

  it("shows temperature and power for a GPU only in the full view", () => {
    const { unmount } = render(<GpuHistory history={history} gpuKey="GPU-a" compact />);
    expect(screen.getAllByRole("img")).toHaveLength(1);
    unmount();

    render(<GpuHistory history={history} gpuKey="GPU-a" />);
    expect(screen.getAllByRole("img")).toHaveLength(3);
    expect(screen.getByText("Temp 70°C")).toBeInTheDocument();
  });

  it("draws nothing for a GPU with no history", () => {
    const { container } = render(<GpuHistory history={history} gpuKey="GPU-b" />);
    expect(container).toBeEmptyDOMElement();
  });
});
