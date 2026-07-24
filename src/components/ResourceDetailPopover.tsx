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
import { ExternalLink } from "lucide-react";
import { createPortal } from "react-dom";

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

export function MetricsUnavailable({ error }: { error?: string | null }) {
  return (
    <div className="resource-unavailable">
      <strong>Metrics unavailable</strong>
      {error && <span>{error}</span>}
    </div>
  );
}
