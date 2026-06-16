import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  ArrowUp,
  Download,
  Folder,
  FolderOpen,
  FolderPlus,
  RefreshCw,
  Trash2,
  Upload,
} from "lucide-react";
import { useSessionStore } from "../stores/sessionStore";
import type {
  SftpEntry,
  SftpListResponse,
  SftpProgressPayload,
} from "../types/session";

export function SftpBrowser() {
  const activeSessionId = useSessionStore((state) => state.activeSessionId);
  const connected = useSessionStore((state) => state.connected);
  const setMessage = useSessionStore((state) => state.setMessage);
  const [path, setPath] = useState(".");
  const [pathDraft, setPathDraft] = useState(".");
  const [entries, setEntries] = useState<SftpEntry[]>([]);
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const [localPath, setLocalPath] = useState("");
  const [newFolderName, setNewFolderName] = useState("");
  const [loading, setLoading] = useState(false);
  const [progress, setProgress] = useState<SftpProgressPayload | null>(null);

  const selectedEntry = useMemo(
    () => entries.find((entry) => entry.path === selectedPath) ?? null,
    [entries, selectedPath],
  );

  useEffect(() => {
    if (connected && activeSessionId) {
      loadDirectory(path).catch((error) =>
        setMessage({ kind: "error", text: String(error) }),
      );
    } else {
      setEntries([]);
      setSelectedPath(null);
      setProgress(null);
    }
  }, [activeSessionId, connected]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;

    listen<SftpProgressPayload>("sftp-progress", (event) => {
      if (event.payload.sessionId === activeSessionId) {
        setProgress(event.payload);
      }
    }).then((nextUnlisten) => {
      if (disposed) {
        nextUnlisten();
      } else {
        unlisten = nextUnlisten;
      }
    });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [activeSessionId]);

  const loadDirectory = async (nextPath: string) => {
    if (!activeSessionId) {
      return;
    }
    setLoading(true);
    try {
      const response = await invoke<SftpListResponse>("sftp_list_dir", {
        sessionId: activeSessionId,
        path: nextPath,
      });
      setPath(response.path);
      setPathDraft(response.path);
      setEntries(response.entries);
      setSelectedPath(null);
    } finally {
      setLoading(false);
    }
  };

  const goUp = () => {
    const normalized = path.replace(/\/+$/, "");
    const parent = normalized.includes("/")
      ? normalized.slice(0, normalized.lastIndexOf("/")) || "/"
      : "..";
    loadDirectory(parent).catch((error) =>
      setMessage({ kind: "error", text: String(error) }),
    );
  };

  const deleteSelected = async () => {
    if (!activeSessionId || !selectedEntry) {
      return;
    }
    setLoading(true);
    try {
      await invoke("sftp_delete", {
        request: {
          sessionId: activeSessionId,
          remotePath: selectedEntry.path,
        },
      });
      await loadDirectory(path);
      setMessage({ kind: "success", text: "Remote item deleted" });
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
    } finally {
      setLoading(false);
    }
  };

  const createFolder = async () => {
    if (!activeSessionId || !newFolderName.trim()) {
      return;
    }
    setLoading(true);
    try {
      await invoke("sftp_mkdir", {
        request: {
          sessionId: activeSessionId,
          remotePath: joinRemotePath(path, newFolderName.trim()),
        },
      });
      setNewFolderName("");
      await loadDirectory(path);
      setMessage({ kind: "success", text: "Remote folder created" });
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
    } finally {
      setLoading(false);
    }
  };

  const downloadSelected = async () => {
    if (!activeSessionId || !selectedEntry || !localPath.trim()) {
      setMessage({ kind: "error", text: "Select a file and local path" });
      return;
    }
    setLoading(true);
    try {
      await invoke("sftp_download_file", {
        request: {
          sessionId: activeSessionId,
          remotePath: selectedEntry.path,
          localPath: localPath.trim(),
        },
      });
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
    } finally {
      setLoading(false);
    }
  };

  const uploadLocalFile = async () => {
    if (!activeSessionId || !localPath.trim()) {
      setMessage({ kind: "error", text: "Enter a local file path" });
      return;
    }
    const remoteName = basename(localPath.trim());
    setLoading(true);
    try {
      await invoke("sftp_upload_file", {
        request: {
          sessionId: activeSessionId,
          remotePath: joinRemotePath(path, remoteName),
          localPath: localPath.trim(),
        },
      });
      await loadDirectory(path);
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
    } finally {
      setLoading(false);
    }
  };

  const progressPercent =
    progress?.totalBytes && progress.totalBytes > 0
      ? Math.min(100, Math.round((progress.transferredBytes / progress.totalBytes) * 100))
      : null;

  return (
    <div className="sftp-browser">
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
          onClick={() =>
            loadDirectory(path).catch((error) =>
              setMessage({ kind: "error", text: String(error) }),
            )
          }
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
          onClick={goUp}
        >
          <ArrowUp size={16} />
        </button>
        <input
          value={pathDraft}
          disabled={!connected}
          onChange={(event) => setPathDraft(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              loadDirectory(pathDraft).catch((error) =>
                setMessage({ kind: "error", text: String(error) }),
              );
            }
          }}
        />
        <button
          className="secondary-button compact"
          type="button"
          disabled={!connected || loading}
          onClick={() =>
            loadDirectory(pathDraft).catch((error) =>
              setMessage({ kind: "error", text: String(error) }),
            )
          }
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
              className={`file-row ${selectedPath === entry.path ? "selected" : ""}`}
              onClick={() => setSelectedPath(entry.path)}
              onDoubleClick={() => {
                if (entry.type === "directory") {
                  loadDirectory(entry.path).catch((error) =>
                    setMessage({ kind: "error", text: String(error) }),
                  );
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

      <div className="sftp-actions">
        <div className="path-row">
          <input
            value={newFolderName}
            disabled={!connected}
            placeholder="new-folder"
            onChange={(event) => setNewFolderName(event.target.value)}
          />
          <button
            className="secondary-button compact"
            type="button"
            disabled={!connected || loading || !newFolderName.trim()}
            onClick={createFolder}
          >
            <FolderPlus size={16} />
            Mkdir
          </button>
        </div>
        <label className="full-label">
          <span>Local path</span>
          <input
            value={localPath}
            disabled={!connected}
            placeholder="C:\\Users\\you\\Downloads\\model.log"
            onChange={(event) => setLocalPath(event.target.value)}
          />
        </label>
        <div className="button-row">
          <button
            className="secondary-button"
            type="button"
            disabled={!connected || loading || !selectedEntry}
            onClick={downloadSelected}
          >
            <Download size={16} />
            Download
          </button>
          <button
            className="secondary-button"
            type="button"
            disabled={!connected || loading || !localPath.trim()}
            onClick={uploadLocalFile}
          >
            <Upload size={16} />
            Upload
          </button>
          <button
            className="secondary-button danger"
            type="button"
            disabled={!connected || loading || !selectedEntry}
            onClick={deleteSelected}
          >
            <Trash2 size={16} />
            Delete
          </button>
        </div>
        {progress && (
          <div className="transfer-progress">
            <div>
              <strong>{progress.operation}</strong>
              <span>
                {formatBytes(progress.transferredBytes)} /{" "}
                {formatBytes(progress.totalBytes)}
              </span>
            </div>
            <div className="progress-track">
              <div
                className="progress-fill"
                style={{ width: `${progressPercent ?? 100}%` }}
              />
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

function joinRemotePath(base: string, name: string) {
  if (!base || base === ".") {
    return name;
  }
  if (base.endsWith("/")) {
    return `${base}${name}`;
  }
  return `${base}/${name}`;
}

function basename(path: string) {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? "upload.bin";
}

function formatBytes(value: number | null) {
  if (value == null) {
    return "-";
  }
  if (value < 1024) {
    return `${value} B`;
  }
  const units = ["KiB", "MiB", "GiB", "TiB"];
  let size = value / 1024;
  let unit = 0;
  while (size >= 1024 && unit < units.length - 1) {
    size /= 1024;
    unit += 1;
  }
  return `${size.toFixed(size >= 10 ? 1 : 2)} ${units[unit]}`;
}

function formatModified(value: number | null) {
  if (value == null) {
    return "-";
  }
  return new Date(value * 1000).toLocaleString();
}
