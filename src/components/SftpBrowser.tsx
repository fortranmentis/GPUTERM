import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
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
  AppSettings,
  LocalEntry,
  LocalListResponse,
  SftpEntry,
  SftpListResponse,
  SftpProgressPayload,
} from "../types/session";
import { formatBytes } from "../utils/formatBytes";

export function SftpBrowser() {
  const activeSessionId = useSessionStore((state) => state.activeSessionId);
  const connected = useSessionStore((state) => state.connected);
  const setMessage = useSessionStore((state) => state.setMessage);
  const [path, setPath] = useState(".");
  const [pathDraft, setPathDraft] = useState(".");
  const [entries, setEntries] = useState<SftpEntry[]>([]);
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const [localPath, setLocalPath] = useState("");
  const [localEntries, setLocalEntries] = useState<LocalEntry[]>([]);
  const [selectedLocalPath, setSelectedLocalPath] = useState<string | null>(null);
  const [newFolderName, setNewFolderName] = useState("");
  const [loading, setLoading] = useState(false);
  const [progress, setProgress] = useState<SftpProgressPayload | null>(null);

  const selectedEntry = useMemo(
    () => entries.find((entry) => entry.path === selectedPath) ?? null,
    [entries, selectedPath],
  );
  const selectedLocalEntry = useMemo(
    () => localEntries.find((entry) => entry.path === selectedLocalPath) ?? null,
    [localEntries, selectedLocalPath],
  );

  useEffect(() => {
    invoke<AppSettings>("load_app_settings")
      .then((settings) => {
        if (settings.recentLocalPath) {
          setLocalPath(settings.recentLocalPath);
          loadLocalDirectory(settings.recentLocalPath).catch((error) =>
            setMessage({ kind: "error", text: String(error) }),
          );
        }
      })
      .catch(() => undefined);
  }, [setMessage]);

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

  const loadLocalDirectory = async (nextPath: string) => {
    const response = await invoke<LocalListResponse>("list_local_dir", {
      path: nextPath,
    });
    setLocalPath(response.path);
    setLocalEntries(response.entries);
    setSelectedLocalPath(null);
    return response;
  };

  const persistLocalPath = async (nextPath: string) => {
    const settings = await invoke<AppSettings>("update_recent_local_path", {
      path: nextPath,
    });
    return settings.recentLocalPath ?? nextPath;
  };

  const applyLocalPath = async (nextPath: string) => {
    const normalized = await persistLocalPath(nextPath);
    await loadLocalDirectory(normalized);
  };

  const browseLocalDirectory = async () => {
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: "Select local SFTP folder",
        defaultPath: localPath || undefined,
      });
      if (selected == null || Array.isArray(selected)) {
        return;
      }
      await applyLocalPath(selected);
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
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
      setMessage({ kind: "error", text: "Select a remote file and local folder" });
      return;
    }
    if (selectedEntry.type === "directory") {
      setMessage({ kind: "error", text: "Select a remote file to download" });
      return;
    }
    const targetLocalPath = joinLocalPath(localPath.trim(), selectedEntry.name);
    setLoading(true);
    try {
      await invoke("sftp_download_file", {
        request: {
          sessionId: activeSessionId,
          remotePath: selectedEntry.path,
          localPath: targetLocalPath,
        },
      });
      await loadLocalDirectory(localPath.trim());
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
    } finally {
      setLoading(false);
    }
  };

  const uploadLocalFile = async () => {
    if (!activeSessionId || !selectedLocalEntry || selectedLocalEntry.entryType !== "file") {
      setMessage({ kind: "error", text: "Select a local file to upload" });
      return;
    }
    const remoteName = selectedLocalEntry.name;
    setLoading(true);
    try {
      await invoke("sftp_upload_file", {
        request: {
          sessionId: activeSessionId,
          remotePath: joinRemotePath(path, remoteName),
          localPath: selectedLocalEntry.path,
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
          <div className="local-path-row">
            <input
              value={localPath}
              placeholder="C:\\Users\\you\\Downloads"
              onChange={(event) => setLocalPath(event.target.value)}
              onBlur={() => {
                if (localPath.trim()) {
                  applyLocalPath(localPath).catch((error) =>
                    setMessage({ kind: "error", text: String(error) }),
                  );
                }
              }}
              onKeyDown={(event) => {
                if (event.key === "Enter" && localPath.trim()) {
                  applyLocalPath(localPath).catch((error) =>
                    setMessage({ kind: "error", text: String(error) }),
                  );
                }
              }}
            />
            <button
              className="secondary-button compact"
              type="button"
              onClick={browseLocalDirectory}
            >
              <FolderOpen size={16} />
              Browse
            </button>
          </div>
        </label>
        {localEntries.length > 0 && (
          <div className="local-file-list" aria-label="Local files">
            {localEntries.slice(0, 6).map((entry) => (
              <button
                className={`local-file-item ${
                  selectedLocalPath === entry.path ? "selected" : ""
                }`}
                key={entry.path}
                type="button"
                onClick={() => {
                  if (entry.entryType === "directory") {
                    applyLocalPath(entry.path).catch((error) =>
                      setMessage({ kind: "error", text: String(error) }),
                    );
                  } else {
                    setSelectedLocalPath(entry.path);
                  }
                }}
              >
                <span>{entry.entryType === "directory" ? "dir" : "file"}</span>
                <strong>{entry.name}</strong>
                <small>{formatBytes(entry.size)}</small>
              </button>
            ))}
          </div>
        )}
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
            disabled={!connected || loading || !selectedLocalEntry}
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

export function joinLocalPath(base: string, name: string) {
  const separator = base.includes("\\") && !base.includes("/") ? "\\" : "/";
  const trimmedBase = base.replace(/[\\/]+$/, "");
  return `${trimmedBase}${separator}${name}`;
}

function formatModified(value: number | null) {
  if (value == null) {
    return "-";
  }
  return new Date(value * 1000).toLocaleString();
}
