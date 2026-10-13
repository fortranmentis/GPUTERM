import { formatNumber } from "../utils/format";

/** Trend windows the history charts offer. */
export const CHART_RANGES = [
  { seconds: 15 * 60, short: "15m", long: "15 minutes" },
  { seconds: 60 * 60, short: "1h", long: "hour" },
] as const;
export type ChartRange = (typeof CHART_RANGES)[number];

export type ChartTone = "cyan" | "amber" | "green" | "red";

export type HistorySeries<T> = {
  key: string;
  label: string;
  /** Line colour; cyan unless set. */
  tone?: ChartTone;
  value: (point: T) => number | null;
};

const UNKNOWN = "—";

/** A 15m / 1h toggle shared by every history chart group. */
export function ChartRangeToggle({
  range,
  onChange,
}: {
  range: ChartRange;
  onChange: (range: ChartRange) => void;
}) {
  return (
    <div className="history-range" role="group" aria-label="Chart range">
      {CHART_RANGES.map((option) => (
        <button
          key={option.short}
          type="button"
          className={option === range ? "selected" : undefined}
          aria-pressed={option === range}
          onClick={() => onChange(option)}
        >
          {option.short}
        </button>
      ))}
    </div>
  );
}

/**
 * Inline SVG line chart, following `AgyHistoryChart`: no chart library, gaps
 * left as gaps rather than interpolated through missing readings.
 */
export function HistoryChart<T extends { capturedAt: number }>({
  label,
  points,
  series,
  max,
  range = CHART_RANGES[0],
  gapSeconds,
  height = 72,
  formatValue = (value) => formatNumber(value, 1),
  pendingHint = "The trend appears after the next successful poll.",
}: {
  label: string;
  points: T[];
  series: HistorySeries<T>[];
  /** Fixed y-axis top; otherwise the axis follows the data. */
  max?: number;
  range?: ChartRange;
  /** Readings further apart than this are not joined by a line. */
  gapSeconds?: number;
  height?: number;
  formatValue?: (value: number) => string;
  pendingHint?: string;
}) {
  const width = 320;
  const left = 30;
  const right = 6;
  const top = 7;
  const bottom = 14;
  const plotWidth = width - left - right;
  const plotHeight = height - top - bottom;

  const now = Math.max(Math.floor(Date.now() / 1000), points.at(-1)?.capturedAt ?? 0);
  const start = now - range.seconds;
  const windowed = points.filter((point) => point.capturedAt >= start);

  const observed = series.flatMap((item) =>
    windowed
      .map((point) => item.value(point))
      .filter((value): value is number => value != null && Number.isFinite(value)),
  );
  // A flat zero line still needs a non-zero axis, or every point sits on the
  // baseline and the chart looks broken.
  const domainMax = max ?? Math.max(1, ...observed);

  const x = (capturedAt: number) =>
    left + Math.max(0, Math.min(1, (capturedAt - start) / range.seconds)) * plotWidth;
  const y = (value: number) =>
    top + (1 - Math.max(0, Math.min(domainMax, value)) / domainMax) * plotHeight;

  const plotted = series.map((item) => {
    const segments: Array<Array<{ at: number; value: number }>> = [];
    let current: Array<{ at: number; value: number }> = [];
    for (const point of windowed) {
      const value = item.value(point);
      const previous = current.at(-1);
      // A missing reading, or a stretch with no readings at all (the
      // connection dropped), breaks the line instead of bridging it.
      const gap =
        gapSeconds != null && previous != null && point.capturedAt - previous.at > gapSeconds;
      if (value == null || !Number.isFinite(value) || gap) {
        if (current.length > 0) {
          segments.push(current);
          current = [];
        }
        if (value == null || !Number.isFinite(value)) {
          continue;
        }
      }
      current.push({ at: point.capturedAt, value });
    }
    if (current.length > 0) {
      segments.push(current);
    }
    const latest = segments.at(-1)?.at(-1)?.value ?? null;
    return { ...item, segments, latest };
  });

  const pointCount = plotted.reduce((total, item) => total + item.segments.flat().length, 0);
  const title = `${label} over the last ${range.long}`;

  return (
    <div className="history-chart">
      <div className="history-heading">
        <strong>{label}</strong>
        <div className="history-legend">
          {plotted.map((item) => (
            <span className={`tone-${item.tone ?? "cyan"}`} key={item.key}>
              {item.label} {item.latest == null ? UNKNOWN : formatValue(item.latest)}
            </span>
          ))}
        </div>
      </div>
      <svg
        viewBox={`0 0 ${width} ${height}`}
        style={{ minHeight: height }}
        role="img"
        aria-label={title}
      >
        <title>{title}</title>
        {[0, domainMax / 2, domainMax].map((value) => (
          <g key={value}>
            <line
              className="history-grid-line"
              x1={left}
              x2={width - right}
              y1={y(value)}
              y2={y(value)}
            />
            <text x={left - 4} y={y(value) + 3} textAnchor="end">
              {formatNumber(value, value >= 10 ? 0 : 1)}
            </text>
          </g>
        ))}
        <text x={left} y={height - 2}>
          −{range.short}
        </text>
        <text x={width - right} y={height - 2} textAnchor="end">
          now
        </text>
        {plotted.flatMap((item) =>
          item.segments.map((segment, index) => {
            const path = segment
              .map(
                (point, pointIndex) =>
                  `${pointIndex === 0 ? "M" : "L"} ${x(point.at).toFixed(2)} ${y(
                    point.value,
                  ).toFixed(2)}`,
              )
              .join(" ");
            const last = segment.at(-1);
            return (
              <g
                className={`history-series tone-${item.tone ?? "cyan"}`}
                key={`${item.key}:${index}`}
              >
                <path d={path} />
                {last && <circle cx={x(last.at)} cy={y(last.value)} r={1.8} />}
              </g>
            );
          }),
        )}
      </svg>
      {pointCount < 2 && <small className="history-pending">{pendingHint}</small>}
    </div>
  );
}
