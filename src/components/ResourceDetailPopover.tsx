import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
  type RefObject,
} from "react";
import { ExternalLink, Thermometer } from "lucide-react";
import { createPortal } from "react-dom";
import type { ThermalCategoryCode, ThermalMetric } from "../types/gpu";
import { formatTemperature, thermalLevel } from "../utils/format";

const MIN_POPOVER_WIDTH = 360;
const MIN_POPOVER_HEIGHT = 240;
const VIEWPORT_MARGIN = 8;

function clamp(value: number, min: number, max: number) {
  return Math.min(Math.max(value, min), Math.max(min, max));
}

type ResourceDetailPopoverProps = {
  anchorRef: RefObject<HTMLElement | null>;
  ariaLabel: string;
  title: string;
  icon: ReactNode;
  headerActions?: ReactNode;
  children: ReactNode;
  className?: string;
  onClose: () => void;
  onPopOut?: () => void;
};

export function ResourceDetailPopover({
  anchorRef,
  ariaLabel,
  title,
  icon,
  headerActions,
  children,
  className = "",
  onClose,
  onPopOut,
}: ResourceDetailPopoverProps) {
  const [style, setStyle] = useState<CSSProperties>({});
  const popoverRef = useRef<HTMLDivElement | null>(null);
  // Once the user drags or resizes the popover, automatic anchor-based
  // placement stops so scroll/resize events do not snap it back.
  const userAdjustedRef = useRef(false);

  const startDrag = (event: ReactMouseEvent<HTMLDivElement>) => {
    if (
      (event.target as HTMLElement).closest(
        ".resource-detail-actions, button, input, label, select",
      )
    ) {
      return;
    }
    const popover = popoverRef.current;
    if (!popover) {
      return;
    }
    event.preventDefault();
    const rect = popover.getBoundingClientRect();
    const offsetX = event.clientX - rect.left;
    const offsetY = event.clientY - rect.top;

    const handleMove = (moveEvent: MouseEvent) => {
      userAdjustedRef.current = true;
      const size = popover.getBoundingClientRect();
      setStyle((current) => ({
        ...current,
        left: clamp(
          moveEvent.clientX - offsetX,
          VIEWPORT_MARGIN,
          window.innerWidth - size.width - VIEWPORT_MARGIN,
        ),
        top: clamp(
          moveEvent.clientY - offsetY,
          VIEWPORT_MARGIN,
          window.innerHeight - size.height - VIEWPORT_MARGIN,
        ),
      }));
    };
    const handleUp = () => {
      window.removeEventListener("mousemove", handleMove);
      window.removeEventListener("mouseup", handleUp);
    };
    window.addEventListener("mousemove", handleMove);
    window.addEventListener("mouseup", handleUp);
  };

  const startResize = (event: ReactMouseEvent<HTMLDivElement>) => {
    const popover = popoverRef.current;
    if (!popover) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    const rect = popover.getBoundingClientRect();
    const startX = event.clientX;
    const startY = event.clientY;

    const handleMove = (moveEvent: MouseEvent) => {
      userAdjustedRef.current = true;
      // Switch from maxHeight-based auto sizing to an explicit size so the
      // user can grow the popover past its initial cap.
      setStyle({
        left: rect.left,
        top: rect.top,
        width: clamp(
          rect.width + moveEvent.clientX - startX,
          MIN_POPOVER_WIDTH,
          window.innerWidth - rect.left - VIEWPORT_MARGIN,
        ),
        height: clamp(
          rect.height + moveEvent.clientY - startY,
          MIN_POPOVER_HEIGHT,
          window.innerHeight - rect.top - VIEWPORT_MARGIN,
        ),
      });
    };
    const handleUp = () => {
      window.removeEventListener("mousemove", handleMove);
      window.removeEventListener("mouseup", handleUp);
    };
    window.addEventListener("mousemove", handleMove);
    window.addEventListener("mouseup", handleUp);
  };

  useEffect(() => {
    const handlePointerDown = (event: MouseEvent) => {
      if (
        popoverRef.current?.contains(event.target as Node) ||
        anchorRef.current?.contains(event.target as Node)
      ) {
        return;
      }
      onClose();
    };
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        onClose();
      }
    };
    document.addEventListener("mousedown", handlePointerDown);
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("mousedown", handlePointerDown);
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [anchorRef, onClose]);

  useLayoutEffect(() => {
    const placePopover = () => {
      if (userAdjustedRef.current) {
        return;
      }
      const anchor = anchorRef.current;
      if (!anchor) {
        return;
      }
      const rect = anchor.getBoundingClientRect();
      const margin = 16;
      const maxHeight = Math.min(560, Math.max(300, window.innerHeight - margin * 2));
      const width = Math.min(980, Math.max(360, window.innerWidth - margin * 2));
      const left = Math.min(
        Math.max(margin, rect.right - width),
        window.innerWidth - width - margin,
      );
      const preferredTop = rect.top - maxHeight - 10;
      const top =
        preferredTop >= margin
          ? preferredTop
          : Math.min(rect.bottom + 10, window.innerHeight - maxHeight - margin);
      setStyle({ left, top: Math.max(margin, top), width, maxHeight });
    };

    placePopover();
    window.addEventListener("resize", placePopover);
    window.addEventListener("scroll", placePopover, true);
    return () => {
      window.removeEventListener("resize", placePopover);
      window.removeEventListener("scroll", placePopover, true);
    };
  }, [anchorRef]);

  return createPortal(
    <div
      className={`disk-detail-popover resource-detail-popover ${className}`}
      ref={popoverRef}
      role="dialog"
      aria-label={ariaLabel}
      style={style}
    >
      <div
        className="disk-detail-title resource-detail-title"
        onMouseDown={startDrag}
      >
        {icon}
        <strong>{title}</strong>
        <div className="resource-detail-actions">
          {headerActions}
          {onPopOut && (
            <button
              className="icon-button ghost"
              type="button"
              aria-label="Open in separate window"
              title="Open in separate window"
              onClick={onPopOut}
            >
              <ExternalLink size={14} />
            </button>
          )}
        </div>
      </div>
      <div className="resource-detail-content">{children}</div>
      <div
        className="resource-detail-resize-handle"
        aria-hidden="true"
        onMouseDown={startResize}
      />
    </div>,
    document.body,
  );
}

