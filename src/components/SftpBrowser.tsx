import { useEffect, useMemo, useState, type DragEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { confirm as confirmDialog, open } from "@tauri-apps/plugin-dialog";
import { Download, FolderPlus, Trash2, Upload } from "lucide-react";
import { LocalFilePanel } from "./LocalFilePanel";
import { RemoteFilePanel } from "./RemoteFilePanel";
import { TransferQueue } from "./TransferQueue";
import { useSessionStore } from "../stores/sessionStore";
import { useTransferStore } from "../stores/transferStore";
import type {
  AppSettings,
  LocalEntry,
  LocalListResponse,
  SftpEntry,
  SftpListResponse,
  SftpProgressPayload,
} from "../types/session";
import type {
  LocalTransferDragFile,
  RemoteTransferDragFile,
  TransferDirection,
  TransferTask,
} from "../types/transfer";

const LOCAL_DRAG_TYPE = "application/x-gputerm-local-files";
const REMOTE_DRAG_TYPE = "application/x-gputerm-remote-files";

type LocalDragFile = LocalTransferDragFile;
type RemoteDragFile = RemoteTransferDragFile;

type SftpTransferRequest = {
  sessionId: string;
  remotePath: string;
  localPath: string;
  transferId: string;
};

export function SftpBrowser() {
  const activeSessionId = useSessionStore((state) => state.activeSessionId);
  const connected = useSessionStore((state) => state.connected);
  const setMessage = useSessionStore((state) => state.setMessage);
  const addTask = useTransferStore((state) => state.addTask);
  const updateTask = useTransferStore((state) => state.updateTask);
  const updateFromProgress = useTransferStore((state) => state.updateFromProgress);
  const setActiveDrag = useTransferStore((state) => state.setActiveDrag);
  const clearActiveDrag = useTransferStore((state) => state.clearActiveDrag);
  const [path, setPath] = useState(".");
  const [pathDraft, setPathDraft] = useState(".");
  const [entries, setEntries] = useState<SftpEntry[]>([]);
  const [selectedRemotePaths, setSelectedRemotePaths] = useState<string[]>([]);
  const [localPath, setLocalPath] = useState("");
  const [localEntries, setLocalEntries] = useState<LocalEntry[]>([]);
  const [selectedLocalPaths, setSelectedLocalPaths] = useState<string[]>([]);
  const [newFolderName, setNewFolderName] = useState("");
  const [loading, setLoading] = useState(false);
  const [remoteDropActive, setRemoteDropActive] = useState(false);
  const [localDropActive, setLocalDropActive] = useState(false);

  const selectedEntry = useMemo(
    () => entries.find((entry) => selectedRemotePaths.includes(entry.path)) ?? null,
    [entries, selectedRemotePaths],
  );
  const selectedRemoteFiles = useMemo(
    () =>
      entries.filter(
        (entry) => selectedRemotePaths.includes(entry.path) && entry.type === "file",
      ),
    [entries, selectedRemotePaths],
  );
  const selectedLocalFiles = useMemo(
    () =>
      localEntries.filter(
        (entry) =>
          selectedLocalPaths.includes(entry.path) && entry.entryType === "file",
      ),
    [localEntries, selectedLocalPaths],
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
      setSelectedRemotePaths([]);
    }
  }, [activeSessionId, connected]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;

    listen<SftpProgressPayload>("sftp-progress", (event) => {
      if (event.payload.sessionId === activeSessionId) {
        updateFromProgress(event.payload);
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
  }, [activeSessionId, updateFromProgress]);

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
      setSelectedRemotePaths([]);
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
    setSelectedLocalPaths([]);
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

  const downloadSelected = () => {
    if (selectedRemoteFiles.length === 0 || !localPath.trim()) {
      setMessage({ kind: "error", text: "Select remote files and a local folder" });
      return;
    }
    enqueueDownloads(selectedRemoteFiles.map(remoteEntryToDragFile));
  };

  const uploadLocalFile = () => {
    if (selectedLocalFiles.length === 0) {
      setMessage({ kind: "error", text: "Select local files to upload" });
      return;
    }
    enqueueUploads(selectedLocalFiles.map(localEntryToDragFile), path);
  };

  const enqueueUploads = async (
    files: LocalDragFile[],
    targetDirectory: string,
  ) => {
    if (!activeSessionId) {
      setMessage({ kind: "error", text: "No active SFTP session" });
      return;
    }
    const uploadable = files.filter((file) => file.entryType === "file");
    if (files.some((file) => file.entryType === "directory")) {
      setMessage({
        kind: "error",
        text: "Directory drag-and-drop is not supported yet",
      });
    }

    await Promise.all(
      uploadable.map(async (file) => {
        const remotePath = joinRemotePath(targetDirectory, file.name);
        const task = createTransferTask(
          "upload",
          file.name,
          file.path,
          remotePath,
          file.size,
        );
        addTask(task);
        let shouldTransfer = false;
        try {
          shouldTransfer = await confirmOverwrite(
            "sftp_path_exists",
            {
              request: {
                sessionId: activeSessionId,
                remotePath,
              },
            },
            remotePath,
          );
        } catch (error) {
          updateTask(task.id, {
            status: "failed",
            error: String(error),
          });
          return;
        }
        if (!shouldTransfer) {
          updateTask(task.id, {
            status: "canceled",
            error: "Skipped because target file already exists",
          });
          return;
        }
        runTransfer("sftp_upload_file", task, {
          sessionId: activeSessionId,
          remotePath,
          localPath: file.path,
          transferId: task.id,
        }).then(() => loadDirectory(path).catch(() => undefined));
      }),
    );
  };

  const enqueueDownloads = async (files: RemoteDragFile[]) => {
    if (!activeSessionId || !localPath.trim()) {
      setMessage({ kind: "error", text: "Select a local folder before download" });
      return;
    }
    const downloadable = files.filter((file) => file.type === "file");
    if (files.some((file) => file.type === "directory")) {
      setMessage({
        kind: "error",
        text: "Directory drag-and-drop is not supported yet",
      });
    }

    await Promise.all(
      downloadable.map(async (file) => {
        const targetLocalPath = joinLocalPath(localPath.trim(), file.name);
        const task = createTransferTask(
          "download",
          file.name,
          file.path,
          targetLocalPath,
          file.size,
        );
        addTask(task);
        let shouldTransfer = false;
        try {
          shouldTransfer = await confirmOverwrite(
            "local_path_exists",
            { path: targetLocalPath },
            targetLocalPath,
          );
        } catch (error) {
          updateTask(task.id, {
            status: "failed",
            error: String(error),
          });
          return;
        }
        if (!shouldTransfer) {
          updateTask(task.id, {
            status: "canceled",
            error: "Skipped because target file already exists",
          });
          return;
        }
        runTransfer("sftp_download_file", task, {
          sessionId: activeSessionId,
          remotePath: file.path,
          localPath: targetLocalPath,
          transferId: task.id,
        }).then(() => loadLocalDirectory(localPath.trim()).catch(() => undefined));
      }),
    );
  };

  const confirmOverwrite = async (
    command: "sftp_path_exists" | "local_path_exists",
    args: Record<string, unknown>,
    targetPath: string,
  ) => {
    const exists = await invoke<boolean>(command, args);
    if (!exists) {
      return true;
    }
    return confirmDialog(`Overwrite existing file?\n${targetPath}`, {
      title: "Confirm overwrite",
      kind: "warning",
      okLabel: "Overwrite",
      cancelLabel: "Skip",
    });
  };

  const runTransfer = async (
    command: "sftp_upload_file" | "sftp_download_file",
    task: TransferTask,
    request: SftpTransferRequest,
  ) => {
    updateTask(task.id, { status: "running" });
    try {
      await invoke(command, { request });
      updateTask(task.id, {
        status: "done",
        transferredBytes: task.totalBytes ?? task.transferredBytes,
        progressPercent: task.totalBytes == null ? null : 100,
      });
    } catch (error) {
      updateTask(task.id, {
        status: "failed",
        error: String(error),
      });
      setMessage({ kind: "error", text: String(error) });
    }
  };

  const selectRemoteEntry = (entry: SftpEntry, additive: boolean) => {
    setSelectedRemotePaths((current) =>
      additive
        ? toggleSelection(current, entry.path)
        : current.includes(entry.path) && current.length === 1
          ? current
          : [entry.path],
    );
  };

  const selectLocalEntry = (entry: LocalEntry, additive: boolean) => {
    setSelectedLocalPaths((current) =>
      additive ? toggleSelection(current, entry.path) : [entry.path],
    );
  };

  const dragLocalEntry = (
    entry: LocalEntry,
    event: DragEvent<HTMLButtonElement>,
  ) => {
    const entriesToDrag =
      selectedLocalPaths.includes(entry.path) && selectedLocalFiles.length > 0
        ? selectedLocalFiles
        : [entry];
    event.dataTransfer.effectAllowed = "copy";
    setActiveDrag({
      kind: "local",
      files: entriesToDrag.map(localEntryToDragFile),
    });
    event.dataTransfer.setData(
      LOCAL_DRAG_TYPE,
      JSON.stringify(entriesToDrag.map(localEntryToDragFile)),
    );
  };

  const dragRemoteEntry = (
    entry: SftpEntry,
    event: DragEvent<HTMLButtonElement>,
  ) => {
    const entriesToDrag =
      selectedRemotePaths.includes(entry.path) && selectedRemotePaths.length > 1
        ? entries.filter((candidate) => selectedRemotePaths.includes(candidate.path))
        : [entry];
    event.dataTransfer.effectAllowed = "copy";
    setActiveDrag({
      kind: "remote",
      files: entriesToDrag.map(remoteEntryToDragFile),
    });
    event.dataTransfer.setData(
      REMOTE_DRAG_TYPE,
      JSON.stringify(entriesToDrag.map(remoteEntryToDragFile)),
    );
  };

  const dropLocalFilesOnRemote = (event: DragEvent<HTMLDivElement>) => {
    event.preventDefault();
    setRemoteDropActive(false);
    const currentDrag = useTransferStore.getState().activeDrag;
    const files =
      currentDrag?.kind === "local"
        ? currentDrag.files
        : readLocalDragFiles(event.dataTransfer);
    clearActiveDrag();
    if (files.length === 0) {
      setMessage({
        kind: "error",
        text: "No local file payload found. Drag files from the local panel.",
      });
      return;
    }
    enqueueUploads(files, path);
  };

  const dropRemoteFilesOnLocal = (event: DragEvent<HTMLDivElement>) => {
    event.preventDefault();
    setLocalDropActive(false);
    const currentDrag = useTransferStore.getState().activeDrag;
    const files =
      currentDrag?.kind === "remote"
        ? currentDrag.files
        : readRemoteDragFiles(event.dataTransfer);
    clearActiveDrag();
    if (files.length === 0) {
      setMessage({
        kind: "error",
        text: "No remote file payload found. Drag files from the remote panel.",
      });
      return;
    }
    enqueueDownloads(files);
  };

  const dropRemoteOnDirectory = (
    _targetDirectory: SftpEntry,
    event: DragEvent<HTMLButtonElement>,
  ) => {
    event.preventDefault();
    event.stopPropagation();
    const currentDrag = useTransferStore.getState().activeDrag;
    if (
      currentDrag?.kind === "remote" ||
      readRemoteDragFiles(event.dataTransfer).length > 0
    ) {
      clearActiveDrag();
      setMessage({
        kind: "info",
        text: "Remote move/copy by drag-and-drop is not implemented yet",
      });
    }
  };

  return (
    <div className="sftp-browser">
      <RemoteFilePanel
        connected={connected}
        loading={loading}
        path={path}
        pathDraft={pathDraft}
        entries={entries}
        selectedPaths={selectedRemotePaths}
        dropActive={remoteDropActive}
        onPathDraftChange={setPathDraft}
        onOpenPath={(nextPath) =>
          loadDirectory(nextPath).catch((error) =>
            setMessage({ kind: "error", text: String(error) }),
          )
        }
        onRefresh={() =>
          loadDirectory(path).catch((error) =>
            setMessage({ kind: "error", text: String(error) }),
          )
        }
        onGoUp={goUp}
        onSelectEntry={selectRemoteEntry}
        onOpenDirectory={(nextPath) =>
          loadDirectory(nextPath).catch((error) =>
            setMessage({ kind: "error", text: String(error) }),
          )
        }
        onDragStart={dragRemoteEntry}
        onDragEnd={clearActiveDrag}
        onDropLocalFiles={dropLocalFilesOnRemote}
        onDragOverLocalFiles={(event) => {
          event.preventDefault();
          setRemoteDropActive(true);
        }}
        onDragLeaveLocalFiles={() => setRemoteDropActive(false)}
        onDropRemoteOnDirectory={dropRemoteOnDirectory}
      />

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

        <LocalFilePanel
          localPath={localPath}
          entries={localEntries}
          selectedPaths={selectedLocalPaths}
          dropActive={localDropActive}
          onPathChange={setLocalPath}
          onApplyPath={(nextPath) =>
            applyLocalPath(nextPath).catch((error) =>
              setMessage({ kind: "error", text: String(error) }),
            )
          }
          onBrowse={browseLocalDirectory}
          onSelectEntry={selectLocalEntry}
          onOpenDirectory={(nextPath) =>
            applyLocalPath(nextPath).catch((error) =>
              setMessage({ kind: "error", text: String(error) }),
            )
          }
          onDragStart={dragLocalEntry}
          onDragEnd={clearActiveDrag}
          onDropRemoteFiles={dropRemoteFilesOnLocal}
          onDragOverRemoteFiles={(event) => {
            event.preventDefault();
            setLocalDropActive(true);
          }}
          onDragLeaveRemoteFiles={() => setLocalDropActive(false)}
        />

        <div className="button-row">
          <button
            className="secondary-button"
            type="button"
            disabled={!connected || loading || selectedRemoteFiles.length === 0}
            onClick={downloadSelected}
          >
            <Download size={16} />
            Download
          </button>
          <button
            className="secondary-button"
            type="button"
            disabled={!connected || loading || selectedLocalFiles.length === 0}
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

        <TransferQueue />
      </div>
    </div>
  );
}

function createTransferTask(
  direction: TransferDirection,
  filename: string,
  sourcePath: string,
  targetPath: string,
  totalBytes: number | null,
): TransferTask {
  return {
    id: createTransferId(),
    direction,
    filename,
    sourcePath,
    targetPath,
    totalBytes,
    transferredBytes: 0,
    progressPercent: totalBytes === 0 ? 100 : null,
    status: "pending",
  };
}

function createTransferId() {
  return globalThis.crypto?.randomUUID?.() ?? `transfer-${Date.now()}-${Math.random()}`;
}

function toggleSelection(current: string[], path: string) {
  return current.includes(path)
    ? current.filter((item) => item !== path)
    : [...current, path];
}

function localEntryToDragFile(entry: LocalEntry): LocalDragFile {
  return {
    name: entry.name,
    path: entry.path,
    entryType: entry.entryType,
    size: entry.size,
  };
}

function remoteEntryToDragFile(entry: SftpEntry): RemoteDragFile {
  return {
    name: entry.name,
    path: entry.path,
    type: entry.type,
    size: entry.size,
  };
}

function readLocalDragFiles(dataTransfer: DataTransfer): LocalDragFile[] {
  const encoded = dataTransfer.getData(LOCAL_DRAG_TYPE);
  if (encoded) {
    return safeParseDragFiles<LocalDragFile>(encoded);
  }
  return Array.from(dataTransfer.files ?? [])
    .map((file) => {
      const path = (file as File & { path?: string }).path ?? file.name;
      return {
        name: file.name,
        path,
        entryType: "file" as const,
        size: file.size,
      };
    })
    .filter((file) => Boolean(file.path));
}

function readRemoteDragFiles(dataTransfer: DataTransfer): RemoteDragFile[] {
  const encoded = dataTransfer.getData(REMOTE_DRAG_TYPE);
  return encoded ? safeParseDragFiles<RemoteDragFile>(encoded) : [];
}

function safeParseDragFiles<T>(encoded: string): T[] {
  try {
    const parsed = JSON.parse(encoded);
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
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
