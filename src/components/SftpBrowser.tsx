import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type ClipboardEvent as ReactClipboardEvent,
  type DragEvent,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { confirm as confirmDialog, open } from "@tauri-apps/plugin-dialog";
import { Download, Trash2, Upload } from "lucide-react";
import { LocalFilePanel } from "./LocalFilePanel";
import { RemoteFilePanel } from "./RemoteFilePanel";
import { TransferQueue } from "./TransferQueue";
import { selectIsActiveConnected, useSessionStore } from "../stores/sessionStore";
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
type PointerTransferDrag =
  | { kind: "local"; files: LocalDragFile[] }
  | { kind: "remote"; files: RemoteDragFile[] };

type PointerDragState = {
  pointerId: number;
  startX: number;
  startY: number;
  moved: boolean;
  drag: PointerTransferDrag;
};

type SftpTransferRequest = {
  sessionId: string;
  remotePath: string;
  localPath: string;
  transferId: string;
};

type SftpBrowserProps = {
  onClose?: () => void;
};

export function SftpBrowser({ onClose }: SftpBrowserProps = {}) {
  const activeSessionId = useSessionStore((state) => state.activeSessionId);
  const sessionConnected = useSessionStore(selectIsActiveConnected);
  const activeSessionIsLocal = useSessionStore((state) =>
    state.sessions.some(
      (session) => session.id === state.activeSessionId && session.isLocal,
    ),
  );
  const connected = sessionConnected && !activeSessionIsLocal;
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
  const [creatingFolder, setCreatingFolder] = useState(false);
  const [storageSplitPercent, setStorageSplitPercent] = useState(58);
  const [loading, setLoading] = useState(false);
  const [remoteDropActive, setRemoteDropActive] = useState(false);
  const [localDropActive, setLocalDropActive] = useState(false);
  const pathBySessionRef = useRef<Map<string, string>>(new Map());
  const remoteDropZoneRef = useRef<HTMLElement>(null);
  const localDropZoneRef = useRef<HTMLElement>(null);
  const storageSplitRef = useRef<HTMLDivElement>(null);
  const storageSplitPointerRef = useRef<number | null>(null);
  const pointerDragRef = useRef<PointerDragState | null>(null);
  const suppressPointerClickRef = useRef(false);
  const enqueueUploadsRef = useRef<
    ((files: LocalDragFile[], targetDirectory: string) => Promise<void>) | null
  >(null);
  const enqueueDownloadsRef = useRef<
    ((files: RemoteDragFile[]) => Promise<void>) | null
  >(null);

  const selectedEntry = useMemo(
    () => entries.find((entry) => selectedRemotePaths.includes(entry.path)) ?? null,
    [entries, selectedRemotePaths],
  );
  const selectedRemoteEntries = useMemo(
    () =>
      entries.filter(
        (entry) =>
          selectedRemotePaths.includes(entry.path) &&
          (entry.type === "file" || entry.type === "directory"),
      ),
    [entries, selectedRemotePaths],
  );
  const selectedLocalEntries = useMemo(
    () =>
      localEntries.filter(
        (entry) =>
          selectedLocalPaths.includes(entry.path) &&
          (entry.entryType === "file" || entry.entryType === "directory"),
      ),
    [localEntries, selectedLocalPaths],
  );

  useEffect(() => {
    invoke<AppSettings>("load_app_settings")
      .then((settings) => {
        if (settings.recentLocalPath) {
          setLocalPath(settings.recentLocalPath);
          // The remembered path may have been deleted or live on an unplugged
          // drive; failing this automatic load silently keeps startup clean.
          loadLocalDirectory(settings.recentLocalPath).catch(() => undefined);
        }
      })
      .catch(() => undefined);
  }, [setMessage]);

  useEffect(() => {
    if (connected && activeSessionId) {
      // Each session remembers its own remote path; fall back to the SFTP
      // starting directory when the remembered path no longer resolves.
      const remembered = pathBySessionRef.current.get(activeSessionId) ?? ".";
      loadDirectory(remembered).catch(() =>
        loadDirectory(".").catch((error) =>
          setMessage({ kind: "error", text: String(error) }),
        ),
      );
    } else {
      setEntries([]);
      setSelectedRemotePaths([]);
      setCreatingFolder(false);
      setNewFolderName("");
    }
  }, [activeSessionId, connected]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;

    // No session filter: transfer ids are globally unique and background
    // sessions' transfers must keep updating while another session is viewed.
    listen<SftpProgressPayload>("sftp-progress", (event) => {
      updateFromProgress(event.payload);
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
  }, [updateFromProgress]);

  const loadDirectory = async (nextPath: string) => {
    const sessionId = activeSessionId;
    if (!sessionId) {
      return;
    }
    setLoading(true);
    try {
      const response = await invoke<SftpListResponse>("sftp_list_dir", {
        sessionId,
        path: nextPath,
      });
      // Record against the invoked session so a fast session switch cannot
      // attribute this listing to the wrong session.
      pathBySessionRef.current.set(sessionId, response.path);
      if (useSessionStore.getState().activeSessionId === sessionId) {
        setPath(response.path);
        setPathDraft(response.path);
        setEntries(response.entries);
        setSelectedRemotePaths([]);
      }
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
      setCreatingFolder(false);
      await loadDirectory(path);
      setMessage({ kind: "success", text: "Remote folder created" });
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
    } finally {
      setLoading(false);
    }
  };

  const downloadSelected = () => {
    if (selectedRemoteEntries.length === 0 || !localPath.trim()) {
      setMessage({ kind: "error", text: "Select remote items and a local folder" });
      return;
    }
    enqueueDownloads(selectedRemoteEntries.map(remoteEntryToDragFile));
  };

  const uploadLocalFile = () => {
    if (selectedLocalEntries.length === 0) {
      setMessage({ kind: "error", text: "Select local items to upload" });
      return;
    }
    enqueueUploads(selectedLocalEntries.map(localEntryToDragFile), path);
  };

  const enqueueUploads = async (
    files: LocalDragFile[],
    targetDirectory: string,
  ) => {
    if (!activeSessionId) {
      setMessage({ kind: "error", text: "No active SFTP session" });
      return;
    }
    const uploadable = files.filter(
      (file) => file.entryType === "file" || file.entryType === "directory",
    );
    if (uploadable.length !== files.length) {
      setMessage({
        kind: "error",
        text: "Unsupported local items were skipped",
      });
    }

    // Pin the session at enqueue time: transfers keep targeting it even if
    // the user switches sessions while they run.
    const sessionId = activeSessionId;
    await Promise.all(
      uploadable.map(async (file) => {
        const remotePath = joinRemotePath(targetDirectory, file.name);
        const task = createTransferTask(
          "upload",
          file.name,
          file.path,
          remotePath,
          file.entryType === "file" ? file.size : null,
          sessionId,
        );
        addTask(task);
        let shouldTransfer = false;
        try {
          shouldTransfer = await confirmOverwrite(
            "sftp_path_exists",
            {
              request: {
                sessionId,
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
            error: "Skipped because target item already exists",
          });
          return;
        }
        runTransfer("sftp_upload_file", task, {
          sessionId,
          remotePath,
          localPath: file.path,
          transferId: task.id,
        }).then(() => {
          if (useSessionStore.getState().activeSessionId === sessionId) {
            loadDirectory(path).catch(() => undefined);
          }
        });
      }),
    );
  };

  const enqueueDownloads = async (files: RemoteDragFile[]) => {
    if (!activeSessionId || !localPath.trim()) {
      setMessage({ kind: "error", text: "Select a local folder before download" });
      return;
    }
    const downloadable = files.filter(
      (file) => file.type === "file" || file.type === "directory",
    );
    if (downloadable.length !== files.length) {
      setMessage({
        kind: "error",
        text: "Unsupported remote items were skipped",
      });
    }

    const sessionId = activeSessionId;
    await Promise.all(
      downloadable.map(async (file) => {
        const targetLocalPath = joinLocalPath(localPath.trim(), file.name);
        const task = createTransferTask(
          "download",
          file.name,
          file.path,
          targetLocalPath,
          file.type === "file" ? file.size : null,
          sessionId,
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
            error: "Skipped because target item already exists",
          });
          return;
        }
        runTransfer("sftp_download_file", task, {
          sessionId,
          remotePath: file.path,
          localPath: targetLocalPath,
          transferId: task.id,
        }).then(() => loadLocalDirectory(localPath.trim()).catch(() => undefined));
      }),
    );
  };

  enqueueUploadsRef.current = enqueueUploads;
  enqueueDownloadsRef.current = enqueueDownloads;

  const describeAndEnqueueLocalPaths = async (
    paths: string[],
    targetDirectory: string,
  ) => {
    if (paths.length === 0) {
      return;
    }
    try {
      const droppedEntries = await invoke<LocalEntry[]>("describe_local_paths", {
        paths,
      });
      await enqueueUploads(
        droppedEntries.map(localEntryToDragFile),
        targetDirectory,
      );
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
    }
  };

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;

    // Browser File objects intentionally do not expose absolute paths on
    // WebView2/WKWebView, and WebKitGTK may omit the file payload entirely.
    // Tauri's native drop event is the cross-platform source of real paths.
    const registerNativeDrop = async () => {
      try {
        const nextUnlisten = await getCurrentWebview().onDragDropEvent((event) => {
          const payload = event.payload;
          if (payload.type === "leave") {
            setRemoteDropActive(false);
            return;
          }

          const overRemote =
            connected &&
            isPhysicalPositionInsideElement(
              payload.position,
              remoteDropZoneRef.current,
              globalThis.devicePixelRatio || 1,
            );

          if (payload.type === "enter" || payload.type === "over") {
            setRemoteDropActive(overRemote);
            return;
          }

          setRemoteDropActive(false);
          if (overRemote && payload.paths.length > 0) {
            void describeAndEnqueueLocalPaths(payload.paths, path);
          }
        });
        if (disposed) {
          nextUnlisten();
        } else {
          unlisten = nextUnlisten;
        }
      } catch (error) {
        if (!disposed) {
          setMessage({
            kind: "error",
            text: `Failed to initialize native file drop: ${String(error)}`,
          });
        }
      }
    };
    void registerNativeDrop();

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [activeSessionId, connected, path, setMessage]);

  useEffect(() => {
    const clearPointerDrag = () => {
      pointerDragRef.current = null;
      clearActiveDrag();
      setRemoteDropActive(false);
      setLocalDropActive(false);
      document.body.classList.remove("sftp-pointer-dragging");
    };

    const onPointerMove = (event: PointerEvent) => {
      const state = pointerDragRef.current;
      if (!state || event.pointerId !== state.pointerId) {
        return;
      }
      if (!state.moved) {
        state.moved =
          Math.hypot(event.clientX - state.startX, event.clientY - state.startY) >= 5;
      }
      if (!state.moved) {
        return;
      }

      event.preventDefault();
      document.body.classList.add("sftp-pointer-dragging");
      setRemoteDropActive(
        state.drag.kind === "local" &&
          isClientPositionInsideElement(
            event.clientX,
            event.clientY,
            remoteDropZoneRef.current,
          ),
      );
      setLocalDropActive(
        state.drag.kind === "remote" &&
          isClientPositionInsideElement(
            event.clientX,
            event.clientY,
            localDropZoneRef.current,
          ),
      );
    };

    const onPointerUp = (event: PointerEvent) => {
      const state = pointerDragRef.current;
      if (!state || event.pointerId !== state.pointerId) {
        return;
      }

      if (state.moved) {
        event.preventDefault();
        suppressPointerClickRef.current = true;
        globalThis.setTimeout(() => {
          suppressPointerClickRef.current = false;
        }, 0);

        if (
          state.drag.kind === "local" &&
          isClientPositionInsideElement(
            event.clientX,
            event.clientY,
            remoteDropZoneRef.current,
          )
        ) {
          void enqueueUploadsRef.current?.(state.drag.files, path);
        } else if (
          state.drag.kind === "remote" &&
          isClientPositionInsideElement(
            event.clientX,
            event.clientY,
            localDropZoneRef.current,
          )
        ) {
          void enqueueDownloadsRef.current?.(state.drag.files);
        }
      }
      clearPointerDrag();
    };

    document.addEventListener("pointermove", onPointerMove, { passive: false });
    document.addEventListener("pointerup", onPointerUp, { passive: false });
    document.addEventListener("pointercancel", clearPointerDrag);
    return () => {
      document.removeEventListener("pointermove", onPointerMove);
      document.removeEventListener("pointerup", onPointerUp);
      document.removeEventListener("pointercancel", clearPointerDrag);
      pointerDragRef.current = null;
      clearActiveDrag();
      document.body.classList.remove("sftp-pointer-dragging");
    };
  }, [clearActiveDrag, path]);

  const confirmOverwrite = async (
    command: "sftp_path_exists" | "local_path_exists",
    args: Record<string, unknown>,
    targetPath: string,
  ) => {
    const exists = await invoke<boolean>(command, args);
    if (!exists) {
      return true;
    }
    return confirmDialog(`Replace or merge the existing item?\n${targetPath}`, {
      title: "Confirm replacement",
      kind: "warning",
      okLabel: "Replace / Merge",
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
      const currentTask = useTransferStore
        .getState()
        .tasks.find((candidate) => candidate.id === task.id);
      const totalBytes = currentTask?.totalBytes ?? task.totalBytes;
      updateTask(task.id, {
        status: "done",
        transferredBytes:
          totalBytes ?? currentTask?.transferredBytes ?? task.transferredBytes,
        progressPercent: 100,
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

  const startPointerDrag = (
    drag: PointerTransferDrag,
    event: ReactPointerEvent<HTMLButtonElement>,
  ) => {
    if (event.button !== 0) {
      return;
    }
    event.currentTarget.setPointerCapture?.(event.pointerId);
    pointerDragRef.current = {
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      moved: false,
      drag,
    };
    setActiveDrag(drag);
  };

  const pointerDragLocalEntry = (
    entry: LocalEntry,
    event: ReactPointerEvent<HTMLButtonElement>,
  ) => {
    const entriesToDrag =
      selectedLocalPaths.includes(entry.path) && selectedLocalEntries.length > 0
        ? selectedLocalEntries
        : [entry];
    startPointerDrag(
      { kind: "local", files: entriesToDrag.map(localEntryToDragFile) },
      event,
    );
  };

  const pointerDragRemoteEntry = (
    entry: SftpEntry,
    event: ReactPointerEvent<HTMLButtonElement>,
  ) => {
    const entriesToDrag =
      selectedRemotePaths.includes(entry.path) && selectedRemotePaths.length > 1
        ? entries.filter((candidate) => selectedRemotePaths.includes(candidate.path))
        : [entry];
    startPointerDrag(
      { kind: "remote", files: entriesToDrag.map(remoteEntryToDragFile) },
      event,
    );
  };

  const consumePointerDragClick = () => {
    if (!suppressPointerClickRef.current) {
      return false;
    }
    suppressPointerClickRef.current = false;
    return true;
  };

  const dropLocalFilesOnRemote = async (event: DragEvent<HTMLDivElement>) => {
    event.preventDefault();
    setRemoteDropActive(false);
    const currentDrag = useTransferStore.getState().activeDrag;
    const files =
      currentDrag?.kind === "local"
        ? currentDrag.files
        : readLocalDragFiles(event.dataTransfer);
    clearActiveDrag();
    if (files.length === 0) {
      const paths = readClipboardLocalPaths(event.dataTransfer);
      if (paths.length > 0) {
        await describeAndEnqueueLocalPaths(paths, path);
        return;
      }
      setMessage({
        kind: "error",
        text: "The dropped item path was unavailable. Please retry the desktop file drop.",
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

  const pasteLocalFilesOnRemote = async (
    event: ReactClipboardEvent<HTMLElement>,
  ) => {
    const target = event.target as HTMLElement;
    if (target.closest("input, textarea, [contenteditable='true']")) {
      return;
    }
    const paths = readClipboardLocalPaths(event.clipboardData);
    if (paths.length === 0) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    try {
      const pastedEntries = await invoke<LocalEntry[]>("describe_local_paths", {
        paths,
      });
      await enqueueUploads(pastedEntries.map(localEntryToDragFile), path);
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
    }
  };

  const updateStorageSplitFromClientY = (clientY: number) => {
    const bounds = storageSplitRef.current?.getBoundingClientRect();
    if (!bounds || bounds.height <= 0) {
      return;
    }
    setStorageSplitPercent(
      clampStorageSplit(((clientY - bounds.top) / bounds.height) * 100),
    );
  };

  return (
    <div className="sftp-browser">
      <div
        ref={storageSplitRef}
        className="sftp-storage-split"
        style={{
          gridTemplateRows: `minmax(0, ${storageSplitPercent}fr) 7px minmax(0, ${100 - storageSplitPercent}fr)`,
        }}
      >
        <RemoteFilePanel
          onClose={onClose}
          containerRef={remoteDropZoneRef}
          connected={connected}
          loading={loading}
          path={path}
          pathDraft={pathDraft}
          entries={entries}
          selectedPaths={selectedRemotePaths}
          dropActive={remoteDropActive}
          creatingFolder={creatingFolder}
          newFolderName={newFolderName}
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
          onPointerDragStart={pointerDragRemoteEntry}
          onConsumePointerDragClick={consumePointerDragClick}
          onDropLocalFiles={dropLocalFilesOnRemote}
          onDragOverLocalFiles={(event) => {
            event.preventDefault();
            setRemoteDropActive(true);
          }}
          onDragLeaveLocalFiles={() => setRemoteDropActive(false)}
          onDropRemoteOnDirectory={dropRemoteOnDirectory}
          onPasteLocalFiles={pasteLocalFilesOnRemote}
          onBeginCreateFolder={() => setCreatingFolder(true)}
          onNewFolderNameChange={setNewFolderName}
          onCreateFolder={createFolder}
          onCancelCreateFolder={() => {
            setNewFolderName("");
            setCreatingFolder(false);
          }}
        />

        <div
          className="sftp-storage-splitter"
          role="separator"
          aria-label="Resize remote and local file panels"
          aria-orientation="horizontal"
          aria-valuemin={25}
          aria-valuemax={75}
          aria-valuenow={Math.round(storageSplitPercent)}
          tabIndex={0}
          onPointerDown={(event) => {
            storageSplitPointerRef.current = event.pointerId;
            event.currentTarget.setPointerCapture?.(event.pointerId);
            updateStorageSplitFromClientY(event.clientY);
          }}
          onPointerMove={(event) => {
            if (storageSplitPointerRef.current === event.pointerId) {
              updateStorageSplitFromClientY(event.clientY);
            }
          }}
          onPointerUp={(event) => {
            if (storageSplitPointerRef.current === event.pointerId) {
              storageSplitPointerRef.current = null;
              event.currentTarget.releasePointerCapture?.(event.pointerId);
            }
          }}
          onPointerCancel={() => {
            storageSplitPointerRef.current = null;
          }}
          onKeyDown={(event) => {
            if (event.key === "ArrowUp" || event.key === "ArrowDown") {
              event.preventDefault();
              const delta = event.key === "ArrowUp" ? -5 : 5;
              setStorageSplitPercent((current) =>
                clampStorageSplit(current + delta),
              );
            }
          }}
        />

        <LocalFilePanel
          containerRef={localDropZoneRef}
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
          onPointerDragStart={pointerDragLocalEntry}
          onConsumePointerDragClick={consumePointerDragClick}
          onDropRemoteFiles={dropRemoteFilesOnLocal}
          onDragOverRemoteFiles={(event) => {
            event.preventDefault();
            setLocalDropActive(true);
          }}
          onDragLeaveRemoteFiles={() => setLocalDropActive(false)}
        />
      </div>

      <div className="sftp-actions">
        <div className="button-row">
          <button
            className="secondary-button"
            type="button"
            disabled={!connected || loading || selectedRemoteEntries.length === 0}
            onClick={downloadSelected}
          >
            <Download size={16} />
            Download
          </button>
          <button
            className="secondary-button"
            type="button"
            disabled={!connected || loading || selectedLocalEntries.length === 0}
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

function clampStorageSplit(value: number) {
  return Math.min(75, Math.max(25, value));
}

function createTransferTask(
  direction: TransferDirection,
  filename: string,
  sourcePath: string,
  targetPath: string,
  totalBytes: number | null,
  sessionId?: string,
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
    sessionId,
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
  return Array.from(dataTransfer.files ?? []).flatMap((file) => {
    const path = (file as File & { path?: string }).path;
    return path
      ? [
          {
            name: file.name,
            path,
            entryType: "file" as const,
            size: file.size,
          },
        ]
      : [];
  });
}

function isPhysicalPositionInsideElement(
  position: { x: number; y: number },
  element: HTMLElement | null,
  scaleFactor: number,
) {
  const safeScaleFactor = scaleFactor > 0 ? scaleFactor : 1;
  return isClientPositionInsideElement(
    position.x / safeScaleFactor,
    position.y / safeScaleFactor,
    element,
  );
}

function isClientPositionInsideElement(
  clientX: number,
  clientY: number,
  element: HTMLElement | null,
) {
  if (!element) {
    return false;
  }
  const rect = element.getBoundingClientRect();
  return (
    clientX >= rect.left &&
    clientX <= rect.right &&
    clientY >= rect.top &&
    clientY <= rect.bottom
  );
}

function readRemoteDragFiles(dataTransfer: DataTransfer): RemoteDragFile[] {
  const encoded = dataTransfer.getData(REMOTE_DRAG_TYPE);
  return encoded ? safeParseDragFiles<RemoteDragFile>(encoded) : [];
}

export function readClipboardLocalPaths(clipboardData: DataTransfer): string[] {
  const filePaths = Array.from(clipboardData.files ?? [])
    .map((file) => (file as File & { path?: string }).path)
    .filter((path): path is string => Boolean(path));
  const payloads = [
    "x-special/gnome-copied-files",
    "text/uri-list",
    "text/plain",
  ]
    .map((type) => {
      try {
        return clipboardData.getData(type);
      } catch {
        return "";
      }
    })
    .filter(Boolean);

  const textPaths = payloads.flatMap((payload) =>
    payload
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter(
        (line) =>
          line.length > 0 &&
          line !== "copy" &&
          line !== "cut" &&
          !line.startsWith("#"),
      )
      .map(fileClipboardLineToPath)
      .filter((path): path is string => path != null),
  );
  return [...new Set([...filePaths, ...textPaths])];
}

function fileClipboardLineToPath(line: string): string | null {
  const unquoted =
    line.length >= 2 &&
    ((line.startsWith('"') && line.endsWith('"')) ||
      (line.startsWith("'") && line.endsWith("'")))
      ? line.slice(1, -1)
      : line;
  if (unquoted.startsWith("file://")) {
    try {
      const url = new URL(unquoted);
      let pathname = decodeURIComponent(url.pathname);
      if (/^\/[A-Za-z]:\//.test(pathname)) {
        pathname = pathname.slice(1);
      }
      if (url.hostname && url.hostname !== "localhost") {
        return `//${url.hostname}${pathname}`;
      }
      return pathname;
    } catch {
      return null;
    }
  }
  return unquoted.startsWith("/") || /^[A-Za-z]:[\\/]/.test(unquoted)
    ? unquoted
    : null;
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
