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
import {
  confirm as confirmDialog,
  message as messageDialog,
  open,
} from "@tauri-apps/plugin-dialog";
import { Download, PenLine, Trash2, Upload } from "lucide-react";
import { LocalFilePanel } from "./LocalFilePanel";
import { RemoteFilePanel } from "./RemoteFilePanel";
import { TransferQueue } from "./TransferQueue";
import { selectIsActiveConnected, useSessionStore } from "../stores/sessionStore";
import { useTransferStore } from "../stores/transferStore";
import { mapWithConcurrencyLimit } from "../utils/concurrency";
import {
  applySelectionGesture,
  countLabel,
  selectAll,
  toggleAll,
  type FileSelection,
  type SelectionGesture,
} from "../utils/fileSelection";
import { startNativeFileDrag } from "../utils/nativeFileDrag";
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
// Each transfer opens its own SSH connection, so a folder drop must not
// launch one handshake per file simultaneously.
const MAX_CONCURRENT_TRANSFERS = 3;

type LocalDragFile = LocalTransferDragFile;
type RemoteDragFile = RemoteTransferDragFile;
type PointerTransferDrag =
  | { kind: "local"; files: LocalDragFile[] }
  | { kind: "remote"; files: RemoteDragFile[] };

type PointerDragState = {
  pointerId: number;
  captureTarget: HTMLButtonElement;
  startX: number;
  startY: number;
  moved: boolean;
  drag: PointerTransferDrag;
  externalIntent: boolean;
  externalStarted: boolean;
  externalPreparation?: Promise<string[]>;
  externalPaths?: string[];
};

type SftpDragOutPath = {
  remotePath: string;
  localPath: string;
};

type SftpTransferRequest = {
  sessionId: string;
  remotePath: string;
  localPath: string;
  transferId: string;
  /** Folders only: leave files that already exist at the destination. */
  keepExisting?: boolean;
};

