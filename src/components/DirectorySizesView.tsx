import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  AlertTriangle,
  ArrowLeft,
  ChevronRight,
  FolderOpen,
  LoaderCircle,
  RefreshCw,
} from "lucide-react";
import type { DirectorySizes } from "../types/dirSizes";
import { formatFileSize } from "../utils/formatBytes";

type CacheEntry = { sizes: DirectorySizes; at: number };

export const DIRECTORY_SIZE_CACHE_TTL_MS = 5 * 60 * 1000;
const MAX_CACHE_ENTRIES = 100;

/**
 * Scans already made, per session and folder, so stepping back up does not
 * run `du` again. Bounded by TTL and entry cap, and cleared on disconnect.
 */
const sizeCache = new Map<string, CacheEntry>();

function getCachedEntry(key: string): CacheEntry | null {
  const cached = sizeCache.get(key);
  if (!cached) return null;
  if (Date.now() - cached.at > DIRECTORY_SIZE_CACHE_TTL_MS) {
    sizeCache.delete(key);
    return null;
  }
  return cached;
}

function setCachedEntry(key: string, entry: CacheEntry) {
  sizeCache.delete(key);
  sizeCache.set(key, entry);
  while (sizeCache.size > MAX_CACHE_ENTRIES) {
    const oldestKey = sizeCache.keys().next().value;
    if (oldestKey === undefined) break;
    sizeCache.delete(oldestKey);
  }
}

export function clearDirectorySizeCache(sessionId?: string) {
  if (!sessionId) {
    sizeCache.clear();
    return;
  }
  const prefix = `${sessionId}:`;
  for (const key of [...sizeCache.keys()]) {
    if (key.startsWith(prefix)) {
      sizeCache.delete(key);
    }
  }
}

function parentOf(path: string, root: string) {
  if (path === root) return null;
  const parent = path.replace(/\/[^/]+\/?$/, "") || "/";
  return parent.length < root.length ? root : parent;
}

function formatAge(at: number) {
  const seconds = Math.round((Date.now() - at) / 1000);
  if (seconds < 60) return "just now";
  const minutes = Math.round(seconds / 60);
  return minutes < 60 ? `${minutes} min ago` : `${Math.round(minutes / 60)} h ago`;
}

/**
 * The folders inside a mount, largest first, one level at a time: click a
 * folder to go into it. Sizes come from `du` on the host, run on demand.
 */
