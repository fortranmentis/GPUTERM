import { useEffect, useRef, useState, type MouseEvent as ReactMouseEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { AlertCircle, CheckCircle2, Info, X } from "lucide-react";
import { RemoteTelemetryBar } from "./components/RemoteTelemetryBar";
import { SessionSidebar } from "./components/SessionSidebar";
import { SftpBrowser } from "./components/SftpBrowser";
import { TerminalPane } from "./components/TerminalPane";
import { useSessionStore } from "./stores/sessionStore";
import type { RemoteTelemetry, TelemetrySettings } from "./types/gpu";
import type { SessionProfile, SftpProgressPayload } from "./types/session";

type TerminalClosedPayload = {
  sessionId: string;
  message?: string | null;
};

const SFTP_WIDTH_STORAGE_KEY = "gputerm.sftpWidth";
const MIN_SFTP_WIDTH = 280;
const DEFAULT_SFTP_WIDTH = 400;

function clampSftpWidth(value: number) {
  const max = Math.max(MIN_SFTP_WIDTH, Math.round(window.innerWidth * 0.6));
  return Math.min(Math.max(Math.round(value), MIN_SFTP_WIDTH), max);
}

function initialSftpWidth() {
  const stored = Number(localStorage.getItem(SFTP_WIDTH_STORAGE_KEY));
  return clampSftpWidth(Number.isFinite(stored) && stored > 0 ? stored : DEFAULT_SFTP_WIDTH);
}

function App() {
  const message = useSessionStore((state) => state.message);
  const setSessions = useSessionStore((state) => state.setSessions);
  const setMessage = useSessionStore((state) => state.setMessage);
  const setConnected = useSessionStore((state) => state.setConnected);
  const setRemoteTelemetry = useSessionStore((state) => state.setRemoteTelemetry);
  const setTelemetrySettings = useSessionStore((state) => state.setTelemetrySettings);
  const [sftpWidth, setSftpWidth] = useState(initialSftpWidth);
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
    let disposed = false;
    const unlisteners: Array<() => void> = [];

    listen<RemoteTelemetry>("remote-telemetry", (event) => {
      setRemoteTelemetry(event.payload);
    }).then((unlisten) => {
      if (disposed) {
        unlisten();
      } else {
        unlisteners.push(unlisten);
      }
    });

    listen<TerminalClosedPayload>("terminal-closed", (event) => {
      const activeSessionId = useSessionStore.getState().activeSessionId;
      if (event.payload.sessionId === activeSessionId) {
        setConnected(false);
        setRemoteTelemetry(null);
        if (event.payload.message) {
          setMessage({ kind: "info", text: event.payload.message });
        }
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
  }, [setConnected, setMessage, setRemoteTelemetry]);

  const messageIcon =
    message?.kind === "error" ? (
      <AlertCircle size={16} />
    ) : message?.kind === "success" ? (
      <CheckCircle2 size={16} />
    ) : (
      <Info size={16} />
    );

  return (
    <div className="app-shell">
      <SessionSidebar />
      <main className="workspace">
        {message && (
          <div className={`app-message ${message.kind}`}>
            {messageIcon}
            <span>{message.text}</span>
            <button
              className="icon-button ghost"
              type="button"
              aria-label="Dismiss message"
              title="Dismiss"
              onClick={() => setMessage(null)}
            >
              <X size={16} />
            </button>
          </div>
        )}
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
