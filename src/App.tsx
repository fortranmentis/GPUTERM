import { useEffect, useRef, useState, type MouseEvent as ReactMouseEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getAllWebviewWindows } from "@tauri-apps/api/webviewWindow";
import { PanelLeftOpen } from "lucide-react";
import { AppMessageOverlay } from "./components/AppMessage";
import { RemoteTelemetryBar } from "./components/RemoteTelemetryBar";
import { SessionSidebar } from "./components/SessionSidebar";
import { SftpBrowser } from "./components/SftpBrowser";
import { TerminalPane } from "./components/TerminalPane";
import { useSessionStore } from "./stores/sessionStore";
import type { RemoteTelemetry, TelemetrySettings } from "./types/gpu";
import type { SessionProfile, SftpProgressPayload } from "./types/session";

type TerminalClosedPayload = {
  sessionId: string;
  terminalId?: string;
  sessionClosed?: boolean;
  message?: string | null;
};

const SFTP_WIDTH_STORAGE_KEY = "gputerm.sftpWidth";
const MIN_SFTP_WIDTH = 280;
const DEFAULT_SFTP_WIDTH = 400;
const SIDEBAR_OPEN_STORAGE_KEY = "gputerm.sidebarOpen";

function clampSftpWidth(value: number) {
  const max = Math.max(MIN_SFTP_WIDTH, Math.round(window.innerWidth * 0.6));
  return Math.min(Math.max(Math.round(value), MIN_SFTP_WIDTH), max);
}

function initialSftpWidth() {
  const stored = Number(localStorage.getItem(SFTP_WIDTH_STORAGE_KEY));
  return clampSftpWidth(Number.isFinite(stored) && stored > 0 ? stored : DEFAULT_SFTP_WIDTH);
}