export function DirectorySizesView({
  sessionId,
  root,
  onBack,
  onOpenInSftp,
}: {
  sessionId: string;
  /** The mount point the user picked; navigation never goes above it. */
  root: string;
  onBack: () => void;
  /** Shows a folder in the SFTP browser; omitted where there is none. */
  onOpenInSftp?: (path: string) => void;
}) {
  const [path, setPath] = useState(root);
  const [entry, setEntry] = useState<CacheEntry | null>(() =>
    getCachedEntry(`${sessionId}:${root}`),
  );
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Only the latest request may update the view: going into a folder or
  // leaving while a scan runs drops the older reply.
  const requestRef = useRef(0);

  const load = useCallback(
    async (target: string, force: boolean) => {
      const key = `${sessionId}:${target}`;
      const cached = getCachedEntry(key);
      const request = ++requestRef.current;
      setPath(target);
      setError(null);
      if (cached && !force) {
        setEntry(cached);
        setLoading(false);
        return;
      }
      setEntry(cached ?? null);
      setLoading(true);
      try {
        const sizes = await invoke<DirectorySizes>("directory_sizes", { sessionId, path: target });
        if (request !== requestRef.current) return;
        const next = { sizes, at: Date.now() };
        setCachedEntry(key, next);
        setEntry(next);
      } catch (loadError) {
        if (request === requestRef.current) setError(String(loadError));
      } finally {
        if (request === requestRef.current) setLoading(false);
      }
    },
    [sessionId],
  );

  const handleCancel = useCallback(() => {
    requestRef.current += 1;
    setLoading(false);
    void invoke("cancel_directory_sizes", { sessionId }).catch(() => {
      // Best-effort cancellation; backend scan may have already finished.
    });
  }, [sessionId]);

  useEffect(() => {
    void load(root, false);
    return () => {
      requestRef.current += 1;
    };
  }, [load, root]);

  const sizes = entry?.sizes;
  const largest = Math.max(1, ...(sizes?.entries.map((item) => item.bytes) ?? [1]));
  const crumbs = path === root ? [root] : [root, ...path.slice(root.length).split("/").filter(Boolean)];
  const up = parentOf(path, root);

  return (
    <div className="dir-sizes">
      <div className="dir-sizes-toolbar">
        <button
          className="icon-button ghost"
          type="button"
          aria-label={up ? "Up one folder" : "Back to disks"}
          title={up ? "Up one folder" : "Back to disks"}
          onClick={() => (up ? void load(up, false) : onBack())}
        >
          <ArrowLeft size={14} />
        </button>
        <nav className="dir-sizes-crumbs" aria-label="Folder path">
          {crumbs.map((crumb, index) => {
            const target =
              index === 0 ? root : `${root.replace(/\/$/, "")}/${crumbs.slice(1, index + 1).join("/")}`;
            const last = index === crumbs.length - 1;
            return (
              <span key={target}>
                {index > 0 && <ChevronRight size={11} aria-hidden="true" />}
                {last ? (
                  <strong title={target}>{crumb}</strong>
                ) : (
                  <button type="button" title={target} onClick={() => void load(target, false)}>
                    {crumb}
                  </button>
                )}
              </span>
            );
          })}
        </nav>
        {onOpenInSftp && (
          <button
            className="icon-button ghost"
            type="button"
            aria-label="Open this folder in SFTP"
            title="Open in SFTP"
            onClick={() => onOpenInSftp(path)}
          >
            <FolderOpen size={14} />
          </button>
        )}
        <button
          className="icon-button ghost"
          type="button"
          aria-label="Rescan this folder"
          title="Rescan"
          disabled={loading}
          onClick={() => void load(path, true)}
        >
          <RefreshCw size={14} className={loading ? "spin" : undefined} />
        </button>
      </div>

      <div className="dir-sizes-status">
        {loading ? (
          <>
            <span>
              <LoaderCircle size={12} className="spin" /> Measuring folders… large trees can take up
              to 30 seconds.
            </span>
            <button className="ghost-button compact" type="button" onClick={handleCancel}>
              Cancel
            </button>
          </>
        ) : sizes ? (
          <span>
            {sizes.totalBytes != null ? `${formatFileSize(sizes.totalBytes)} in total · ` : ""}
            measured {entry ? formatAge(entry.at) : ""}
          </span>
        ) : null}
      </div>

      {error && (
        <div className="dir-sizes-warning error" role="alert">
          {error}
        </div>
      )}
      {sizes && !sizes.complete && (
        <div className="dir-sizes-warning">
          <AlertTriangle size={12} /> Stopped after the time limit: sizes are lower bounds and some
          folders may be missing.
        </div>
      )}
      {sizes && sizes.unreadable > 0 && (
        <div className="dir-sizes-warning">
          <AlertTriangle size={12} /> {sizes.unreadable} path{sizes.unreadable === 1 ? "" : "s"}{" "}
          could not be read (permissions), so their sizes are left out.
        </div>
      )}

      {sizes && (
        <div className="dir-sizes-list">
          {sizes.entries.length === 0 && !loading && (
            <div className="empty-list compact">No folders here</div>
          )}
          {sizes.entries.map((item) => (
            <button
              className="dir-sizes-row"
              type="button"
              key={item.path}
              title={item.path}
              onClick={() => void load(item.path, false)}
            >
              <span className="dir-sizes-name">{item.name}</span>
              <span className="dir-sizes-bar" aria-hidden="true">
                <span style={{ width: `${Math.max(1, (item.bytes / largest) * 100)}%` }} />
              </span>
              <span className="dir-sizes-bytes">{formatFileSize(item.bytes)}</span>
              <ChevronRight size={12} aria-hidden="true" />
            </button>
          ))}
          {sizes.filesBytes != null && sizes.filesBytes > 0 && (
            <div className="dir-sizes-row static">
              <span className="dir-sizes-name muted">Files in this folder</span>
              <span className="dir-sizes-bar" aria-hidden="true">
                <span style={{ width: `${Math.min(100, (sizes.filesBytes / largest) * 100)}%` }} />
              </span>
              <span className="dir-sizes-bytes">{formatFileSize(sizes.filesBytes)}</span>
              <span />
            </div>
          )}
          {sizes.moreEntries > 0 && (
            <div className="dir-sizes-more">and {sizes.moreEntries} smaller folders</div>
          )}
        </div>
      )}
    </div>
  );
}
