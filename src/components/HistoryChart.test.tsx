import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { CHART_RANGES, HistoryChart } from "./HistoryChart";

const NOW = 1_900_000_000;

type Point = { capturedAt: number; value: number | null };

function paths(container: HTMLElement) {
  return container.querySelectorAll("svg g.history-series path").length;
}

function chart(points: Point[], gapSeconds?: number) {
  return render(
    <HistoryChart<Point>
      label="CPU usage (%)"
      points={points}
      gapSeconds={gapSeconds}
      series={[{ key: "cpu", label: "CPU", value: (point) => point.value }]}
    />,
  );
}

describe("HistoryChart", () => {
  it("breaks the line at a missing reading", () => {
    const { container } = chart([
      { capturedAt: NOW - 20, value: 10 },
      { capturedAt: NOW - 15, value: null },
      { capturedAt: NOW - 10, value: 20 },
      { capturedAt: NOW - 5, value: 30 },
    ]);
    expect(paths(container)).toBe(2);
  });

  it("breaks the line across a stretch with no readings", () => {
    const points = [
      { capturedAt: NOW - 300, value: 10 },
      { capturedAt: NOW - 295, value: 12 },
      { capturedAt: NOW - 10, value: 20 },
      { capturedAt: NOW - 5, value: 30 },
    ];
    expect(paths(chart(points).container)).toBe(1);
    expect(paths(chart(points, 15).container)).toBe(2);
  });

  it("labels the axis with the chosen range and shows the latest value", () => {
    render(
      <HistoryChart<Point>
        label="Memory (%)"
        points={[
          { capturedAt: NOW - 5, value: 40 },
          { capturedAt: NOW, value: 42.5 },
        ]}
        range={CHART_RANGES[1]}
        max={100}
        formatValue={(value) => `${value}%`}
        series={[{ key: "memory", label: "RAM", value: (point) => point.value }]}
      />,
    );
    expect(screen.getByRole("img", { name: "Memory (%) over the last hour" })).toBeInTheDocument();
    expect(screen.getByText("−1h")).toBeInTheDocument();
    expect(screen.getByText("RAM 42.5%")).toBeInTheDocument();
  });
});
