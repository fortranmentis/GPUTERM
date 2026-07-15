import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal, type IDisposable } from "@xterm/xterm";
import { Terminal as TerminalIcon, X } from "lucide-react";
import { selectIsActiveConnected, useSessionStore } from "../stores/sessionStore";
import { useDisconnectSession } from "../hooks/useDisconnectSession";
import {
  appendPendingOutput,
  clearPendingOutput,
  takePendingOutput,
} from "../utils/terminalBuffer";

type TerminalOutputPayload = {
  sessionId: string;
  data: string;
};

type TerminalClosedPayload = {
  sessionId: string;
};

type TermEntry = {
  terminal: Terminal;
  fitAddon: FitAddon;
  onData: IDisposable;
};

function createTerminal(): Terminal {
  return new Terminal({
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
}

export function TerminalPane() {
  const activeSessionId = useSessionStore((state) => state.activeSessionId);
  const connectedSessionIds = useSessionStore((state) => state.connectedSessionIds);
  const isActiveConnected = useSessionStore(selectIsActiveConnected);
  const sessions = useSessionStore((state) => state.sessions);
  const setMessage = useSessionStore((state) => state.setMessage);

  // One xterm instance per session (created lazily once its host div exists).
  const instancesRef = useRef<Map<string, TermEntry>>(new Map());
  const hostRefs = useRef<Map<string, HTMLDivElement>>(new Map());
  const [hostIds, setHostIds] = useState<string[]>([]);
  const prevConnectedRef = useRef<string[]>([]);
  const activeSessionRef = useRef<string | null>(activeSessionId);
  const fitTimerRef = useRef<number | null>(null);
  const pendingOutputRef = useRef<Map<string, string>>(new Map());

  const activeProfile =
    sessions.find((session) => session.id === activeSessionId) ?? null;

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
    const sessionId = activeSessionRef.current;
    if (!sessionId) {
      return;
    }
    const entry = instancesRef.current.get(sessionId);
    if (!entry) {
      return;
    }
    try {
      entry.fitAddon.fit();
      if (useSessionStore.getState().connectedSessionIds.includes(sessionId)) {
        invoke("terminal_resize", {
          sessionId,
          cols: entry.terminal.cols,
          rows: entry.terminal.rows,
        }).catch(() => undefined);
      }
    } catch {
      // xterm can throw while its container is being mounted or resized to zero.
    }
  };

  // Keep one host per live session. A dead *active* session keeps its host so
  // the last output stays readable; dead background sessions are dropped.
  useEffect(() => {
    activeSessionRef.current = activeSessionId;
    setHostIds((previous) => {
      const keep = previous.filter(
        (id) => connectedSessionIds.includes(id) || id === activeSessionId,
      );
      const added = connectedSessionIds.filter((id) => !keep.includes(id));
      return added.length > 0 || keep.length !== previous.length
        ? [...keep, ...added]
        : previous;
    });
  }, [activeSessionId, connectedSessionIds]);

  // Create terminals for new hosts, dispose terminals whose host went away.
  useEffect(() => {
    for (const id of hostIds) {
      const host = hostRefs.current.get(id);
      if (!host || instancesRef.current.has(id)) {
        continue;
      }
      const terminal = createTerminal();
      const fitAddon = new FitAddon();
      terminal.loadAddon(fitAddon);
      terminal.open(host);
      const onData = terminal.onData((data) => {
        if (!useSessionStore.getState().connectedSessionIds.includes(id)) {
          return;
        }
        invoke("terminal_write", { sessionId: id, data }).catch((error) => {
          setMessage({ kind: "error", text: String(error) });
        });
      });
      instancesRef.current.set(id, { terminal, fitAddon, onData });

      const pending = takePendingOutput(pendingOutputRef.current, id);
      if (pending) {
        terminal.write(pending);
      }
      if (id === activeSessionRef.current) {
        scheduleFit(60);
        terminal.focus();
      }
    }

    for (const [id, entry] of [...instancesRef.current]) {
      if (!hostIds.includes(id)) {
        entry.onData.dispose();
        entry.terminal.dispose();
        instancesRef.current.delete(id);
      }
    }
  }, [hostIds, setMessage]);

  // Reconnecting the same profile replaces the backend channel: reset the
  // existing terminal and replay any output captured during the handshake.
  useEffect(() => {
    const previous = prevConnectedRef.current;
    prevConnectedRef.current = connectedSessionIds;
    for (const id of connectedSessionIds) {
      if (previous.includes(id)) {
        continue;
      }
      const entry = instancesRef.current.get(id);
      if (!entry) {
        continue; // freshly created hosts flush their buffer on creation
      }
      entry.terminal.reset();
      const pending = takePendingOutput(pendingOutputRef.current, id);
      if (pending) {
        entry.terminal.write(pending);
      }
      if (id === activeSessionRef.current) {
        scheduleFit(60);
        entry.terminal.focus();
      }
    }
  }, [connectedSessionIds]);

  // Switching the visible session: refit and focus it.
  useEffect(() => {
    const entry = activeSessionId
      ? instancesRef.current.get(activeSessionId)
      : undefined;
    if (entry) {
      scheduleFit(60);
      entry.terminal.focus();
    }
  }, [activeSessionId]);

  const surfaceRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const surface = surfaceRef.current;
    if (!surface) {
      return;
    }
    const resizeObserver = new ResizeObserver(() => {
      scheduleFit(20);
    });
    resizeObserver.observe(surface);

    return () => {
      if (fitTimerRef.current != null) {
        window.clearTimeout(fitTimerRef.current);
        fitTimerRef.current = null;
      }
      resizeObserver.disconnect();
      for (const entry of instancesRef.current.values()) {
        entry.onData.dispose();
        entry.terminal.dispose();
      }
      instancesRef.current.clear();
    };
  }, []);

  useEffect(() => {
    let disposed = false;
    const unlisteners: Array<() => void> = [];
    const register = (promise: Promise<() => void>) => {
      promise.then((unlisten) => {
        if (disposed) {
          unlisten();
        } else {
          unlisteners.push(unlisten);
        }
      });
    };

    register(
      listen<TerminalOutputPayload>("terminal-output", (event) => {
        const { sessionId, data } = event.payload;
        const entry = instancesRef.current.get(sessionId);
        if (entry) {
          entry.terminal.write(data);
        } else {
          appendPendingOutput(pendingOutputRef.current, sessionId, data);
        }
      }),
    );

    register(
      listen<TerminalClosedPayload>("terminal-closed", (event) => {
        // Drop buffered tail output (e.g. "logout") so it does not replay on
        // the next connect to the same profile.
        clearPendingOutput(pendingOutputRef.current, event.payload.sessionId);
      }),
    );

    return () => {
      disposed = true;
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, []);

  const disconnect = useDisconnectSession();

  return (
    <div className="terminal-pane">
      <div className="terminal-tabs">
        <div className={`terminal-tab ${isActiveConnected ? "online" : ""}`}>
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
          disabled={!isActiveConnected}
          aria-label="Disconnect terminal"
          title="Disconnect"
          onClick={() => disconnect()}
        >
          <X size={16} />
        </button>
      </div>
      <div className="terminal-surface" ref={surfaceRef}>
        {hostIds.length === 0 && (
          <div className="terminal-empty">No active SSH session</div>
        )}
        {hostIds.map((id) => (
          <div
            key={id}
            className={`xterm-host ${id === activeSessionId ? "" : "xterm-host-hidden"}`}
            ref={(element) => {
              if (element) {
                hostRefs.current.set(id, element);
              } else {
                hostRefs.current.delete(id);
              }
            }}
          />
        ))}
      </div>
    </div>
  );
}
