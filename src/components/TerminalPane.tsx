import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type FormEvent,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal, type IDisposable } from "@xterm/xterm";
import {
  ArrowLeft,
  Columns2,
  LoaderCircle,
  PanelBottom,
  PanelLeft,
  PanelRight,
  PanelTop,
  Plus,
  Server,
  Terminal as TerminalIcon,
  X,
} from "lucide-react";
import { useSessionStore } from "../stores/sessionStore";
import { useDisconnectSession } from "../hooks/useDisconnectSession";
import {
  appendPendingOutput,
  clearPendingOutput,
  takePendingOutput,
} from "../utils/terminalBuffer";
import { withHostKeyPrompt } from "../utils/hostKeyPrompt";
import { attachWebKitHangulImeWorkaround } from "../utils/xtermHangulIme";
import {
  createTerminalLayout,
  getTerminalLayoutPaneIds,
  insertTerminalPane,
  reconcileTerminalLayout,
  removeTerminalPaneFromLayout,
  updateTerminalSplitRatio,
  type TerminalLayoutNode,
  type TerminalSplitPlacement,
} from "../utils/terminalLayout";
import type {
  SessionConnectRequest,
  SessionProfile,
  TerminalPaneInfo,
  TerminalSessionInfo,
} from "../types/session";

type TerminalOutputPayload = {
  sessionId: string;
  terminalId?: string;
  data: string;
};

type TerminalClosedPayload = {
  sessionId: string;
  terminalId?: string;
};

type TermEntry = {
  terminal: Terminal;
  fitAddon: FitAddon;
  onData: IDisposable;
  imeWorkaround: { dispose(): void };
  inputQueue: string;
  writing: boolean;
  writable: boolean;
};

const MAX_TERMINAL_PANES = 4;

type SplitPlacementControlsProps = {
  placement: TerminalSplitPlacement;
  ratio: number;
  onPlacementChange: (placement: TerminalSplitPlacement) => void;
  onRatioChange: (ratio: number) => void;
};

function SplitPlacementControls({
  placement,
  ratio,
  onPlacementChange,
  onRatioChange,
}: SplitPlacementControlsProps) {
  const placements: Array<{
    value: TerminalSplitPlacement;
    label: string;
    icon: typeof PanelLeft;
  }> = [
    { value: "left", label: "Left", icon: PanelLeft },
    { value: "right", label: "Right", icon: PanelRight },
    { value: "top", label: "Top", icon: PanelTop },
    { value: "bottom", label: "Bottom", icon: PanelBottom },
  ];

  return (
    <div className="terminal-split-options">
      <fieldset>
        <legend>New pane position</legend>
        <div className="terminal-split-placement-grid">
          {placements.map((option) => {
            const Icon = option.icon;
            return (
              <button
                key={option.value}
                type="button"
                className={placement === option.value ? "selected" : ""}
                aria-pressed={placement === option.value}
                aria-label={`Place new pane ${option.label.toLowerCase()}`}
                onClick={() => onPlacementChange(option.value)}
              >
                <Icon size={15} />
                {option.label}
              </button>
            );
          })}
        </div>
      </fieldset>
      <label className="terminal-split-ratio">
        <span>
          New pane size <output>{ratio}%</output>
        </span>
        <input
          type="range"
          min="20"
          max="80"
          step="5"
          value={ratio}
          aria-label="New pane size"
          onChange={(event) => onRatioChange(Number(event.target.value))}
        />
      </label>
    </div>
  );
}

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

function isClosedChannelError(error: unknown) {
  const message = String(error).toLowerCase();
  return (
    message.includes("closed this channel") ||
    message.includes("channel is closed") ||
    message.includes("channel is not open") ||
    message.includes("no active terminal")
  );
}

