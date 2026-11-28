import type { LocalEntry, SftpEntry } from "../types/session";
import type {
  LocalTransferDragFile,
  RemoteTransferDragFile,
  TransferDirection,
  TransferTask,
} from "../types/transfer";

export function clampStorageSplit(value: number) {
  return Math.min(75, Math.max(25, value));
}

export function createTransferId() {
  return globalThis.crypto?.randomUUID?.() ?? `transfer-${Date.now()}-${Math.random()}`;
}

export function createTransferTask(
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

export function localEntryToDragFile(entry: LocalEntry): LocalTransferDragFile {
  return {
    name: entry.name,
    path: entry.path,
    entryType: entry.entryType,
    size: entry.size,
  };
}

export function remoteEntryToDragFile(entry: SftpEntry): RemoteTransferDragFile {
  return {
    name: entry.name,
    path: entry.path,
    type: entry.type,
    size: entry.size,
  };
}

export function safeParseDragFiles<T>(encoded: string): T[] {
  try {
    const parsed = JSON.parse(encoded);
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
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

export function isClientPositionInsideElement(
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

export function isPhysicalPositionInsideElement(
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

export function isNearViewportEdge(clientX: number, clientY: number) {
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

/** Last path component, for either separator. */
export function baseName(path: string) {
  const trimmed = path.replace(/[\\/]+$/, "");
  return trimmed.slice(Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\")) + 1);
}

/** Remote parent folder, POSIX-style. */
export function parentRemotePath(remotePath: string) {
  const trimmed = remotePath.replace(/\/+$/, "");
  const index = trimmed.lastIndexOf("/");
  if (index < 0) {
    return ".";
  }
  return index === 0 ? "/" : trimmed.slice(0, index);
}

export function joinRemotePath(base: string, name: string) {
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
