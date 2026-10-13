import { useState, type ReactNode } from "react";
import {
  CHART_RANGES,
  ChartRangeToggle,
  HistoryChart,
  type ChartRange,
} from "./HistoryChart";
import { formatPercent } from "../utils/format";
import type {
  GpuHistoryPoint,
  SystemHistoryPoint,
  TelemetryHistory,
} from "../types/telemetryHistory";

/** Heights for the popover sparkline and the full detail chart. */
const COMPACT_HEIGHT = 48;
const FULL_HEIGHT = 72;

const percent = (value: number) => formatPercent(value, 0);

/**
 * A titled group of history charts. The compact form (inside a popover)
 * always shows the last 15 minutes; the full form (a detached window) adds
 * the 15m / 1h toggle.
 */
function HistoryGroup({
  compact,
  children,
}: {
  compact: boolean;
  children: (range: ChartRange, height: number) => ReactNode;
}) {
  const [range, setRange] = useState<ChartRange>(CHART_RANGES[0]);
  return (
    <section className={`usage-history${compact ? " compact" : ""}`} aria-label="Usage history">
      {!compact && <ChartRangeToggle range={range} onChange={setRange} />}
      <div className="history-chart-grid">
        {children(compact ? CHART_RANGES[0] : range, compact ? COMPACT_HEIGHT : FULL_HEIGHT)}
      </div>
    </section>
  );
}

/** Lines are broken across a stretch with no buckets (a dropped connection). */
const gapFor = (history: TelemetryHistory) => history.bucketSecs * 3;

export function CpuHistory({
  history,
  compact = false,
}: {
  history: TelemetryHistory | null | undefined;
  compact?: boolean;
}) {
  if (!history) {
    return null;
  }
  return (
    <HistoryGroup compact={compact}>
      {(range, height) => (
        <HistoryChart<SystemHistoryPoint>
          label="CPU usage (%)"
          points={history.system}
          range={range}
          height={height}
          max={100}
          gapSeconds={gapFor(history)}
          formatValue={percent}
          series={[{ key: "cpu", label: "CPU", value: (point) => point.cpuPercent }]}
        />
      )}
    </HistoryGroup>
  );
}

export function MemoryHistory({
  history,
  compact = false,
}: {
  history: TelemetryHistory | null | undefined;
  compact?: boolean;
}) {
  if (!history) {
    return null;
  }
  return (
    <HistoryGroup compact={compact}>
      {(range, height) => (
        <HistoryChart<SystemHistoryPoint>
          label="Memory (%)"
          points={history.system}
          range={range}
          height={height}
          max={100}
          gapSeconds={gapFor(history)}
          formatValue={percent}
          series={[
            { key: "memory", label: "RAM", value: (point) => point.memoryPercent },
            { key: "swap", label: "Swap", tone: "amber", value: (point) => point.swapPercent },
          ]}
        />
      )}
    </HistoryGroup>
  );
}

export function DiskHistory({
  history,
  compact = false,
}: {
  history: TelemetryHistory | null | undefined;
  compact?: boolean;
}) {
  if (!history) {
    return null;
  }
  return (
    <HistoryGroup compact={compact}>
      {(range, height) => (
        <HistoryChart<SystemHistoryPoint>
          label="Fullest disk (%)"
          points={history.system}
          range={range}
          height={height}
          max={100}
          gapSeconds={gapFor(history)}
          formatValue={percent}
          series={[{ key: "disk", label: "Used", value: (point) => point.diskMaxPercent }]}
        />
      )}
    </HistoryGroup>
  );
}

export function GpuHistory({
  history,
  gpuKey,
  compact = false,
}: {
  history: TelemetryHistory | null | undefined;
  gpuKey: string;
  compact?: boolean;
}) {
  const series = history?.gpus.find((gpu) => gpu.key === gpuKey);
  if (!history || !series) {
    return null;
  }
  return (
    <HistoryGroup compact={compact}>
      {(range, height) => (
        <>
          <HistoryChart<GpuHistoryPoint>
            label="GPU and VRAM (%)"
            points={series.points}
            range={range}
            height={height}
            max={100}
            gapSeconds={gapFor(history)}
            formatValue={percent}
            series={[
              { key: "util", label: "GPU", value: (point) => point.utilPercent },
              { key: "vram", label: "VRAM", tone: "green", value: (point) => point.vramPercent },
            ]}
          />
          {!compact && (
            <>
              <HistoryChart<GpuHistoryPoint>
                label="Temperature (°C)"
                points={series.points}
                range={range}
                height={height}
                gapSeconds={gapFor(history)}
                formatValue={(value) => `${Math.round(value)}°C`}
                series={[
                  {
                    key: "temperature",
                    label: "Temp",
                    tone: "red",
                    value: (point) => point.temperatureC,
                  },
                ]}
              />
              <HistoryChart<GpuHistoryPoint>
                label="Power (W)"
                points={series.points}
                range={range}
                height={height}
                max={series.powerLimitW ?? undefined}
                gapSeconds={gapFor(history)}
                formatValue={(value) => `${Math.round(value)} W`}
                series={[
                  { key: "power", label: "Power", tone: "amber", value: (point) => point.powerW },
                ]}
              />
            </>
          )}
        </>
      )}
    </HistoryGroup>
  );
}