export function DetailUsageBar({
  value,
  level = "normal",
  ariaLabel,
}: {
  value: number | null;
  level?: "normal" | "warning" | "critical" | "unknown";
  ariaLabel?: string;
}) {
  const width = value == null ? 0 : Math.max(0, Math.min(100, value));
  return (
    <div
      className={`resource-usage-bar ${level}`}
      role={ariaLabel ? "progressbar" : undefined}
      aria-label={ariaLabel}
      aria-valuemin={ariaLabel ? 0 : undefined}
      aria-valuemax={ariaLabel ? 100 : undefined}
      aria-valuenow={ariaLabel && value != null ? width : undefined}
    >
      <div style={{ width: `${width}%` }} />
    </div>
  );
}

export function Metric({
  label,
  value,
  title,
  warning = false,
}: {
  label: string;
  value: string;
  title?: string;
  warning?: boolean;
}) {
  return (
    <div className={warning ? "warning" : ""} title={title}>
      <span>{label}</span>
      <strong title={value}>{value}</strong>
    </div>
  );
}

/**
 * The headline temperature as it appears on a bar card.
 *
 * Renders nothing at all when the host cannot report this category — the cards
 * are narrow, and a permanent "not supported" would cost a line on every WSL2,
 * container, and macOS host while saying nothing actionable. The reason lives in
 * the popover instead, one click away, the same way `.llm-unsupported` does it.
 */
export function ThermalChip({
  thermal,
  kind,
}: {
  thermal: ThermalMetric | null;
  kind: ThermalCategoryCode;
}) {
  if (thermal?.unsupported.some((entry) => entry.category === kind)) {
    return null;
  }
  const group = thermal ? thermal[kind] : null;
  return (
    <>
      <Thermometer size={13} /> {formatTemperature(group?.headlineC ?? null)}
    </>
  );
}