function App() {
  const setSessions = useSessionStore((state) => state.setSessions);
  const setMessage = useSessionStore((state) => state.setMessage);
  const removeTerminalPane = useSessionStore((state) => state.removeTerminalPane);
  const showSession = useSessionStore((state) => state.showSession);
  const setSessionTelemetry = useSessionStore((state) => state.setSessionTelemetry);
  const setTelemetrySettings = useSessionStore((state) => state.setTelemetrySettings);
  const [sftpWidth, setSftpWidth] = useState(initialSftpWidth);
  const [sidebarOpen, setSidebarOpen] = useState(
    () => localStorage.getItem(SIDEBAR_OPEN_STORAGE_KEY) !== "false",
  );
  const workspaceGridRef = useRef<HTMLDivElement | null>(null);

  const startSplitterDrag = (event: ReactMouseEvent<HTMLDivElement>) => {
    event.preventDefault();
    const grid = workspaceGridRef.current;
    if (!grid) {
      return;
    }
    const gridRight = grid.getBoundingClientRect().right;
    const previousUserSelect = document.body.style.userSelect;
    const previousCursor = document.body.style.cursor;
    document.body.style.userSelect = "none";
    document.body.style.cursor = "col-resize";

    let latestWidth = sftpWidth;
    const handleMove = (moveEvent: MouseEvent) => {
      latestWidth = clampSftpWidth(gridRight - moveEvent.clientX - 3);
      setSftpWidth(latestWidth);
    };
    const handleUp = () => {
      window.removeEventListener("mousemove", handleMove);
      window.removeEventListener("mouseup", handleUp);
      document.body.style.userSelect = previousUserSelect;
      document.body.style.cursor = previousCursor;
      localStorage.setItem(SFTP_WIDTH_STORAGE_KEY, String(latestWidth));
    };
    window.addEventListener("mousemove", handleMove);
    window.addEventListener("mouseup", handleUp);
  };

  useEffect(() => {
    invoke<SessionProfile[]>("load_sessions")
      .then(setSessions)
      .catch((error) =>
        setMessage({ kind: "error", text: String(error) }),
      );
    invoke<TelemetrySettings>("get_telemetry_settings")
      .then(setTelemetrySettings)
      .catch(() => undefined);
  }, [setMessage, setSessions, setTelemetrySettings]);

  useEffect(() => {
    // Closing the main window must take the detached detail windows with it;
    // otherwise the process keeps running until they are closed by hand.
    // Registering this listener disables Tauri's automatic close — the API
    // wrapper calls destroy() afterwards (needs core:window:allow-destroy in
    // the capability), and a handler that throws would skip that destroy and
    // leave the window unclosable, so failures here must be swallowed.
    const unlistenPromise = getCurrentWindow().onCloseRequested(async () => {
      try {
        const windows = await getAllWebviewWindows();
        await Promise.all(
          windows
            .filter((webviewWindow) => webviewWindow.label.startsWith("detail-"))
            .map((webviewWindow) => webviewWindow.close().catch(() => undefined)),
        );
      } catch {
        // Never block the main window's close over the detail windows.
      }
    });
    return () => {
      unlistenPromise.then((unlisten) => unlisten()).catch(() => undefined);
    };
  }, []);

  useEffect(() => {
    let disposed = false;
    const unlisteners: Array<() => void> = [];

    listen<RemoteTelemetry>("remote-telemetry", (event) => {
      setSessionTelemetry(event.payload.sessionId, event.payload);
    }).then((unlisten) => {
      if (disposed) {
        unlisten();
      } else {
        unlisteners.push(unlisten);
      }
    });

    listen<TerminalClosedPayload>("terminal-closed", (event) => {
      const state = useSessionStore.getState();
      const terminalId = event.payload.terminalId ?? event.payload.sessionId;
      const panes = state.terminalIdsBySession[event.payload.sessionId] ?? [];
      // A replaced connection can finish shutting down after its successor is
      // already live. Its unique terminal id keeps that stale close event from
      // disconnecting the new session.
      if (!panes.includes(terminalId)) {
        return;
      }
      const remainingPanes = panes.filter((id) => id !== terminalId);
      removeTerminalPane(event.payload.sessionId, terminalId);
      const sessionClosed =
        event.payload.sessionClosed === true || remainingPanes.length === 0;
      if (sessionClosed && event.payload.sessionId === state.activeSessionId) {
        const remainingSessions = state.connectedSessionIds.filter(
          (id) => id !== event.payload.sessionId,
        );
        showSession(remainingSessions[remainingSessions.length - 1] ?? null);
      }
      if (event.payload.message) {
        const isActive = event.payload.sessionId === state.activeSessionId;
        const profileName = state.sessions.find(
          (session) => session.id === event.payload.sessionId,
        )?.name;
        const prefix = !isActive && profileName ? `${profileName}: ` : "";
        const paneSuffix = sessionClosed ? "" : " (terminal pane)";
        setMessage({
          kind: "info",
          text: `${prefix}${event.payload.message}${paneSuffix}`,
        });
      }
    }).then((unlisten) => {
      if (disposed) {
        unlisten();
      } else {
        unlisteners.push(unlisten);
      }
    });

    listen<SftpProgressPayload>("sftp-progress", (event) => {
      const progress = event.payload;
      if (!progress.done) {
        return;
      }
      if (progress.error) {
        setMessage({ kind: "error", text: progress.error });
      } else {
        setMessage({
          kind: "success",
          text: `${progress.operation} complete: ${progress.remotePath}`,
        });
      }
    }).then((unlisten) => {
      if (disposed) {
        unlisten();
      } else {
        unlisteners.push(unlisten);
      }
    });

    return () => {
      disposed = true;
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, [
    removeTerminalPane,
    showSession,
    setMessage,
    setSessionTelemetry,
  ]);

  const setHostSelectorOpen = (open: boolean) => {
    setSidebarOpen(open);
    localStorage.setItem(SIDEBAR_OPEN_STORAGE_KEY, String(open));
    window.setTimeout(() => window.dispatchEvent(new Event("resize")), 0);
  };

  return (
    <div className="app-shell">
      {sidebarOpen && (
        <SessionSidebar onClose={() => setHostSelectorOpen(false)} />
      )}
      <main className={`workspace ${sidebarOpen ? "" : "sidebar-closed"}`}>
        {!sidebarOpen && (
          <button
            className="icon-button host-selector-open"
            type="button"
            aria-label="Open host selector"
            title="Open host selector"
            onClick={() => setHostSelectorOpen(true)}
          >
            <PanelLeftOpen size={18} />
          </button>
        )}
        <AppMessageOverlay />
        <div
          className="workspace-grid"
          ref={workspaceGridRef}
          style={{
            gridTemplateColumns: `minmax(0, 1fr) 6px min(${sftpWidth}px, 60%)`,
          }}
        >
          <section className="terminal-region">
            <TerminalPane />
          </section>
          <div
            className="workspace-splitter"
            role="separator"
            aria-orientation="vertical"
            aria-label="Resize SFTP panel"
            onMouseDown={startSplitterDrag}
          />
          <section className="sftp-region">
            <SftpBrowser />
          </section>
        </div>
        <RemoteTelemetryBar />
      </main>
    </div>
  );
}

export default App;
