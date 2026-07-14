import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
  type RefObject,
} from "react";
import { createPortal } from "react-dom";

type ResourceDetailPopoverProps = {
  anchorRef: RefObject<HTMLElement | null>;
  ariaLabel: string;
  title: string;
  icon: ReactNode;
  headerActions?: ReactNode;
  children: ReactNode;
  className?: string;
  onClose: () => void;
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
}: ResourceDetailPopoverProps) {
  const [style, setStyle] = useState<CSSProperties>({});
  const popoverRef = useRef<HTMLDivElement | null>(null);

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
      <div className="disk-detail-title resource-detail-title">
        {icon}
        <strong>{title}</strong>
        {headerActions && <div className="resource-detail-actions">{headerActions}</div>}
      </div>
      <div className="resource-detail-content">{children}</div>
    </div>,
    document.body,
  );
}

export function DetailUsageBar({
  value,
  level = "normal",
}: {
  value: number | null;
  level?: "normal" | "warning" | "critical" | "unknown";
}) {
  const width = value == null ? 0 : Math.max(0, Math.min(100, value));
  return (
    <div className={`resource-usage-bar ${level}`}>
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
