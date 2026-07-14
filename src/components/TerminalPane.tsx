import { useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import { Terminal as TerminalIcon, X } from "lucide-react";
import { useSessionStore } from "../stores/sessionStore";
import { useDisconnectSession } from "../hooks/useDisconnectSession";

type TerminalOutputPayload = {
  sessionId: string;
  data: string;
};

export function TerminalPane() {
  const activeSessionId = useSessionStore((state) => state.activeSessionId);
  const connected = useSessionStore((state) => state.connected);
  const sessions = useSessionStore((state) => state.sessions);
  const setMessage = useSessionStore((state) => state.setMessage);
  const terminalHostRef = useRef<HTMLDivElement | null>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const activeSessionRef = useRef<string | null>(activeSessionId);
  const connectedRef = useRef(connected);
  const lastSessionRef = useRef<string | null>(activeSessionId);
  const wasConnectedRef = useRef(connected);
  const fitTimerRef = useRef<number | null>(null);

  const activeProfile =
    sessions.find((session) => session.id === activeSessionId) ?? null;

  useEffect(() => {
    const sessionChanged = activeSessionId !== lastSessionRef.current;
    const reconnected = connected && !wasConnectedRef.current;
    lastSessionRef.current = activeSessionId;
    wasConnectedRef.current = connected;
    activeSessionRef.current = activeSessionId;
    connectedRef.current = connected;
    if (terminalRef.current && (sessionChanged || reconnected)) {
      terminalRef.current.reset();
    }
    if (terminalRef.current && connected) {
      scheduleFit(60);
      terminalRef.current.focus();
    }
  }, [activeSessionId, connected]);

  useEffect(() => {
    if (!terminalHostRef.current) {
      return;
    }

    const terminal = new Terminal({
      cursorBlink: true,
      convertEol: true,
      fontFamily:
        "Cascadia Mono, JetBrains Mono, SFMono-Regular, Consolas, monospace",
      fontSize: 13,
      lineHeight: 1.18,
      scrollback: 8000,
      theme: {
        background: "#080b10",
        foreground: "#d6dde7",
        cursor: "#67e8f9",
        cursorAccent: "#071014",
        selectionBackground: "#1e3a46",
        black: "#111827",
        blue: "#60a5fa",
        brightBlack: "#4b5563",
        brightBlue: "#93c5fd",
        brightCyan: "#67e8f9",
        brightGreen: "#86efac",
        brightMagenta: "#f0abfc",
        brightRed: "#fca5a5",
        brightWhite: "#ffffff",
        brightYellow: "#fde68a",
        cyan: "#22d3ee",
        green: "#34d399",
        magenta: "#e879f9",
        red: "#f87171",
        white: "#e5e7eb",
        yellow: "#fbbf24",
      },
    });
    const fitAddon = new FitAddon();
    terminal.loadAddon(fitAddon);
    terminal.open(terminalHostRef.current);
    terminalRef.current = terminal;
    fitAddonRef.current = fitAddon;

    const dataDisposable = terminal.onData((data) => {
      const sessionId = activeSessionRef.current;
      if (!sessionId || !connectedRef.current) {
        return;
      }
      invoke("terminal_write", { sessionId, data }).catch((error) => {
        setMessage({ kind: "error", text: String(error) });
      });
    });

    const resizeObserver = new ResizeObserver(() => {
      scheduleFit(20);
    });
    resizeObserver.observe(terminalHostRef.current);
    scheduleFit(60);

    return () => {
      if (fitTimerRef.current != null) {
        window.clearTimeout(fitTimerRef.current);
        fitTimerRef.current = null;
      }
      dataDisposable.dispose();
      resizeObserver.disconnect();
      terminal.dispose();
      terminalRef.current = null;
      fitAddonRef.current = null;
    };
  }, [setMessage]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;

    listen<TerminalOutputPayload>("terminal-output", (event) => {
      if (event.payload.sessionId === activeSessionRef.current) {
        terminalRef.current?.write(event.payload.data);
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
  }, []);

  const scheduleFit = (delayMs: number) => {
    if (fitTimerRef.current != null) {
      window.clearTimeout(fitTimerRef.current);
    }
    fitTimerRef.current = window.setTimeout(() => {
      fitTimerRef.current = null;
      fitAndResize();
    }, delayMs);
  };

  const fitAndResize = () => {
    const terminal = terminalRef.current;
    const fitAddon = fitAddonRef.current;
    const sessionId = activeSessionRef.current;
    if (!terminal || !fitAddon) {
      return;
    }
    try {
      fitAddon.fit();
      if (sessionId && connectedRef.current) {
        invoke("terminal_resize", {
          sessionId,
          cols: terminal.cols,
          rows: terminal.rows,
        }).catch(() => undefined);
      }
    } catch {
      // xterm can throw while its container is being mounted or resized to zero.
    }
  };

  const disconnect = useDisconnectSession();

  return (
    <div className="terminal-pane">
      <div className="terminal-tabs">
        <div className={`terminal-tab ${connected ? "online" : ""}`}>
          <TerminalIcon size={16} />
          <span>
            {activeProfile
              ? `${activeProfile.name} (${activeProfile.host})`
              : "Terminal"}
          </span>
        </div>
        <button
          className="icon-button"
          type="button"
          disabled={!connected}
          aria-label="Disconnect terminal"
          title="Disconnect"
          onClick={disconnect}
        >
          <X size={16} />
        </button>
      </div>
      <div className="terminal-surface">
        {!connected && <div className="terminal-empty">No active SSH session</div>}
        <div ref={terminalHostRef} className="xterm-host" />
      </div>
    </div>
  );
}