/** What to do when a transfer's destination already exists. */
type OverwriteDecision = "replace" | "keep" | "skip";

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
  const [renamingPath, setRenamingPath] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");
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
  const dragOutCacheRef = useRef<Map<string, Promise<string[]>>>(new Map());
  const suppressPointerClickRef = useRef(false);
  const remoteAnchorRef = useRef<string | null>(null);
  const localAnchorRef = useRef<string | null>(null);
  // The local folder last listed. `localPath` doubles as the path input's
  // draft, so it cannot tell which folder the panel actually shows.
  const loadedLocalDirRef = useRef<string | null>(null);
  const enqueueUploadsRef = useRef<
    ((files: LocalDragFile[], targetDirectory: string) => Promise<void>) | null
  >(null);
  const enqueueDownloadsRef = useRef<
    ((files: RemoteDragFile[]) => Promise<void>) | null
  >(null);
  const moveRemoteFilesRef = useRef<
    ((files: RemoteDragFile[], targetDirectory: string) => Promise<void>) | null
  >(null);
  const prepareRemoteDragOutRef = useRef<
    ((files: RemoteDragFile[]) => Promise<string[]>) | null
  >(null);

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
      remoteAnchorRef.current = null;
      setCreatingFolder(false);
      setNewFolderName("");
    }
    setRenamingPath(null);
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
        remoteAnchorRef.current = null;
      }
    } finally {
      setLoading(false);
    }
  };

  const loadLocalDirectory = async (nextPath: string) => {
    const response = await invoke<LocalListResponse>("list_local_dir", {
      path: nextPath,
    });
    loadedLocalDirRef.current = response.path;
    setLocalPath(response.path);
    setLocalEntries(response.entries);
    setSelectedLocalPaths([]);
    localAnchorRef.current = null;
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
    if (!activeSessionId || selectedRemoteEntries.length === 0) {
      return;
    }
    // Deleting is irreversible, so it asks first — the far milder overwrite path
    // already does. Every selected entry is deleted, not just the first one in
    // directory order.
    const summary =
      selectedRemoteEntries.length === 1
        ? selectedRemoteEntries[0].name
        : `${selectedRemoteEntries.length} items`;
    const confirmed = await confirmDialog(
      `Delete ${summary} from the remote host? This cannot be undone.`,
      { title: "Delete remote items", kind: "warning" },
    );
    if (!confirmed) {
      return;
    }

    setLoading(true);
    const failures: string[] = [];
    try {
      for (const entry of selectedRemoteEntries) {
        try {
          await invoke("sftp_delete", {
            request: { sessionId: activeSessionId, remotePath: entry.path },
          });
        } catch (error) {
          failures.push(`${entry.name}: ${String(error)}`);
        }
      }
      await loadDirectory(path);
      if (failures.length > 0) {
        setMessage({ kind: "error", text: failures.join("\n") });
      } else {
        setMessage({
          kind: "success",
          text:
            selectedRemoteEntries.length === 1
              ? "Remote item deleted"
              : `${selectedRemoteEntries.length} remote items deleted`,
        });
      }
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
    } finally {
      setLoading(false);
    }
  };

  const beginRename = () => {
    const entry = selectedRemoteEntries[0];
    if (selectedRemoteEntries.length !== 1 || !entry) {
      return;
    }
    setCreatingFolder(false);
    setRenamingPath(entry.path);
    setRenameDraft(entry.name);
  };

  const applyRename = async () => {
    const entry = entries.find((candidate) => candidate.path === renamingPath);
    const name = renameDraft.trim();
    if (!activeSessionId || !entry || !name) {
      return;
    }
    if (name === entry.name) {
      setRenamingPath(null);
      return;
    }
    if (name.includes("/") || name === "." || name === "..") {
      setMessage({ kind: "error", text: "A name cannot contain '/' or be '.' or '..'" });
      return;
    }
    const targetPath = joinRemotePath(parentRemotePath(entry.path), name);
    setLoading(true);
    try {
      await invoke("sftp_rename", {
        request: {
          sessionId: activeSessionId,
          sourcePath: entry.path,
          targetPath,
        },
      });
      setRenamingPath(null);
      setSelectedRemotePaths([targetPath]);
      await loadDirectory(path);
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
    } finally {
      setLoading(false);
    }
  };

  /** Moves remote items into another remote folder with a server-side rename. */
  const moveRemoteFiles = async (
    files: RemoteDragFile[],
    targetDirectory: string,
  ) => {
    if (!activeSessionId) {
      return;
    }
    const movable = files.filter(
      (file) =>
        file.path !== targetDirectory &&
        // A folder cannot move into itself or its own subtree.
        !targetDirectory.startsWith(`${file.path.replace(/\/+$/, "")}/`) &&
        parentRemotePath(file.path) !== targetDirectory,
    );
    if (movable.length === 0) {
      return;
    }
    setLoading(true);
    const failures: string[] = [];
    try {
      for (const file of movable) {
        try {
          await invoke("sftp_rename", {
            request: {
              sessionId: activeSessionId,
              sourcePath: file.path,
              targetPath: joinRemotePath(targetDirectory, file.name),
            },
          });
        } catch (error) {
          failures.push(`${file.name}: ${String(error)}`);
        }
      }
      await loadDirectory(path);
      if (failures.length > 0) {
        setMessage({ kind: "error", text: failures.join("\n") });
      } else {
        setMessage({
          kind: "success",
          text: `Moved ${movable.length === 1 ? movable[0].name : `${movable.length} items`} to ${targetDirectory}`,
        });
      }
    } finally {
      setLoading(false);
    }
  };
  moveRemoteFilesRef.current = moveRemoteFiles;

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
    // Confirmations run one at a time: mapping them in parallel stacked one
    // native dialog per dropped file.
    const approved: Array<{ task: TransferTask; request: SftpTransferRequest }> = [];
    for (const file of uploadable) {
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
      let decision: OverwriteDecision = "skip";
      try {
        decision = await confirmOverwrite(
          "sftp_path_exists",
          {
            request: {
              sessionId,
              remotePath,
            },
          },
          remotePath,
          file.entryType === "directory",
        );
      } catch (error) {
        updateTask(task.id, {
          status: "failed",
          error: String(error),
        });
        continue;
      }
      if (decision === "skip") {
        updateTask(task.id, {
          status: "canceled",
          error: "Skipped because target item already exists",
        });
        continue;
      }
      approved.push({
        task,
        request: {
          sessionId,
          remotePath,
          localPath: file.path,
          transferId: task.id,
          keepExisting: decision === "keep",
        },
      });
    }

    if (approved.length === 0) {
      return;
    }
    // Left running in the background, as before, but capped so a large drop
    // does not open one SSH connection per file at once.
    void mapWithConcurrencyLimit(approved, MAX_CONCURRENT_TRANSFERS, ({ task, request }) =>
      runTransfer("sftp_upload_file", task, request),
    ).then(() => {
      // Read what the session shows now, not what it showed at enqueue time:
      // the user may have browsed elsewhere during a long upload. Reload only
      // when the upload landed in that folder (or in a folder row inside it),
      // since a reload also clears the selection.
      const viewed = pathBySessionRef.current.get(sessionId);
      if (
        useSessionStore.getState().activeSessionId === sessionId &&
        viewed &&
        (viewed === targetDirectory || parentRemotePath(targetDirectory) === viewed)
      ) {
        loadDirectory(viewed).catch(() => undefined);
      }
    });
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
    const targetDirectory = localPath.trim();
    const approved: Array<{ task: TransferTask; request: SftpTransferRequest }> = [];
    for (const file of downloadable) {
      const targetLocalPath = joinLocalPath(targetDirectory, file.name);
      const task = createTransferTask(
        "download",
        file.name,
        file.path,
        targetLocalPath,
        file.type === "file" ? file.size : null,
        sessionId,
      );
      addTask(task);
      let decision: OverwriteDecision = "skip";
      try {
        decision = await confirmOverwrite(
          "local_path_exists",
          { path: targetLocalPath },
          targetLocalPath,
          file.type === "directory",
        );
      } catch (error) {
        updateTask(task.id, {
          status: "failed",
          error: String(error),
        });
        continue;
      }
      if (decision === "skip") {
        updateTask(task.id, {
          status: "canceled",
          error: "Skipped because target item already exists",
        });
        continue;
      }
      approved.push({
        task,
        request: {
          sessionId,
          remotePath: file.path,
          localPath: targetLocalPath,
          transferId: task.id,
          keepExisting: decision === "keep",
        },
      });
    }

    if (approved.length === 0) {
      return;
    }
    void mapWithConcurrencyLimit(approved, MAX_CONCURRENT_TRANSFERS, ({ task, request }) =>
      runTransfer("sftp_download_file", task, request),
    ).then(() => {
      // As with uploads, refresh only if the local panel still shows the
      // folder the downloads went to.
      const viewed = loadedLocalDirRef.current;
      if (viewed && viewed === targetDirectory) {
        loadLocalDirectory(viewed).catch(() => undefined);
      }
    });
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
              nativeDropCoordinateScale(
                globalThis.navigator?.userAgent ?? "",
                globalThis.devicePixelRatio || 1,
              ),
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
      const state = pointerDragRef.current;
      if (state) {
        try {
          if (state.captureTarget.hasPointerCapture?.(state.pointerId)) {
            state.captureTarget.releasePointerCapture?.(state.pointerId);
          }
        } catch {
          // The browser may already have released capture while crossing the
          // native window boundary.
        }
      }
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
      const overLocal =
        state.drag.kind === "remote" &&
        isClientPositionInsideElement(
          event.clientX,
          event.clientY,
          localDropZoneRef.current,
        );
      setRemoteDropActive(
        state.drag.kind === "local" &&
          isClientPositionInsideElement(
            event.clientX,
            event.clientY,
            remoteDropZoneRef.current,
          ),
      );
      setLocalDropActive(overLocal);

      if (state.drag.kind !== "remote") {
        return;
      }

      state.externalIntent =
        !overLocal && isNearViewportEdge(event.clientX, event.clientY);
      if (!state.externalIntent || state.externalStarted) {
        return;
      }

      const launchNativeDrag = (paths: string[]) => {
        if (
          pointerDragRef.current !== state ||
          !state.externalIntent ||
          state.externalStarted
        ) {
          return;
        }
        state.externalStarted = true;
        suppressPointerClickRef.current = true;
        globalThis.setTimeout(() => {
          suppressPointerClickRef.current = false;
        }, 0);
        clearPointerDrag();
        setMessage({
          kind: "info",
          text: "Drop the prepared items in Finder, Explorer, or Files",
        });
        void startNativeFileDrag(paths).catch((error) => {
          setMessage({
            kind: "error",
            text: `Failed to start desktop file drag: ${String(error)}`,
          });
        });
      };

      if (state.externalPaths) {
        launchNativeDrag(state.externalPaths);
        return;
      }
      if (state.externalPreparation) {
        return;
      }

      const prepare = prepareRemoteDragOutRef.current;
      if (!prepare) {
        return;
      }
      setMessage({
        kind: "info",
        text: "Preparing remote items for desktop drag. Keep holding while they download.",
      });
      const preparation = prepare(state.drag.files);
      state.externalPreparation = preparation;
      void preparation
        .then((paths) => {
          state.externalPaths = paths;
          if (pointerDragRef.current === state && state.externalIntent) {
            launchNativeDrag(paths);
          } else if (state.externalIntent) {
            setMessage({
              kind: "info",
              text: "Remote items are ready. Drag them outside GpuTerm again.",
            });
          }
        })
        .catch((error) => {
          if (state.externalIntent) {
            setMessage({ kind: "error", text: String(error) });
          }
        });
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

        const remoteDirectory =
          state.drag.kind === "remote"
            ? document
                .elementFromPoint?.(event.clientX, event.clientY)
                ?.closest<HTMLElement>("[data-remote-directory]")?.dataset
                .remoteDirectory
            : undefined;
        if (state.drag.kind === "remote" && remoteDirectory) {
          void moveRemoteFilesRef.current?.(state.drag.files, remoteDirectory);
        } else if (
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
  }, [clearActiveDrag, path, setMessage]);

  const confirmOverwrite = async (
    command: "sftp_path_exists" | "local_path_exists",
    args: Record<string, unknown>,
    targetPath: string,
    isDirectory: boolean,
  ): Promise<OverwriteDecision> => {
    const exists = await invoke<boolean>(command, args);
    if (!exists) {
      return "replace";
    }
    if (!isDirectory) {
      const replace = await confirmDialog(
        `Replace the existing item?\n${targetPath}`,
        {
          title: "Confirm replacement",
          kind: "warning",
          okLabel: "Replace",
          cancelLabel: "Skip",
        },
      );
      return replace ? "replace" : "skip";
    }
    // A folder merges into the existing one; the user picks what happens to
    // files that exist on both sides.
    const labels = {
      yes: "Replace files",
      no: "Keep existing",
      cancel: "Skip",
    };
    const choice = await messageDialog(
      `A folder named "${baseName(targetPath)}" already exists.\n${targetPath}\n\nMerge into it, replacing files that exist in both, or keep the existing files?`,
      { title: "Merge folders", kind: "warning", buttons: labels },
    );
    if (choice === "Yes" || choice === labels.yes) {
      return "replace";
    }
    if (choice === "No" || choice === labels.no) {
      return "keep";
    }
    return "skip";
  };

  const runTransfer = async (
    command: "sftp_upload_file" | "sftp_download_file",
    task: TransferTask,
    request: SftpTransferRequest,
  ): Promise<boolean> => {
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
      return true;
    } catch (error) {
      // A cancel also rejects the command. The `canceled` progress event is
      // emitted before the command returns, so let any queued event land
      // before deciding whether this was a failure.
      await new Promise((resolve) => setTimeout(resolve, 0));
      if (transferWasCanceled(task.id)) {
        return false;
      }
      updateTask(task.id, {
        status: "failed",
        error: String(error),
      });
      setMessage({ kind: "error", text: String(error) });
      return false;
    }
  };

  const prepareRemoteDragOut = (files: RemoteDragFile[]) => {
    const sessionId = activeSessionId;
    if (!sessionId) {
      return Promise.reject(new Error("No active SFTP session"));
    }
    const downloadable = files.filter(
      (file) => file.type === "file" || file.type === "directory",
    );
    if (downloadable.length === 0) {
      return Promise.reject(new Error("No downloadable remote items selected"));
    }

    const cacheKey = JSON.stringify({
      sessionId,
      files: downloadable.map((file) => [file.path, file.type, file.size]),
    });
    const cached = dragOutCacheRef.current.get(cacheKey);
    if (cached) {
      return cached;
    }

    const preparation = invoke<SftpDragOutPath[]>("sftp_create_drag_out_paths", {
      remotePaths: downloadable.map((file) => file.path),
    }).then(async (reservedPaths) => {
      const results = await Promise.all(
        reservedPaths.map(async (reservedPath) => {
          const file = downloadable.find(
            (candidate) => candidate.path === reservedPath.remotePath,
          );
          if (!file) {
            return false;
          }
          const task = createTransferTask(
            "download",
            file.name,
            file.path,
            reservedPath.localPath,
            file.type === "file" ? file.size : null,
            sessionId,
          );
          addTask(task);
          return runTransfer("sftp_download_file", task, {
            sessionId,
            remotePath: file.path,
            localPath: reservedPath.localPath,
            transferId: task.id,
          });
        }),
      );
      if (results.some((succeeded) => !succeeded)) {
        throw new Error("Failed to prepare one or more remote items for desktop drag");
      }
      return reservedPaths.map((reservedPath) => reservedPath.localPath);
    });

    dragOutCacheRef.current.set(cacheKey, preparation);
    void preparation
      .then(() => {
        globalThis.setTimeout(() => {
          if (dragOutCacheRef.current.get(cacheKey) === preparation) {
            dragOutCacheRef.current.delete(cacheKey);
          }
        }, 5 * 60 * 1000);
      })
      .catch(() => {
        if (dragOutCacheRef.current.get(cacheKey) === preparation) {
          dragOutCacheRef.current.delete(cacheKey);
        }
      });
    return preparation;
  };

  prepareRemoteDragOutRef.current = prepareRemoteDragOut;

  // Selection changes go through one updater so the anchor (the last
  // non-Shift click) moves with the paths it extends a range from.
  const updateRemoteSelection = (
    change: (selection: FileSelection) => FileSelection,
  ) => {
    const next = change({
      paths: selectedRemotePaths,
      anchor: remoteAnchorRef.current,
    });
    remoteAnchorRef.current = next.anchor;
    setSelectedRemotePaths(next.paths);
  };

  const updateLocalSelection = (
    change: (selection: FileSelection) => FileSelection,
  ) => {
    const next = change({
      paths: selectedLocalPaths,
      anchor: localAnchorRef.current,
    });
    localAnchorRef.current = next.anchor;
    setSelectedLocalPaths(next.paths);
  };

  const selectRemoteEntry = (
    entry: SftpEntry,
    gesture: SelectionGesture,
    orderedPaths: string[],
  ) => {
    updateRemoteSelection((selection) =>
      applySelectionGesture(selection, orderedPaths, entry.path, gesture),
    );
  };

  const selectLocalEntry = (
    entry: LocalEntry,
    gesture: SelectionGesture,
    orderedPaths: string[],
  ) => {
    updateLocalSelection((selection) =>
      applySelectionGesture(selection, orderedPaths, entry.path, gesture),
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
      captureTarget: event.currentTarget,
      startX: event.clientX,
      startY: event.clientY,
      moved: false,
      drag,
      externalIntent: false,
      externalStarted: false,
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
    targetDirectory: SftpEntry,
    event: DragEvent<HTMLButtonElement>,
  ) => {
    event.preventDefault();
    event.stopPropagation();
    const currentDrag = useTransferStore.getState().activeDrag;
    const files =
      currentDrag?.kind === "remote"
        ? currentDrag.files
        : readRemoteDragFiles(event.dataTransfer);
    if (files.length > 0) {
      clearActiveDrag();
      void moveRemoteFiles(files, targetDirectory.path);
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
          onToggleAll={(orderedPaths) =>
            updateRemoteSelection((selection) => toggleAll(selection, orderedPaths))
          }
          onSelectAll={(orderedPaths) =>
            updateRemoteSelection((selection) => selectAll(selection, orderedPaths))
          }
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
          renamingPath={renamingPath}
          renameDraft={renameDraft}
          onRenameDraftChange={setRenameDraft}
          onConfirmRename={() => void applyRename()}
          onCancelRename={() => setRenamingPath(null)}
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
          onToggleAll={(orderedPaths) =>
            updateLocalSelection((selection) => toggleAll(selection, orderedPaths))
          }
          onSelectAll={(orderedPaths) =>
            updateLocalSelection((selection) => selectAll(selection, orderedPaths))
          }
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
            aria-label="Download"
            title="Download selected remote items"
            disabled={!connected || loading || selectedRemoteEntries.length === 0}
            onClick={downloadSelected}
          >
            <Download size={16} />
            <span className="button-label">
              {countLabel("Download", selectedRemoteEntries.length)}
            </span>
          </button>
          <button
            className="secondary-button"
            type="button"
            aria-label="Upload"
            title="Upload selected local items"
            disabled={!connected || loading || selectedLocalEntries.length === 0}
            onClick={uploadLocalFile}
          >
            <Upload size={16} />
            <span className="button-label">
              {countLabel("Upload", selectedLocalEntries.length)}
            </span>
          </button>
          <button
            className="secondary-button"
            type="button"
            aria-label="Rename"
            title="Rename the selected remote item"
            disabled={!connected || loading || selectedRemoteEntries.length !== 1}
            onClick={beginRename}
          >
            <PenLine size={16} />
            <span className="button-label">Rename</span>
          </button>
          <button
            className="secondary-button danger"
            type="button"
            aria-label="Delete"
            title="Delete selected remote item"
            disabled={!connected || loading || selectedRemoteEntries.length === 0}
            onClick={deleteSelected}
          >
            <Trash2 size={16} />
            <span className="button-label">
              {countLabel("Delete", selectedRemoteEntries.length)}
            </span>
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

/**
 * Wry reports native drop positions in AppKit/GTK logical coordinates, while
 * WebView2 reports physical pixels. Only Windows therefore needs DPI scaling
 * before comparing the native event with a DOMClientRect.
 */
export function nativeDropCoordinateScale(userAgent: string, deviceScale: number) {
  if (!/Windows/i.test(userAgent)) {
    return 1;
  }
  return deviceScale > 0 ? deviceScale : 1;
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

function isNearViewportEdge(clientX: number, clientY: number) {
  const edgeThreshold = 32;
  const width = globalThis.innerWidth || document.documentElement.clientWidth;
  const height = globalThis.innerHeight || document.documentElement.clientHeight;
  return (
    clientX <= edgeThreshold ||
    clientY <= edgeThreshold ||
    clientX >= width - edgeThreshold ||
    clientY >= height - edgeThreshold
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

/** Last path component, for either separator. */
function baseName(path: string) {
  const trimmed = path.replace(/[\\/]+$/, "");
  return trimmed.slice(Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\")) + 1);
}

/** Remote parent folder, POSIX-style. */
function transferWasCanceled(taskId: string) {
  return (
    useTransferStore.getState().tasks.find((task) => task.id === taskId)?.status ===
    "canceled"
  );
}

function parentRemotePath(remotePath: string) {
  const trimmed = remotePath.replace(/\/+$/, "");
  const index = trimmed.lastIndexOf("/");
  if (index < 0) {
    return ".";
  }
  return index === 0 ? "/" : trimmed.slice(0, index);
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
