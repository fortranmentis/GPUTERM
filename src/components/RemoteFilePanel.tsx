import { ArrowUp, Folder, FolderOpen, RefreshCw } from "lucide-react";
import type { DragEvent } from "react";
import type { SftpEntry } from "../types/session";
import { formatBytes } from "../utils/formatBytes";

type RemoteFilePanelProps = {
  connected: boolean;
  loading: boolean;
  path: string;
  pathDraft: string;
  entries: SftpEntry[];
  selectedPaths: string[];
  dropActive: boolean;
  onPathDraftChange: (path: string) => void;
  onOpenPath: (path: string) => void;
  onRefresh: () => void;
  onGoUp: () => void;
  onSelectEntry: (entry: SftpEntry, additive: boolean) => void;
  onOpenDirectory: (path: string) => void;
  onDragStart: (entry: SftpEntry, event: DragEvent<HTMLButtonElement>) => void;
  onDragEnd: () => void;
  onDropLocalFiles: (event: DragEvent<HTMLDivElement>) => void;
  onDragOverLocalFiles: (event: DragEvent<HTMLDivElement>) => void;
  onDragLeaveLocalFiles: () => void;
  onDropRemoteOnDirectory: (
    targetDirectory: SftpEntry,
    event: DragEvent<HTMLButtonElement>,
  ) => void;
};

export function RemoteFilePanel({
  connected,
  loading,
  path,
  pathDraft,
  entries,
  selectedPaths,
  dropActive,
  onPathDraftChange,
  onOpenPath,
  onRefresh,
  onGoUp,
  onSelectEntry,
  onOpenDirectory,
  onDragStart,
  onDragEnd,
  onDropLocalFiles,
  onDragOverLocalFiles,
  onDragLeaveLocalFiles,
  onDropRemoteOnDirectory,
}: RemoteFilePanelProps) {
  return (
    <section
      className={`sftp-subpanel remote-drop-zone ${dropActive ? "drop-active" : ""}`}
      data-testid="remote-drop-zone"
      onDrop={onDropLocalFiles}
      onDragOver={onDragOverLocalFiles}
      onDragLeave={onDragLeaveLocalFiles}
    >
      <div className="panel-title-row">
        <div>
          <h2>SFTP</h2>
          <p>{connected ? path : "Disconnected"}</p>
        </div>
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
          className="secondary-button compact"
          type="button"
          disabled={!connected || loading}
          onClick={() => onOpenPath(pathDraft)}
        >
          <FolderOpen size={16} />
          Open
        </button>
      </div>

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
              draggable={entry.type === "file" || entry.type === "directory"}
              onClick={(event) => onSelectEntry(entry, event.ctrlKey || event.metaKey)}
              onDoubleClick={() => {
                if (entry.type === "directory") {
                  onOpenDirectory(entry.path);
                }
              }}
              onDragStart={(event) => onDragStart(entry, event)}
              onDragEnd={onDragEnd}
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
