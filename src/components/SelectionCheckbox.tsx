import { useEffect, useRef } from "react";
import type { SelectAllState } from "../utils/fileSelection";

/** Header checkbox for a file list: checked, clear, or indeterminate. */
export function SelectAllCheckbox({
  state,
  disabled,
  label,
  onToggle,
}: {
  state: SelectAllState;
  disabled?: boolean;
  label: string;
  onToggle: () => void;
}) {
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (ref.current) {
      ref.current.indeterminate = state === "some";
    }
  }, [state]);
  return (
    <input
      ref={ref}
      className="file-select-checkbox"
      type="checkbox"
      aria-label={label}
      title={state === "all" ? "Clear selection" : "Select all"}
      checked={state === "all"}
      aria-checked={state === "some" ? "mixed" : state === "all"}
      disabled={disabled}
      onChange={onToggle}
    />
  );
}
