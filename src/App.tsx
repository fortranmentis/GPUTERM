import { useEffect } from "react";
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

function App() {
  const message = useSessionStore((state) => state.message);
  const setSessions = useSessionStore((state) => state.setSessions);
  const setMessage = useSessionStore((state) => state.setMessage);
  const setConnected = useSessionStore((state) => state.setConnected);
  const setRemoteTelemetry = useSessionStore((state) => state.setRemoteTelemetry);
  const setTelemetrySettings = useSessionStore((state) => state.setTelemetrySettings);

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
        <div className="workspace-grid">
          <section className="terminal-region">
            <TerminalPane />
          </section>
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
