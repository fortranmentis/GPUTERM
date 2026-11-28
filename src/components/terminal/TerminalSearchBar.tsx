import type { RefObject } from "react";
import { ChevronDown, ChevronUp, X } from "lucide-react";

type TerminalSearchBarProps = {
  inputRef: RefObject<HTMLInputElement | null>;
  query: string;
  matches: { current: number; total: number } | null;
  onQueryChange: (query: string) => void;
  onSearch: (query: string, direction: "next" | "previous") => void;
  onClose: () => void;
};

export function TerminalSearchBar({
  inputRef,
  query,
  matches,
  onQueryChange,
  onSearch,
  onClose,
}: TerminalSearchBarProps) {
  return (
    <div
      className="terminal-search-bar"
      role="search"
      onMouseDown={(event) => event.stopPropagation()}
    >
      <input
        ref={inputRef}
        value={query}
        type="search"
        autoFocus
        aria-label="Search terminal output"
        placeholder="Find in terminal"
        onChange={(event) => {
          onQueryChange(event.target.value);
          onSearch(event.target.value, "next");
        }}
        onKeyDown={(event) => {
          if (event.key === "Enter") {
            event.preventDefault();
            onSearch(query, event.shiftKey ? "previous" : "next");
          } else if (event.key === "Escape") {
            event.preventDefault();
            onClose();
          }
        }}
      />
      <span className="terminal-search-status" aria-live="polite">
        {query === ""
          ? ""
          : matches == null
            ? "…"
            : matches.total === 0
              ? "No matches"
              : `${matches.current}/${matches.total}`}
      </span>
      <button
        type="button"
        aria-label="Previous match"
        title="Previous match (Shift+Enter)"
        disabled={query === ""}
        onClick={() => onSearch(query, "previous")}
      >
        <ChevronUp size={14} />
      </button>
      <button
        type="button"
        aria-label="Next match"
        title="Next match (Enter)"
        disabled={query === ""}
        onClick={() => onSearch(query, "next")}
      >
        <ChevronDown size={14} />
      </button>
      <button
        type="button"
        aria-label="Close search"
        title="Close search (Escape)"
        onClick={onClose}
      >
        <X size={14} />
      </button>
    </div>
  );
}
