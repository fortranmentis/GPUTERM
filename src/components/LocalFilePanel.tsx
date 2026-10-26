import { FolderOpen } from "lucide-react";
import { useMemo, type DragEvent, type KeyboardEvent, type PointerEvent, type Ref } from "react";
import type { LocalEntry } from "../types/session";
import { formatFileSize } from "../utils/formatBytes";
import {
  isTextEditingTarget,
  selectAllState,
  type SelectionGesture,
} from "../utils/fileSelection";
import { SelectAllCheckbox } from "./SelectionCheckbox";

type LocalFilePanelProps = {
  containerRef?: Ref<HTMLElement>;
  localPath: string;
  entries: LocalEntry[];
  selectedPaths: string[];
  dropActive: boolean;
  onPathChange: (path: string) => void;
  onApplyPath: (path: string) => void;
  onBrowse: () => void;
  /** `orderedPaths` is the display order, for Shift ranges. */
  onSelectEntry: (
    entry: LocalEntry,
    gesture: SelectionGesture,
    orderedPaths: string[],
  ) => void;
  /** Header checkbox: select every entry, or clear when all are selected. */
  onToggleAll: (orderedPaths: string[]) => void;
  /** Ctrl/Cmd+A while the panel has focus. */
  onSelectAll: (orderedPaths: string[]) => void;
  onOpenDirectory: (path: string) => void;
  onPointerDragStart: (
    entry: LocalEntry,
    event: PointerEvent<HTMLButtonElement>,
  ) => void;
  onConsumePointerDragClick: () => boolean;
  onDropRemoteFiles: (event: DragEvent<HTMLDivElement>) => void;
  onDragOverRemoteFiles: (event: DragEvent<HTMLDivElement>) => void;
  onDragLeaveRemoteFiles: () => void;
};

export function LocalFilePanel({
  containerRef,
  localPath,
  entries,
  selectedPaths,
  dropActive,
  onPathChange,
  onApplyPath,
  onBrowse,
  onSelectEntry,
  onToggleAll,
  onSelectAll,
  onOpenDirectory,
  onPointerDragStart,
  onConsumePointerDragClick,
  onDropRemoteFiles,
  onDragOverRemoteFiles,
  onDragLeaveRemoteFiles,
}: LocalFilePanelProps) {
  const orderedPaths = useMemo(() => entries.map((entry) => entry.path), [entries]);
  const headerState = selectAllState(selectedPaths, orderedPaths);
  const selectedCount = orderedPaths.filter((path) => selectedPaths.includes(path)).length;

  const selectAllFromKeyboard = (event: KeyboardEvent<HTMLElement>) => {
    if (
      (event.ctrlKey || event.metaKey) &&
      !event.altKey &&
      event.key.toLowerCase() === "a" &&
      !isTextEditingTarget(event.target)
    ) {
      event.preventDefault();
      onSelectAll(orderedPaths);
    }
  };

  const applyPathFromKeyboard = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Enter" && localPath.trim()) {
      onApplyPath(localPath);
    }
  };

  return (
    <section
      ref={containerRef}
      className={`sftp-subpanel local-drop-zone ${dropActive ? "drop-active" : ""}`}
      data-testid="local-drop-zone"
      tabIndex={-1}
      onDrop={onDropRemoteFiles}
      onDragOver={onDragOverRemoteFiles}
      onDragLeave={onDragLeaveRemoteFiles}
      onKeyDown={selectAllFromKeyboard}
    >
      <label className="full-label">
        <span>Local path</span>
        <div className="local-path-row">
          <input
            value={localPath}
            placeholder="C:\\Users\\you\\Downloads"
            onChange={(event) => onPathChange(event.target.value)}
            onBlur={() => {
              if (localPath.trim()) {
                onApplyPath(localPath);
              }
            }}
            onKeyDown={applyPathFromKeyboard}
          />
          <button
            className="secondary-button compact"
            type="button"
            aria-label="Browse local folder"
            title="Browse local folder"
            onClick={onBrowse}
          >
            <FolderOpen size={16} />
            <span className="button-label">Browse</span>
          </button>
        </div>
      </label>
      {entries.length > 0 && (
        <div className="local-file-head">
          <span className="file-select-cell">
            <SelectAllCheckbox
              state={headerState}
              label="Select all local items"
              onToggle={() => onToggleAll(orderedPaths)}
            />
          </span>
          <span>{selectedCount > 0 ? `${selectedCount} selected` : "Select all"}</span>
        </div>
      )}
      <div className="local-file-list" aria-label="Local files" aria-multiselectable="true">
        {entries.map((entry) => (
          <div
            key={entry.path}
            className={`local-file-select-row ${
              selectedPaths.includes(entry.path) ? "selected" : ""
            }`}
            role="row"
            aria-selected={selectedPaths.includes(entry.path)}
          >
            <span className="file-select-cell">
              <input
                className="file-select-checkbox"
                type="checkbox"
                aria-label={`Select ${entry.name}`}
                checked={selectedPaths.includes(entry.path)}
                onChange={(event) => {
                  const native = event.nativeEvent as MouseEvent;
                  onSelectEntry(
                    entry,
                    {
                      shift: Boolean(native.shiftKey),
                      additive: Boolean(native.ctrlKey || native.metaKey),
                      checkbox: true,
                    },
                    orderedPaths,
                  );
                }}
              />
            </span>
            <button
              className={`local-file-item ${
                selectedPaths.includes(entry.path) ? "selected" : ""
              }`}
              draggable={false}
              type="button"
              onClick={(event) => {
                if (onConsumePointerDragClick()) {
                  event.preventDefault();
                  return;
                }
                onSelectEntry(
                  entry,
                  { shift: event.shiftKey, additive: event.ctrlKey || event.metaKey },
                  orderedPaths,
                );
              }}
              onDoubleClick={() => {
                if (entry.entryType === "directory") {
                  onOpenDirectory(entry.path);
                }
              }}
              onPointerDown={(event) => onPointerDragStart(entry, event)}
            >
              <span>{entry.entryType === "directory" ? "dir" : "file"}</span>
              <strong>{entry.name}</strong>
              <small>{formatFileSize(entry.size)}</small>
            </button>
          </div>
        ))}
        {entries.length === 0 && (
          <div className="empty-list compact">No local files loaded</div>
        )}
      </div>
    </section>
  );
}