export function TerminalPane() {
  const activeSessionId = useSessionStore((state) => state.activeSessionId);
  const terminalViewRevision = useSessionStore(
    (state) => state.terminalViewRevision,
  );
  const terminalIdsBySession = useSessionStore(
    (state) => state.terminalIdsBySession,
  );
  const connectedSessionIds = useSessionStore((state) => state.connectedSessionIds);
  const sessions = useSessionStore((state) => state.sessions);
  const setActiveSession = useSessionStore((state) => state.setActiveSession);
  const addConnectedSession = useSessionStore(
    (state) => state.addConnectedSession,
  );
  const addTerminalPane = useSessionStore((state) => state.addTerminalPane);
  const removeTerminalPane = useSessionStore((state) => state.removeTerminalPane);
  const setMessage = useSessionStore((state) => state.setMessage);

  const instancesRef = useRef<Map<string, TermEntry>>(new Map());
  const hostRefs = useRef<Map<string, HTMLDivElement>>(new Map());
  const pendingOutputRef = useRef<Map<string, string>>(new Map());
  const activePaneIdsRef = useRef<string[]>([]);
  const fitTimerRef = useRef<number | null>(null);
  const splitPickerRef = useRef<HTMLDivElement | null>(null);
  const sessionPickerRef = useRef<HTMLDivElement | null>(null);
  const lastTerminalViewRevisionRef = useRef<number | null>(null);
  const [paneLayout, setPaneLayout] = useState<TerminalLayoutNode | null>(null);
  const [focusedTerminalId, setFocusedTerminalId] = useState<string | null>(null);
  const [splitting, setSplitting] = useState(false);
  const [splitPickerOpen, setSplitPickerOpen] = useState(false);
  const [splitPlacement, setSplitPlacement] =
    useState<TerminalSplitPlacement>("right");
  const [newPaneRatio, setNewPaneRatio] = useState(50);
  const [sessionPickerOpen, setSessionPickerOpen] = useState(false);
  const [selectedOtherSessionId, setSelectedOtherSessionId] = useState<
    string | null
  >(null);
  const [otherSessionPassword, setOtherSessionPassword] = useState("");
  const [otherProxyPassword, setOtherProxyPassword] = useState("");
  const [addingSession, setAddingSession] = useState(false);

  const activeProfile =
    sessions.find((session) => session.id === activeSessionId) ?? null;
  const activeSessionPaneIds = activeSessionId
    ? terminalIdsBySession[activeSessionId] ?? []
    : [];
  const visiblePaneIds = useMemo(
    () => getTerminalLayoutPaneIds(paneLayout),
    [paneLayout],
  );
  const allTerminalIds = useMemo(
    () => Object.values(terminalIdsBySession).flat(),
    [terminalIdsBySession],
  );
  const sessionIdByTerminalId = useMemo(() => {
    const result = new Map<string, string>();
    for (const [sessionId, terminalIds] of Object.entries(terminalIdsBySession)) {
      for (const terminalId of terminalIds) {
        result.set(terminalId, sessionId);
      }
    }
    return result;
  }, [terminalIdsBySession]);
  const visibleSessionIds = useMemo(
    () =>
      new Set(
        visiblePaneIds
          .map((terminalId) => sessionIdByTerminalId.get(terminalId))
          .filter((sessionId): sessionId is string => Boolean(sessionId)),
      ),
    [sessionIdByTerminalId, visiblePaneIds],
  );
  const focusedSessionId =
    (focusedTerminalId &&
      sessionIdByTerminalId.get(focusedTerminalId)) ||
    activeSessionId;
  const isFocusedConnected =
    focusedSessionId != null &&
    connectedSessionIds.includes(focusedSessionId);
  const selectedOtherProfile =
    sessions.find((session) => session.id === selectedOtherSessionId) ?? null;
  const otherSessions = sessions.filter(
    (session) => !visibleSessionIds.has(session.id),
  );
  const renderTerminalIds = [
    ...visiblePaneIds,
    ...allTerminalIds.filter(
      (terminalId) => !visiblePaneIds.includes(terminalId),
    ),
  ];

  activePaneIdsRef.current = visiblePaneIds;

  useEffect(() => {
    if (
      lastTerminalViewRevisionRef.current == null ||
      lastTerminalViewRevisionRef.current !== terminalViewRevision
    ) {
      lastTerminalViewRevisionRef.current = terminalViewRevision;
      setPaneLayout(createTerminalLayout(activeSessionPaneIds));
    }
  }, [
    activeSessionId,
    activeSessionPaneIds.join("|"),
    terminalViewRevision,
  ]);

  useEffect(() => {
    setPaneLayout((current) => {
      const currentIds = getTerminalLayoutPaneIds(current);
      const retainedIds = currentIds.filter((terminalId) =>
        allTerminalIds.includes(terminalId),
      );
      const desiredIds =
        retainedIds.length > 0 ? retainedIds : activeSessionPaneIds;
      return reconcileTerminalLayout(current, desiredIds);
    });
  }, [activeSessionPaneIds.join("|"), allTerminalIds.join("|")]);

  const fitVisiblePanes = () => {
    for (const terminalId of activePaneIdsRef.current) {
      const entry = instancesRef.current.get(terminalId);
      if (!entry) {
        continue;
      }
      try {
        entry.fitAddon.fit();
        entry.terminal.refresh(0, Math.max(0, entry.terminal.rows - 1));
        if (entry.writable) {
          invoke("terminal_resize", {
            terminalId,
            cols: entry.terminal.cols,
            rows: entry.terminal.rows,
          }).catch(() => undefined);
        }
      } catch {
        // xterm can throw while its container is mounting or has zero size.
      }
    }
  };

  const scheduleFit = (delayMs: number) => {
    if (fitTimerRef.current != null) {
      window.clearTimeout(fitTimerRef.current);
    }
    fitTimerRef.current = window.setTimeout(() => {
      fitTimerRef.current = null;
      fitVisiblePanes();
    }, delayMs);
  };

  const registerTerminalHost = (
    terminalId: string,
    element: HTMLDivElement | null,
  ) => {
    if (!element) {
      hostRefs.current.delete(terminalId);
      return;
    }

    hostRefs.current.set(terminalId, element);
    const entry = instancesRef.current.get(terminalId);
    const terminalElement = entry?.terminal.element;
    if (entry && terminalElement && terminalElement.parentElement !== element) {
      // A pane can move to a different branch when the split layout changes.
      // Keep the existing xterm DOM and scrollback attached to the new host
      // instead of leaving it inside the detached React element.
      element.appendChild(terminalElement);
      scheduleFit(0);
    }
  };

  const flushInput = async (terminalId: string, entry: TermEntry) => {
    if (entry.writing || !entry.writable) {
      return;
    }
    entry.writing = true;
    try {
      while (entry.writable && entry.inputQueue.length > 0) {
        // Coalesce key-repeat bursts and serialize IPC writes so multiple
        // commands never race each other against a closing SSH channel.
        const data = entry.inputQueue;
        entry.inputQueue = "";
        try {
          await invoke("terminal_write", { terminalId, data });
        } catch (error) {
          entry.writable = false;
          entry.inputQueue = "";
          if (!isClosedChannelError(error)) {
            setMessage({ kind: "error", text: String(error) });
          }
        }
      }
    } finally {
      entry.writing = false;
      if (entry.writable && entry.inputQueue.length > 0) {
        void flushInput(terminalId, entry);
      }
    }
  };

  // Create an xterm instance for every live backend pane and preserve
  // background sessions' scrollback while another host is active.
  useEffect(() => {
    for (const terminalId of allTerminalIds) {
      const host = hostRefs.current.get(terminalId);
      if (
        !host ||
        !visiblePaneIds.includes(terminalId) ||
        instancesRef.current.has(terminalId)
      ) {
        continue;
      }
      const terminal = createTerminal();
      const fitAddon = new FitAddon();
      terminal.loadAddon(fitAddon);
      terminal.open(host);
      const imeWorkaround = attachWebKitHangulImeWorkaround(terminal);
      const entry: TermEntry = {
        terminal,
        fitAddon,
        onData: { dispose() {} },
        imeWorkaround,
        inputQueue: "",
        writing: false,
        writable: true,
      };
      entry.onData = terminal.onData((data) => {
        entry.inputQueue += data;
        void flushInput(terminalId, entry);
      });
      instancesRef.current.set(terminalId, entry);

      const pending = takePendingOutput(pendingOutputRef.current, terminalId);
      if (pending) {
        terminal.write(pending);
      }
    }

    for (const [terminalId, entry] of [...instancesRef.current]) {
      if (allTerminalIds.includes(terminalId)) {
        continue;
      }
      entry.writable = false;
      entry.inputQueue = "";
      entry.onData.dispose();
      entry.imeWorkaround.dispose();
      entry.terminal.dispose();
      instancesRef.current.delete(terminalId);
    }

    scheduleFit(60);
  }, [allTerminalIds.join("|"), visiblePaneIds.join("|")]);

  useEffect(() => {
    const nextFocused =
      focusedTerminalId && visiblePaneIds.includes(focusedTerminalId)
        ? focusedTerminalId
        : visiblePaneIds[0] ?? null;
    if (nextFocused !== focusedTerminalId) {
      setFocusedTerminalId(nextFocused);
    }
    scheduleFit(60);
  }, [visiblePaneIds.join("|")]);

  useEffect(() => {
    if (!focusedTerminalId) {
      return;
    }
    const timer = window.setTimeout(
      () => instancesRef.current.get(focusedTerminalId)?.terminal.focus(),
      70,
    );
    return () => window.clearTimeout(timer);
  }, [focusedTerminalId]);

  useEffect(() => {
    if (!sessionPickerOpen && !splitPickerOpen) {
      return;
    }
    const handlePointerDown = (event: MouseEvent) => {
      const target = event.target as Node;
      if (
        !sessionPickerRef.current?.contains(target) &&
        !splitPickerRef.current?.contains(target)
      ) {
        setSessionPickerOpen(false);
        setSplitPickerOpen(false);
        setSelectedOtherSessionId(null);
        setOtherSessionPassword("");
        setOtherProxyPassword("");
      }
    };
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setSessionPickerOpen(false);
        setSplitPickerOpen(false);
        setSelectedOtherSessionId(null);
        setOtherSessionPassword("");
        setOtherProxyPassword("");
      }
    };
    document.addEventListener("mousedown", handlePointerDown);
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("mousedown", handlePointerDown);
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [sessionPickerOpen, splitPickerOpen]);

  const surfaceRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    const surface = surfaceRef.current;
    if (!surface) {
      return;
    }
    const resizeObserver = new ResizeObserver(() => scheduleFit(20));
    resizeObserver.observe(surface);
    return () => resizeObserver.disconnect();
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
        const terminalId = event.payload.terminalId ?? sessionId;
        const entry = instancesRef.current.get(terminalId);
        if (entry) {
          entry.terminal.write(data);
        } else {
          appendPendingOutput(pendingOutputRef.current, terminalId, data);
        }
      }),
    );

    register(
      listen<TerminalClosedPayload>("terminal-closed", (event) => {
        const terminalId = event.payload.terminalId ?? event.payload.sessionId;
        const entry = instancesRef.current.get(terminalId);
        if (entry) {
          entry.writable = false;
          entry.inputQueue = "";
        }
        clearPendingOutput(pendingOutputRef.current, terminalId);
      }),
    );

    return () => {
      disposed = true;
      unlisteners.forEach((unlisten) => unlisten());
      if (fitTimerRef.current != null) {
        window.clearTimeout(fitTimerRef.current);
        fitTimerRef.current = null;
      }
      for (const entry of instancesRef.current.values()) {
        entry.writable = false;
        entry.onData.dispose();
        entry.imeWorkaround.dispose();
        entry.terminal.dispose();
      }
      instancesRef.current.clear();
    };
  }, []);

  const focusPane = (terminalId: string, sessionId?: string) => {
    setFocusedTerminalId(terminalId);
    const ownerSessionId =
      sessionId ?? sessionIdByTerminalId.get(terminalId) ?? null;
    if (ownerSessionId && ownerSessionId !== activeSessionId) {
      setActiveSession(ownerSessionId);
    }
  };

  const closeSplitPicker = () => setSplitPickerOpen(false);

  const insertPaneIntoLayout = (
    terminalId: string,
    targetTerminalId = focusedTerminalId,
  ) => {
    setPaneLayout((current) =>
      insertTerminalPane(
        current,
        targetTerminalId,
        terminalId,
        splitPlacement,
        newPaneRatio,
      ),
    );
  };

  const startPaneResize = (
    event: ReactMouseEvent<HTMLDivElement>,
    path: readonly ("first" | "second")[],
    direction: "horizontal" | "vertical",
  ) => {
    event.preventDefault();
    event.stopPropagation();
    const splitElement = event.currentTarget.parentElement;
    if (!splitElement) {
      return;
    }

    const bounds = splitElement.getBoundingClientRect();
    const previousUserSelect = document.body.style.userSelect;
    const previousCursor = document.body.style.cursor;
    const cursor = direction === "horizontal" ? "col-resize" : "row-resize";
    document.body.style.userSelect = "none";
    document.body.style.cursor = cursor;

    const handleMove = (moveEvent: MouseEvent) => {
      const ratio =
        direction === "horizontal"
          ? ((moveEvent.clientX - bounds.left) / bounds.width) * 100
          : ((moveEvent.clientY - bounds.top) / bounds.height) * 100;
      setPaneLayout((current) =>
        updateTerminalSplitRatio(current, path, ratio),
      );
      scheduleFit(0);
    };
    const handleUp = () => {
      window.removeEventListener("mousemove", handleMove);
      window.removeEventListener("mouseup", handleUp);
      document.body.style.userSelect = previousUserSelect;
      document.body.style.cursor = previousCursor;
      scheduleFit(20);
    };
    window.addEventListener("mousemove", handleMove);
    window.addEventListener("mouseup", handleUp);
  };

  const addSplit = async () => {
    const sourceTerminalId =
      focusedTerminalId && visiblePaneIds.includes(focusedTerminalId)
        ? focusedTerminalId
        : visiblePaneIds[0] ?? null;
    const sourceSessionId =
      (sourceTerminalId &&
        sessionIdByTerminalId.get(sourceTerminalId)) ||
      activeSessionId;
    if (
      !sourceSessionId ||
      !connectedSessionIds.includes(sourceSessionId) ||
      visiblePaneIds.length >= MAX_TERMINAL_PANES
    ) {
      return;
    }
    setSplitting(true);
    try {
      const source =
        (sourceTerminalId &&
          instancesRef.current.get(sourceTerminalId)?.terminal) ||
        undefined;
      const info = await invoke<TerminalPaneInfo>("create_terminal_split", {
        sessionId: sourceSessionId,
        cols: source?.cols ?? 80,
        rows: source?.rows ?? 24,
      });
      addTerminalPane(sourceSessionId, info.terminalId);
      insertPaneIntoLayout(info.terminalId, sourceTerminalId);
      focusPane(info.terminalId, sourceSessionId);
      closeSplitPicker();
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
    } finally {
      setSplitting(false);
    }
  };

  const closeSplit = async (terminalId: string) => {
    const sessionId = sessionIdByTerminalId.get(terminalId);
    if (!sessionId || visiblePaneIds.length <= 1) {
      return;
    }
    try {
      await invoke("disconnect_terminal_pane", { terminalId });
      const nextLayout = removeTerminalPaneFromLayout(paneLayout, terminalId);
      const remaining = getTerminalLayoutPaneIds(nextLayout);
      removeTerminalPane(sessionId, terminalId);
      setPaneLayout(nextLayout);
      const nextTerminalId = remaining[0] ?? null;
      if (nextTerminalId) {
        focusPane(nextTerminalId);
      } else {
        setFocusedTerminalId(null);
      }
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
    }
  };

  const closeSessionPicker = () => {
    setSessionPickerOpen(false);
    setSelectedOtherSessionId(null);
    setOtherSessionPassword("");
    setOtherProxyPassword("");
  };

  const addExistingSessionPane = (
    profile: SessionProfile,
    terminalId: string,
  ) => {
    if (!visiblePaneIds.includes(terminalId)) {
      insertPaneIntoLayout(terminalId);
    }
    focusPane(terminalId, profile.id);
    closeSessionPicker();
  };

  const chooseOtherSession = (profile: SessionProfile) => {
    const existingTerminalId = terminalIdsBySession[profile.id]?.[0];
    if (
      connectedSessionIds.includes(profile.id) &&
      existingTerminalId
    ) {
      addExistingSessionPane(profile, existingTerminalId);
      return;
    }
    setSelectedOtherSessionId(profile.id);
    setOtherSessionPassword("");
    setOtherProxyPassword("");
  };

  const connectOtherSession = async (event: FormEvent) => {
    event.preventDefault();
    if (!selectedOtherProfile || visiblePaneIds.length >= MAX_TERMINAL_PANES) {
      return;
    }
    const source =
      (focusedTerminalId &&
        instancesRef.current.get(focusedTerminalId)?.terminal) ||
      undefined;
    const request: SessionConnectRequest = {
      id: selectedOtherProfile.id,
      name: selectedOtherProfile.name,
      host: selectedOtherProfile.host,
      port: selectedOtherProfile.port,
      username: selectedOtherProfile.username,
      password: otherSessionPassword || null,
      privateKeyPath: selectedOtherProfile.privateKeyPath ?? null,
      proxyJumpId: selectedOtherProfile.proxyJumpId ?? null,
      proxyJumpPassword: otherProxyPassword || null,
      cols: source?.cols ?? 80,
      rows: source?.rows ?? 24,
    };

    setAddingSession(true);
    try {
      const info = await withHostKeyPrompt(() =>
        invoke<TerminalSessionInfo>("connect_terminal", { request }),
      );
      const terminalId = info.terminalId ?? info.sessionId;
      addConnectedSession(info.sessionId, terminalId);
      insertPaneIntoLayout(terminalId);
      focusPane(terminalId, info.sessionId);
      closeSessionPicker();
      setMessage({
        kind: "success",
        text: `${info.profile.name} added to the terminal layout`,
      });
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
    } finally {
      setAddingSession(false);
    }
  };

  const disconnect = useDisconnectSession();

  const renderTerminalCell = (terminalId: string, visible: boolean) => {
    const paneSessionId = sessionIdByTerminalId.get(terminalId);
    const paneProfile = sessions.find(
      (session) => session.id === paneSessionId,
    );
    return (
      <div
        key={terminalId}
        role={visible ? "group" : undefined}
        aria-label={
          visible ? `${paneProfile?.name ?? "SSH"} terminal pane` : undefined
        }
        className={`terminal-split-cell ${
          visible ? "" : "terminal-split-cell-hidden"
        } ${visible && terminalId === focusedTerminalId ? "focused" : ""} ${
          visible && visibleSessionIds.size > 1 ? "mixed-session" : ""
        }`}
        onMouseDown={() => {
          if (visible) {
            focusPane(terminalId);
          }
        }}
      >
        {visible && visibleSessionIds.size > 1 && (
          <span
            className="terminal-split-session-label"
            title={
              paneProfile
                ? `${paneProfile.name} (${paneProfile.username}@${paneProfile.host})`
                : paneSessionId
            }
          >
            {paneProfile?.name ?? "SSH"}
          </span>
        )}
        {visible && visiblePaneIds.length > 1 && (
          <button
            className="terminal-split-close"
            type="button"
            aria-label="Close terminal pane"
            title="Close terminal pane"
            onClick={(event) => {
              event.stopPropagation();
              void closeSplit(terminalId);
            }}
          >
            <X size={13} />
          </button>
        )}
        <div
          className="xterm-host"
          ref={(element) => registerTerminalHost(terminalId, element)}
        />
      </div>
    );
  };

  const renderLayoutNode = (
    node: TerminalLayoutNode,
    path: readonly ("first" | "second")[] = [],
  ): ReactNode => {
    if (node.type === "pane") {
      return renderTerminalCell(node.terminalId, true);
    }

    const horizontal = node.direction === "horizontal";
    const style = horizontal
      ? { gridTemplateColumns: `${node.ratio}fr 6px ${100 - node.ratio}fr` }
      : { gridTemplateRows: `${node.ratio}fr 6px ${100 - node.ratio}fr` };
    return (
      <div
        className={`terminal-split-node ${node.direction}`}
        style={style}
      >
        <div className="terminal-split-child">
          {renderLayoutNode(node.first, [...path, "first"])}
        </div>
        <div
          className="terminal-pane-divider"
          role="separator"
          aria-label="Resize terminal panes"
          aria-orientation={horizontal ? "vertical" : "horizontal"}
          aria-valuemin={15}
          aria-valuemax={85}
          aria-valuenow={node.ratio}
          onMouseDown={(event) =>
            startPaneResize(event, path, node.direction)
          }
        />
        <div className="terminal-split-child">
          {renderLayoutNode(node.second, [...path, "second"])}
        </div>
      </div>
    );
  };

  return (
    <div className="terminal-pane">
      <div className="terminal-tabs">
        <div className={`terminal-tab ${isFocusedConnected ? "online" : ""}`}>
          <TerminalIcon size={16} />
          <span>
            {activeProfile
              ? `${activeProfile.name} (${activeProfile.host})`
              : "Terminal"}
          </span>
          {visiblePaneIds.length > 1 && (
            <small>
              {visibleSessionIds.size > 1
                ? `${visibleSessionIds.size} sessions · ${visiblePaneIds.length} panes`
                : `${visiblePaneIds.length} panes`}
            </small>
          )}
        </div>
        <div className="terminal-tab-actions">
          <div className="terminal-split-picker-anchor" ref={splitPickerRef}>
            <button
              className="icon-button"
              type="button"
              disabled={
                !isFocusedConnected ||
                splitting ||
                visiblePaneIds.length >= MAX_TERMINAL_PANES
              }
              aria-label="Split terminal"
              title="Split focused session"
              aria-expanded={splitPickerOpen}
              onClick={() => {
                setSessionPickerOpen(false);
                setSplitPickerOpen((open) => !open);
              }}
            >
              {splitting ? (
                <LoaderCircle className="spin" size={16} />
              ) : (
                <Columns2 size={16} />
              )}
            </button>
            {splitPickerOpen && (
              <div
                className="terminal-split-picker"
                role="dialog"
                aria-label="Split terminal options"
              >
                <div className="terminal-session-picker-heading">
                  Split focused pane
                </div>
                <SplitPlacementControls
                  placement={splitPlacement}
                  ratio={newPaneRatio}
                  onPlacementChange={setSplitPlacement}
                  onRatioChange={setNewPaneRatio}
                />
                <button
                  className="primary-button"
                  type="button"
                  disabled={splitting}
                  onClick={() => void addSplit()}
                >
                  {splitting ? (
                    <LoaderCircle className="spin" size={15} />
                  ) : (
                    <Columns2 size={15} />
                  )}
                  Split pane
                </button>
              </div>
            )}
          </div>
          <div className="terminal-session-picker-anchor" ref={sessionPickerRef}>
            <button
              className="icon-button"
              type="button"
              disabled={
                addingSession ||
                visiblePaneIds.length >= MAX_TERMINAL_PANES ||
                otherSessions.length === 0
              }
              aria-label="Add another session"
              title="Add another session to this terminal layout"
              aria-expanded={sessionPickerOpen}
              onClick={() => {
                if (sessionPickerOpen) {
                  closeSessionPicker();
                } else {
                  setSplitPickerOpen(false);
                  setSessionPickerOpen(true);
                }
              }}
            >
              {addingSession ? (
                <LoaderCircle className="spin" size={16} />
              ) : (
                <Plus size={16} />
              )}
            </button>
            {sessionPickerOpen && (
              <div
                className="terminal-session-picker"
                role="dialog"
                aria-label="Add another session"
              >
                <SplitPlacementControls
                  placement={splitPlacement}
                  ratio={newPaneRatio}
                  onPlacementChange={setSplitPlacement}
                  onRatioChange={setNewPaneRatio}
                />
                {selectedOtherProfile ? (
                  <form onSubmit={connectOtherSession}>
                    <div className="terminal-session-picker-title">
                      <button
                        className="icon-button ghost"
                        type="button"
                        aria-label="Back to sessions"
                        onClick={() => setSelectedOtherSessionId(null)}
                      >
                        <ArrowLeft size={14} />
                      </button>
                      <span>
                        <strong>{selectedOtherProfile.name}</strong>
                        <small>
                          {selectedOtherProfile.username}@
                          {selectedOtherProfile.host}
                        </small>
                      </span>
                    </div>
                    <label>
                      <span>Password / key passphrase</span>
                      <input
                        autoFocus
                        type="password"
                        autoComplete="current-password"
                        value={otherSessionPassword}
                        placeholder="Leave blank for key/agent auth"
                        onChange={(event) =>
                          setOtherSessionPassword(event.target.value)
                        }
                      />
                    </label>
                    {selectedOtherProfile.proxyJumpId && (
                      <label>
                        <span>Jump host password</span>
                        <input
                          type="password"
                          autoComplete="off"
                          value={otherProxyPassword}
                          placeholder="Leave blank for key/agent auth"
                          onChange={(event) =>
                            setOtherProxyPassword(event.target.value)
                          }
                        />
                      </label>
                    )}
                    <button
                      className="primary-button"
                      type="submit"
                      disabled={addingSession}
                    >
                      {addingSession ? (
                        <LoaderCircle className="spin" size={15} />
                      ) : (
                        <Plus size={15} />
                      )}
                      Add pane
                    </button>
                  </form>
                ) : (
                  <>
                    <div className="terminal-session-picker-heading">
                      Add another session
                    </div>
                    <div className="terminal-session-picker-list">
                      {otherSessions.map((session) => (
                        <button
                          key={session.id}
                          type="button"
                          onClick={() => chooseOtherSession(session)}
                        >
                          <Server size={15} />
                          <span>
                            <strong>{session.name}</strong>
                            <small>
                              {session.username}@{session.host}
                            </small>
                          </span>
                          {connectedSessionIds.includes(session.id) && (
                            <i title="Connected">Live</i>
                          )}
                        </button>
                      ))}
                    </div>
                  </>
                )}
              </div>
            )}
          </div>
          <button
            className="icon-button"
            type="button"
            disabled={!isFocusedConnected}
            aria-label="Disconnect terminal"
            title="Disconnect"
            onClick={() => disconnect(focusedSessionId ?? undefined)}
          >
            <X size={16} />
          </button>
        </div>
      </div>
      <div className="terminal-surface" ref={surfaceRef}>
        {visiblePaneIds.length === 0 && (
          <div className="terminal-empty">No active SSH session</div>
        )}
        <div className="terminal-split-grid">
          {paneLayout && renderLayoutNode(paneLayout)}
          {renderTerminalIds
            .filter((terminalId) => !visiblePaneIds.includes(terminalId))
            .map((terminalId) => renderTerminalCell(terminalId, false))}
        </div>
      </div>
    </div>
  );
}
