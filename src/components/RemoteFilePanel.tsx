import {
  ArrowUp,
  Check,
  Folder,
  FolderOpen,
  FolderPlus,
  PanelRightClose,
  RefreshCw,
  X,
} from "lucide-react";
import type { ClipboardEvent, DragEvent, PointerEvent, Ref } from "react";
import type { SftpEntry } from "../types/session";
import { formatBytes } from "../utils/formatBytes";

type RemoteFilePanelProps = {
  onClose?: () => void;
  containerRef?: Ref<HTMLElement>;
  connected: boolean;
  loading: boolean;
  path: string;
  pathDraft: string;
  entries: SftpEntry[];
  selectedPaths: string[];
  dropActive: boolean;
  creatingFolder: boolean;
  newFolderName: string;
  onPathDraftChange: (path: string) => void;
  onOpenPath: (path: string) => void;
  onRefresh: () => void;
  onGoUp: () => void;
  onSelectEntry: (entry: SftpEntry, additive: boolean) => void;
  onOpenDirectory: (path: string) => void;
  onPointerDragStart: (
    entry: SftpEntry,
    event: PointerEvent<HTMLButtonElement>,
  ) => void;
  onConsumePointerDragClick: () => boolean;
  onDropLocalFiles: (event: DragEvent<HTMLDivElement>) => void;
  onDragOverLocalFiles: (event: DragEvent<HTMLDivElement>) => void;
  onDragLeaveLocalFiles: () => void;
  onDropRemoteOnDirectory: (
    targetDirectory: SftpEntry,
    event: DragEvent<HTMLButtonElement>,
  ) => void;
  onPasteLocalFiles: (event: ClipboardEvent<HTMLElement>) => void;
  onBeginCreateFolder: () => void;
  onNewFolderNameChange: (name: string) => void;
  onCreateFolder: () => void;
  onCancelCreateFolder: () => void;
};

export function RemoteFilePanel({
  onClose,
  containerRef,
  connected,
  loading,
  path,
  pathDraft,
  entries,
  selectedPaths,
  dropActive,
  creatingFolder,
  newFolderName,
  onPathDraftChange,
  onOpenPath,
  onRefresh,
  onGoUp,
  onSelectEntry,
  onOpenDirectory,
  onPointerDragStart,
  onConsumePointerDragClick,
  onDropLocalFiles,
  onDragOverLocalFiles,
  onDragLeaveLocalFiles,
  onDropRemoteOnDirectory,
  onPasteLocalFiles,
  onBeginCreateFolder,
  onNewFolderNameChange,
  onCreateFolder,
  onCancelCreateFolder,
}: RemoteFilePanelProps) {
  return (
    <section
      ref={containerRef}
      className={`sftp-subpanel remote-drop-zone ${dropActive ? "drop-active" : ""}`}
      data-testid="remote-drop-zone"
      tabIndex={0}
      title="Drop or paste local items here. Drag remote items outside GpuTerm to export them."
      onDrop={onDropLocalFiles}
      onDragOver={onDragOverLocalFiles}
      onDragLeave={onDragLeaveLocalFiles}
      onPaste={onPasteLocalFiles}
    >
      <div className="panel-title-row">
        <div>
          <h2>SFTP</h2>
          <p>{connected ? path : "Disconnected"}</p>
        </div>
        <div className="panel-title-actions">
          <button
            className="icon-button"
            type="button"
            disabled={!connected || loading}
            aria-label="Refresh directory"
            title="Refresh"
            onClick={onRefresh}
          >
            <RefreshCw size={16} />
          </button>
          {onClose && (
            <button
              className="icon-button ghost"
              type="button"
              aria-label="Close SFTP panel"
              title="Close SFTP panel"
              onClick={onClose}
            >
              <PanelRightClose size={17} />
            </button>
          )}
        </div>
      </div>

      <div className="path-row">
        <button
          className="icon-button"
          type="button"
          disabled={!connected || loading}
          aria-label="Parent directory"
          title="Parent directory"
          onClick={onGoUp}
        >
          <ArrowUp size={16} />
        </button>
        <input
          value={pathDraft}
          disabled={!connected}
          onChange={(event) => onPathDraftChange(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              onOpenPath(pathDraft);
            }
          }}
        />
        <button
          className="icon-button"
          type="button"
          disabled={!connected || loading}
          aria-label="Open remote path"
          title="Open remote path"
          onClick={() => onOpenPath(pathDraft)}
        >
          <FolderOpen size={16} />
        </button>
        <button
          className="icon-button"
          type="button"
          disabled={!connected || loading}
          aria-label="Create remote folder"
          title="Create remote folder"
          onClick={onBeginCreateFolder}
        >
          <FolderPlus size={16} />
        </button>
      </div>

      {creatingFolder && (
        <form
          className="new-folder-row"
          onSubmit={(event) => {
            event.preventDefault();
            onCreateFolder();
          }}
        >
          <input
            autoFocus
            value={newFolderName}
            placeholder="Folder name"
            aria-label="New remote folder name"
            onChange={(event) => onNewFolderNameChange(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Escape") {
                event.preventDefault();
                onCancelCreateFolder();
              }
            }}
          />
          <button
            className="icon-button"
            type="submit"
            disabled={loading || !newFolderName.trim()}
            aria-label="Confirm new folder"
            title="Create folder"
          >
            <Check size={16} />
          </button>
          <button
            className="icon-button ghost"
            type="button"
            aria-label="Cancel new folder"
            title="Cancel"
            onClick={onCancelCreateFolder}
          >
            <X size={16} />
          </button>
        </form>
      )}

      <div className="file-table">
        <div className="file-row file-head">
          <span>Name</span>
          <span>Type</span>
          <span>Size</span>
          <span>Modified</span>
        </div>
        <div className="file-list">
          {entries.map((entry) => (
            <button
              key={entry.path}
              type="button"
              className={`file-row ${selectedPaths.includes(entry.path) ? "selected" : ""}`}
              draggable={false}
              onClick={(event) => {
                if (onConsumePointerDragClick()) {
                  event.preventDefault();
                  return;
                }
                onSelectEntry(entry, event.ctrlKey || event.metaKey);
              }}
              onDoubleClick={() => {
                if (entry.type === "directory") {
                  onOpenDirectory(entry.path);
                }
              }}
              onPointerDown={(event) => onPointerDragStart(entry, event)}
              onDrop={(event) => {
                if (entry.type === "directory") {
                  onDropRemoteOnDirectory(entry, event);
                }
              }}
              onDragOver={(event) => {
                if (entry.type === "directory") {
                  event.preventDefault();
                }
              }}
            >
              <span className="file-name">
                {entry.type === "directory" ? (
                  <Folder size={15} />
                ) : (
                  <span className="file-dot" />
                )}
                {entry.name}
              </span>
              <span>{entry.type}</span>
              <span>{formatBytes(entry.size)}</span>
              <span>{formatModified(entry.modifiedTime)}</span>
            </button>
          ))}
          {connected && entries.length === 0 && (
            <div className="empty-list">Empty directory</div>
          )}
          {!connected && <div className="empty-list">No SFTP session</div>}
        </div>
      </div>
    </section>
  );
}

function formatModified(value: number | null) {
  if (value == null) {
    return "-";
  }
  return new Date(value * 1000).toLocaleString();
}
