import { FolderOpen } from "lucide-react";
import type { DragEvent, KeyboardEvent, PointerEvent, Ref } from "react";
import type { LocalEntry } from "../types/session";
import { formatBytes } from "../utils/formatBytes";

type LocalFilePanelProps = {
  containerRef?: Ref<HTMLElement>;
  localPath: string;
  entries: LocalEntry[];
  selectedPaths: string[];
  dropActive: boolean;
  onPathChange: (path: string) => void;
  onApplyPath: (path: string) => void;
  onBrowse: () => void;
  onSelectEntry: (entry: LocalEntry, additive: boolean) => void;
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
  onOpenDirectory,
  onPointerDragStart,
  onConsumePointerDragClick,
  onDropRemoteFiles,
  onDragOverRemoteFiles,
  onDragLeaveRemoteFiles,
}: LocalFilePanelProps) {
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
      onDrop={onDropRemoteFiles}
      onDragOver={onDragOverRemoteFiles}
      onDragLeave={onDragLeaveRemoteFiles}
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
          <button className="secondary-button compact" type="button" onClick={onBrowse}>
            <FolderOpen size={16} />
            Browse
          </button>
        </div>
      </label>
      <div className="local-file-list" aria-label="Local files">
        {entries.map((entry) => (
          <button
            className={`local-file-item ${
              selectedPaths.includes(entry.path) ? "selected" : ""
            }`}
            draggable={false}
            key={entry.path}
            type="button"
            onClick={(event) => {
              if (onConsumePointerDragClick()) {
                event.preventDefault();
                return;
              }
              onSelectEntry(entry, event.ctrlKey || event.metaKey);
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
            <small>{formatBytes(entry.size)}</small>
          </button>
        ))}
        {entries.length === 0 && (
          <div className="empty-list compact">No local files loaded</div>
        )}
      </div>
    </section>
  );
}