const THERMAL_HEADINGS: Record<ThermalCategoryCode, string> = {
  cpu: "CPU temperature",
  memory: "Memory temperature",
  disk: "Drive temperature",
};

/**
 * Fill fraction for a temperature gauge.
 *
 * A temperature is not a percentage, so the bar is drawn relative to the point
 * the hardware calls dangerous — the same number `thermalLevel` colours on.
 * Without that anchoring a 62 C CPU and a 62 C NVMe would draw identical bars
 * even though one is idle and the other is close to throttling.
 */
function thermalFill(value: number | null, criticalC: number | null, fallbackCritical: number) {
  if (value == null) return null;
  const ceiling = criticalC ?? fallbackCritical;
  if (!(ceiling > 0)) return null;
  return Math.max(0, Math.min(100, (value / ceiling) * 100));
}

const THERMAL_FALLBACK_CRITICAL: Record<ThermalCategoryCode, number> = {
  cpu: 100,
  memory: 90,
  disk: 80,
};

/**
 * Temperature block for one category, drawn as a sibling of the metric grid
 * rather than inside it — `.resource-metric-grid` is a fixed three-column grid
 * that both the CPU and memory popovers already fill exactly.
 *
 * The three possible states deliberately render as three different shapes:
 *   * measured        → a labelled gauge per sensor
 *   * not read yet    → the heading with an `n/a` gauge
 *   * host cannot     → no gauge at all, and the reason spelled out
 * Collapsing the last two would make every WSL2 and container host look broken.
 */
export function ThermalSection({
  thermal,
  kind,
  error,
}: {
  thermal: ThermalMetric | null;
  kind: ThermalCategoryCode;
  error?: string | null;
}) {
  const unsupported = thermal?.unsupported.find((entry) => entry.category === kind);
  if (unsupported) {
    return (
      <div className="thermal-section">
        <div className="thermal-section-title">{THERMAL_HEADINGS[kind]}</div>
        <div className="thermal-unsupported">
          <strong>Not available on this host</strong>
          <span>{unsupported.reason}</span>
        </div>
      </div>
    );
  }

  const group = thermal ? thermal[kind] : null;
  // A caveat means the number is not a die temperature (an ACPI thermal zone,
  // which is often a chassis sensor). Reporting it is useful; colouring it
  // would lend it an authority it does not have.
  const colorless = group?.caveat != null;

  return (
    <div className="thermal-section">
      <div className="thermal-section-title">
        {THERMAL_HEADINGS[kind]}
        {group?.headlineLabel && <span>{group.headlineLabel}</span>}
      </div>
      {group?.caveat && <div className="thermal-caveat">{group.caveat}</div>}
      {error && <div className="thermal-caveat">{error}</div>}
      <div className="thermal-gauge-grid">
        {group && group.sensors.length > 0 ? (
          group.sensors.map((sensor, index) => {
            const level = colorless
              ? "normal"
              : thermalLevel(sensor.temperatureC, kind, sensor.highC, sensor.criticalC);
            // A caveated reading keeps its bar *length* — that still says "near
            // the top of the scale" — but the bar goes grey rather than green,
            // because a 96%-full green bar reads as an endorsement of a number
            // we have just told the user not to trust.
            const barLevel = colorless ? "unknown" : level;
            return (
              <div key={`${sensor.source}-${sensor.label}-${index}`}>
                <span title={`${sensor.label} (${sensor.source})`}>{sensor.label}</span>
                <strong className={level}>{formatTemperature(sensor.temperatureC)}</strong>
                <DetailUsageBar
                  value={thermalFill(
                    sensor.temperatureC,
                    sensor.criticalC,
                    THERMAL_FALLBACK_CRITICAL[kind],
                  )}
                  level={barLevel}
                />
              </div>
            );
          })
        ) : (
          <div>
            <span>Temperature</span>
            <strong className="unknown">{formatTemperature(null)}</strong>
            <DetailUsageBar value={null} level="unknown" />
          </div>
        )}
      </div>
    </div>
  );
}

export function MetricsUnavailable({ error }: { error?: string | null }) {
  return (
    <div className="resource-unavailable">
      <strong>Metrics unavailable</strong>
      {error && <span>{error}</span>}
    </div>
  );
}
