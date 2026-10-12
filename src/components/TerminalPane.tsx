import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type FormEvent,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { confirm } from "@tauri-apps/plugin-dialog";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { Terminal, type IDisposable } from "@xterm/xterm";
import {
  ArrowLeft,
  ChevronDown,
  ChevronUp,
  Columns2,
  KeyRound,
  Laptop,
  LoaderCircle,
  PanelBottom,
  PanelLeft,
  PanelRight,
  PanelTop,
  PlugZap,
  Plus,
  RefreshCw,
  Server,
  SquareTerminal,
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
import { snippetPayload } from "../utils/snippetPayload";
import { SnippetsPicker } from "./SnippetsPicker";
import { TerminalSettingsPopover } from "./TerminalSettingsPopover";
import { useSettingsStore } from "../stores/settingsStore";
import {
  DEFAULT_TERMINAL_FONT_SIZE,
  searchDecorations,
  type TerminalAppearance,
} from "../utils/terminalAppearance";
import { matchTerminalShortcut } from "../utils/terminalShortcuts";
import type { Snippet } from "../types/snippet";
import { shellQuote } from "../utils/shellQuote";
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
  TerminalClosedPayload,
  TerminalPaneInfo,
  TerminalSessionInfo,
  TmuxListing,
  TmuxSession,
} from "../types/session";

type TerminalOutputPayload = {
  sessionId: string;
  terminalId?: string;
  data: string;
};

type TermEntry = {
  terminal: Terminal;
  fitAddon: FitAddon;
  searchAddon: SearchAddon;
  onSearchResults: IDisposable;
  onData: IDisposable;
  imeWorkaround: { dispose(): void };
  inputQueue: string;
  writing: boolean;
  writable: boolean;
};

const MAX_TERMINAL_PANES = 4;

/** Compact relative age for a tmux session's creation time (unix seconds). */
function formatTmuxAge(createdSeconds: number): string {
  const seconds = Math.max(0, Date.now() / 1000 - createdSeconds);
  if (seconds < 60) {
    return "just now";
  }
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) {
    return `${minutes}m ago`;
  }
  const hours = Math.floor(minutes / 60);
  if (hours < 24) {
    return `${hours}h ago`;
  }
  return `${Math.floor(hours / 24)}d ago`;
}

/**
 * Returns why tmux would reject this session name, or null if it is usable.
 * A blank name is allowed — tmux then numbers the session itself.
 */
function tmuxNameError(name: string): string | null {
  if (name === "") {
    return null;
  }
  if (/[.:]/.test(name)) {
    return "Session names cannot contain '.' or ':'";
  }
  if (name.startsWith("-")) {
    // Quoting does not help: tmux's own parser would read it as an option.
    return "Session names cannot start with '-'";
  }
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f]/.test(name)) {
    return "Session names cannot contain control characters";
  }
  return null;
}

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

function createTerminal(appearance: TerminalAppearance): Terminal {
  return new Terminal({
    cursorBlink: true,
    convertEol: true,
    fontFamily: appearance.font.family,
    fontSize: appearance.fontSize,
    lineHeight: 1.18,
    scrollback: 8000,
    theme: appearance.theme.xterm,
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
  const terminalAppearance = useSettingsStore((state) => state.terminalAppearance);
  const setTerminalAppearance = useSettingsStore(
    (state) => state.setTerminalAppearance,
  );
  // New panes read the current look without re-running the creation effect.
  const terminalAppearanceRef = useRef(terminalAppearance);
  terminalAppearanceRef.current = terminalAppearance;

  const instancesRef = useRef<Map<string, TermEntry>>(new Map());
  const hostRefs = useRef<Map<string, HTMLDivElement>>(new Map());
  const pendingOutputRef = useRef<Map<string, string>>(new Map());
  const activePaneIdsRef = useRef<string[]>([]);
  const fitTimerRef = useRef<number | null>(null);
  const splitPickerRef = useRef<HTMLDivElement | null>(null);
  const sessionPickerRef = useRef<HTMLDivElement | null>(null);
  const tmuxPickerRef = useRef<HTMLDivElement | null>(null);
  const lastTerminalViewRevisionRef = useRef<number | null>(null);
  const [paneLayout, setPaneLayout] = useState<TerminalLayoutNode | null>(null);
  const [focusedTerminalId, setFocusedTerminalId] = useState<string | null>(null);
  // Panes whose shell or channel ended, with the reason the backend reported.
  // Without this the pane just silently stops accepting input.
  const [closedPanes, setClosedPanes] = useState<Record<string, string>>({});
  const [reconnectingPane, setReconnectingPane] = useState<string | null>(null);
  // Scrollback search for the focused pane. An inline bar rather than a dialog,
  // so it introduces no new focus trap.
  const [searchOpen, setSearchOpen] = useState(false);
  const [searchQuery, setSearchQuery] = useState("");
  const [searchMatches, setSearchMatches] = useState<{
    current: number;
    total: number;
  } | null>(null);
  const searchInputRef = useRef<HTMLInputElement | null>(null);
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
  const [otherSessionCredentialStored, setOtherSessionCredentialStored] =
    useState(false);
  const [otherProxyCredentialStored, setOtherProxyCredentialStored] =
    useState(false);
  const [addingSession, setAddingSession] = useState(false);
  const [tmuxPickerOpen, setTmuxPickerOpen] = useState(false);
  const [tmuxListing, setTmuxListing] = useState<TmuxListing | null>(null);
  const [tmuxLoading, setTmuxLoading] = useState(false);
  // tmux failures stay inside the popover: `setMessage({kind:"error"})` opens a
  // blocking modal, which is far too heavy for "this host has no tmux".
  const [tmuxError, setTmuxError] = useState<string | null>(null);
  const [tmuxBusy, setTmuxBusy] = useState(false);
  const [tmuxKillingId, setTmuxKillingId] = useState<string | null>(null);
  const [newTmuxName, setNewTmuxName] = useState("");
  const [newTmuxNameError, setNewTmuxNameError] = useState<string | null>(null);

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
  // The pane a new split is cut from. Derived once so the tmux picker cannot
  // list one host's sessions and then attach on another: `focusedTerminalId`
  // may not be visible, and its fallback differs from `focusedSessionId`'s.
  const splitSourceTerminalId =
    focusedTerminalId && visiblePaneIds.includes(focusedTerminalId)
      ? focusedTerminalId
      : visiblePaneIds[0] ?? null;
  const splitSourceSessionId =
    (splitSourceTerminalId &&
      sessionIdByTerminalId.get(splitSourceTerminalId)) ||
    activeSessionId;
  const selectedOtherProfile =
    sessions.find((session) => session.id === selectedOtherSessionId) ?? null;
  const selectedOtherProxyProfile =
    sessions.find(
      (session) => session.id === selectedOtherProfile?.proxyJumpId,
    ) ?? null;
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
    let disposed = false;
    setOtherSessionCredentialStored(false);
    setOtherProxyCredentialStored(false);

    const check = async (sessionId: string | undefined) => {
      if (!sessionId) return false;
      try {
        return (
          (await invoke<boolean>("has_saved_credential", { sessionId })) === true
        );
      } catch {
        return false;
      }
    };

    void Promise.all([
      check(selectedOtherProfile?.isLocal ? undefined : selectedOtherProfile?.id),
      check(selectedOtherProxyProfile?.id),
    ]).then(([targetStored, proxyStored]) => {
      if (!disposed) {
        setOtherSessionCredentialStored(targetStored);
        setOtherProxyCredentialStored(proxyStored);
      }
    });
    return () => {
      disposed = true;
    };
  }, [selectedOtherProfile?.id, selectedOtherProfile?.isLocal, selectedOtherProxyProfile?.id]);

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

  // Types a saved command into the focused pane through the same queue as
  // keystrokes, so it can never interleave with typing in flight.
  const sendSnippet = (snippet: Snippet) => {
    const terminalId = splitSourceTerminalId;
    const entry = terminalId ? instancesRef.current.get(terminalId) : undefined;
    if (!terminalId || !entry || !entry.writable) {
      setMessage({ kind: "error", text: "The focused terminal is not connected" });
      return;
    }
    entry.inputQueue += snippetPayload(
      snippet.command,
      snippet.appendReturn,
      entry.terminal.modes.bracketedPasteMode,
    );
    void flushInput(terminalId, entry);
    entry.terminal.focus();
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
      const terminal = createTerminal(terminalAppearanceRef.current);
      const fitAddon = new FitAddon();
      const searchAddon = new SearchAddon();
      terminal.loadAddon(fitAddon);
      terminal.loadAddon(searchAddon);
      terminal.open(host);
      const imeWorkaround = attachWebKitHangulImeWorkaround(terminal);
      const entry: TermEntry = {
        terminal,
        fitAddon,
        searchAddon,
        // The addon is the authority on match counts; deriving them here would
        // only ever approximate what it already knows.
        onSearchResults: searchAddon.onDidChangeResults((results) =>
          setSearchMatches(
            results
              ? { current: results.resultIndex + 1, total: results.resultCount }
              : null,
          ),
        ),
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
      entry.onSearchResults.dispose();
      entry.imeWorkaround.dispose();
      entry.searchAddon.dispose();
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
    if (!sessionPickerOpen && !splitPickerOpen && !tmuxPickerOpen) {
      return;
    }
    const closeAll = () => {
      setSessionPickerOpen(false);
      setSplitPickerOpen(false);
      setSelectedOtherSessionId(null);
      setOtherSessionPassword("");
      setOtherProxyPassword("");
      setTmuxPickerOpen(false);
      setNewTmuxName("");
      setNewTmuxNameError(null);
    };
    const handlePointerDown = (event: MouseEvent) => {
      const target = event.target as Node;
      if (
        !sessionPickerRef.current?.contains(target) &&
        !splitPickerRef.current?.contains(target) &&
        !tmuxPickerRef.current?.contains(target)
      ) {
        closeAll();
      }
    };
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        closeAll();
      }
    };
    document.addEventListener("mousedown", handlePointerDown);
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("mousedown", handlePointerDown);
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [sessionPickerOpen, splitPickerOpen, tmuxPickerOpen]);

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
        setClosedPanes((current) => ({
          ...current,
          [terminalId]: event.payload.message ?? "Connection closed",
        }));
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
        entry.onSearchResults.dispose();
        entry.imeWorkaround.dispose();
        entry.searchAddon.dispose();
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
    placement = splitPlacement,
  ) => {
    setPaneLayout((current) =>
      insertTerminalPane(
        current,
        targetTerminalId,
        terminalId,
        placement,
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

  /**
   * Opens a split pane on the focused session, optionally typing a command into
   * its fresh login shell. Returns null when the guards reject the request.
   *
   * Shared by the split button and the tmux picker so a tmux pane is created
   * exactly like any other split.
   */
  const openSplitSourcePane = async (
    setBusy: (busy: boolean) => void,
    initialCommand?: string,
    placement = splitPlacement,
  ) => {
    const sourceTerminalId = splitSourceTerminalId;
    const sourceSessionId = splitSourceSessionId;
    if (
      !sourceSessionId ||
      !connectedSessionIds.includes(sourceSessionId) ||
      visiblePaneIds.length >= MAX_TERMINAL_PANES
    ) {
      return null;
    }
    setBusy(true);
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
      if (initialCommand) {
        // Type-ahead into the new pane's login shell. Its TermEntry does not
        // exist yet, so there is no input queue to join and no keystroke this
        // could interleave with; the remote PTY holds the bytes until the
        // shell first reads. Ctrl-U discards anything a profile left on the
        // line, and the leading space keeps it out of shell history for users
        // who set HISTCONTROL=ignorespace.
        await invoke("terminal_write", {
          terminalId: info.terminalId,
          data: `\u0015 ${initialCommand}\r`,
        });
      }
      addTerminalPane(sourceSessionId, info.terminalId);
      insertPaneIntoLayout(info.terminalId, sourceTerminalId, placement);
      focusPane(info.terminalId, sourceSessionId);
      return info;
    } finally {
      setBusy(false);
    }
  };

  const addSplit = async () => {
    try {
      if (await openSplitSourcePane(setSplitting)) {
        closeSplitPicker();
      }
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
    }
  };

  const closeTmuxPicker = () => {
    setTmuxPickerOpen(false);
    setNewTmuxName("");
    setNewTmuxNameError(null);
  };

  const loadTmuxSessions = useCallback(async () => {
    if (!splitSourceSessionId) {
      return;
    }
    setTmuxLoading(true);
    setTmuxError(null);
    try {
      const listing = await invoke<TmuxListing>("list_tmux_sessions", {
        sessionId: splitSourceSessionId,
      });
      setTmuxListing(listing ?? null);
    } catch (error) {
      setTmuxListing(null);
      setTmuxError(String(error));
    } finally {
      setTmuxLoading(false);
    }
  }, [splitSourceSessionId]);

  // Fetch when the popover opens, and refetch if the focused host changes
  // while it is still open.
  useEffect(() => {
    if (!tmuxPickerOpen) {
      return;
    }
    void loadTmuxSessions();
  }, [tmuxPickerOpen, loadTmuxSessions]);

  const attachTmuxSession = async (session: TmuxSession) => {
    try {
      // Address the session by tmux's own id, never by its name: the name is
      // remote-controlled data and this string is typed straight into a live
      // login shell. `env TMUX=` clears the nesting guard for the common
      // dotfile that auto-attaches tmux on interactive login.
      const opened = await openSplitSourcePane(
        setTmuxBusy,
        `env TMUX= tmux attach-session -t ${shellQuote(session.id)}`,
      );
      if (opened) {
        closeTmuxPicker();
      }
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
    }
  };

  const createTmuxSession = async (event: FormEvent) => {
    event.preventDefault();
    const name = newTmuxName.trim();
    const nameError = tmuxNameError(name);
    if (nameError) {
      setNewTmuxNameError(nameError);
      return;
    }
    try {
      // `-A` attaches instead of failing if the name is already taken. A blank
      // name lets tmux pick the next number.
      const command = name
        ? `env TMUX= tmux new-session -A -s ${shellQuote(name)}`
        : "env TMUX= tmux new-session";
      const opened = await openSplitSourcePane(setTmuxBusy, command);
      if (opened) {
        closeTmuxPicker();
      }
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
    }
  };

  const killTmuxSession = async (session: TmuxSession) => {
    if (!splitSourceSessionId) {
      return;
    }
    const accepted = await confirm(
      `Kill tmux session "${session.name}"? Everything running inside it stops.`,
      { title: "Kill tmux session", kind: "warning" },
    );
    if (!accepted) {
      return;
    }
    setTmuxKillingId(session.id);
    try {
      await invoke("kill_tmux_session", {
        sessionId: splitSourceSessionId,
        tmuxId: session.id,
      });
      await loadTmuxSessions();
    } catch (error) {
      setTmuxError(String(error));
    } finally {
      setTmuxKillingId(null);
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
      isLocal: selectedOtherProfile.isLocal ?? false,
      password: selectedOtherProfile.isLocal
        ? null
        : otherSessionPassword || null,
      privateKeyPath: selectedOtherProfile.isLocal
        ? null
        : selectedOtherProfile.privateKeyPath ?? null,
      proxyJumpId: selectedOtherProfile.isLocal
        ? null
        : selectedOtherProfile.proxyJumpId ?? null,
      proxyJumpPassword: selectedOtherProfile.isLocal
        ? null
        : otherProxyPassword || null,
      reuseStoredCredentials: true,
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
        kind: info.credentialWarning ? "info" : "success",
        text: info.credentialWarning
          ? `${info.profile.name} was added, but secure credential storage reported: ${info.credentialWarning}`
          : `${info.profile.name} added to the terminal layout`,
      });
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
    } finally {
      setAddingSession(false);
    }
  };

  const disconnect = useDisconnectSession();

  const runSearch = (query: string, direction: "next" | "previous") => {
    const entry = focusedTerminalId
      ? instancesRef.current.get(focusedTerminalId)
      : null;
    if (!entry) {
      return;
    }
    if (!query) {
      entry.searchAddon.clearDecorations();
      setSearchMatches(null);
      return;
    }
    const options = {
      decorations: searchDecorations(terminalAppearance.theme),
    };
    const found =
      direction === "next"
        ? entry.searchAddon.findNext(query, options)
        : entry.searchAddon.findPrevious(query, options);
    if (!found) {
      setSearchMatches({ current: 0, total: 0 });
    }
  };

  const openSearch = useCallback(() => {
    // The bar renders inside the focused pane, so without one there is nothing
    // to search and nothing would appear.
    if (!focusedTerminalId || !instancesRef.current.has(focusedTerminalId)) {
      return;
    }
    setSearchOpen(true);
    // Deferred so the input exists before focus moves to it.
    window.setTimeout(() => searchInputRef.current?.select(), 0);
  }, [focusedTerminalId]);

  const closeSearch = () => {
    setSearchOpen(false);
    setSearchMatches(null);
    for (const entry of instancesRef.current.values()) {
      entry.searchAddon.clearDecorations();
    }
    if (focusedTerminalId) {
      instancesRef.current.get(focusedTerminalId)?.terminal.focus();
    }
  };

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!(event.ctrlKey || event.metaKey) || event.key.toLowerCase() !== "f") {
        return;
      }
      // Ctrl+F belongs to whatever field the user is typing in — the path box,
      // the profile form — unless that field is this search bar. The target is
      // not always an element: with focus nowhere it is the window itself.
      const { target } = event;
      if (target instanceof Element) {
        const field = target.closest("input, textarea, select");
        if (field && !target.closest(".terminal-search-bar")) {
          return;
        }
      }
      event.preventDefault();
      openSearch();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [openSearch]);

  // Restyle every live pane when the theme, font or size changes, then refit:
  // a new font size changes how many rows and columns fit.
  useEffect(() => {
    for (const entry of instancesRef.current.values()) {
      entry.terminal.options.theme = terminalAppearance.theme.xterm;
      entry.terminal.options.fontFamily = terminalAppearance.font.family;
      entry.terminal.options.fontSize = terminalAppearance.fontSize;
    }
    scheduleFit(0);
    // scheduleFit only touches refs.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [terminalAppearance]);

  const focusAdjacentPane = (delta: number) => {
    if (visiblePaneIds.length < 2) {
      return;
    }
    const index = focusedTerminalId ? visiblePaneIds.indexOf(focusedTerminalId) : -1;
    const next =
      visiblePaneIds[
        (Math.max(index, 0) + delta + visiblePaneIds.length) % visiblePaneIds.length
      ];
    focusPane(next);
    instancesRef.current.get(next)?.terminal.focus();
  };

  // Clears scrollback, then sends Ctrl-L so the shell redraws its prompt.
  const clearFocusedScreen = () => {
    const terminalId = splitSourceTerminalId;
    const entry = terminalId ? instancesRef.current.get(terminalId) : undefined;
    if (!terminalId || !entry) {
      return;
    }
    entry.terminal.clear();
    if (entry.writable) {
      entry.inputQueue += "\u000c";
      void flushInput(terminalId, entry);
    }
  };

  const runShortcut = (action: NonNullable<ReturnType<typeof matchTerminalShortcut>>) => {
    switch (action) {
      case "splitRight":
      case "splitDown":
        if (!splitting) {
          void openSplitSourcePane(
            setSplitting,
            undefined,
            action === "splitRight" ? "right" : "bottom",
          ).catch((error) => setMessage({ kind: "error", text: String(error) }));
        }
        return;
      case "closePane":
        if (focusedTerminalId && visiblePaneIds.length > 1) {
          void closeSplit(focusedTerminalId);
        }
        return;
      case "clear":
        clearFocusedScreen();
        return;
      case "find":
        openSearch();
        return;
      case "findNext":
      case "findPrevious":
        if (searchOpen && searchQuery) {
          runSearch(searchQuery, action === "findNext" ? "next" : "previous");
        } else {
          openSearch();
        }
        return;
      case "fontBigger":
      case "fontSmaller":
        setTerminalAppearance((current) => ({
          ...current,
          fontSize: current.fontSize + (action === "fontBigger" ? 1 : -1),
        }));
        return;
      case "fontReset":
        setTerminalAppearance((current) => ({
          ...current,
          fontSize: DEFAULT_TERMINAL_FONT_SIZE,
        }));
        return;
      case "previousPane":
        focusAdjacentPane(-1);
        return;
      case "nextPane":
        focusAdjacentPane(1);
        return;
    }
  };
  const runShortcutRef = useRef(runShortcut);
  runShortcutRef.current = runShortcut;

  // Capture phase, so a shortcut typed into a pane is handled before xterm
  // forwards the key to the remote shell.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const action = matchTerminalShortcut(event);
      if (!action) {
        return;
      }
      // Other text fields keep their own keys; xterm's helper textarea and
      // the search bar do not count as such fields.
      const { target } = event;
      if (target instanceof Element) {
        const field = target.closest("input, textarea, select");
        if (
          field &&
          !target.closest(".xterm") &&
          !target.closest(".terminal-search-bar")
        ) {
          return;
        }
      }
      event.preventDefault();
      event.stopPropagation();
      runShortcutRef.current(action);
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, []);

  // Reopens a shell for a pane whose connection ended.
  const reconnectPane = async (terminalId: string) => {
    const paneSessionId = sessionIdByTerminalId.get(terminalId);
    const profile = sessions.find((session) => session.id === paneSessionId);
    if (!paneSessionId || !profile) {
      setMessage({
        kind: "error",
        text: "This pane's profile is no longer available",
      });
      return;
    }

    const source = instancesRef.current.get(terminalId)?.terminal;
    const request: SessionConnectRequest = {
      id: profile.id,
      name: profile.name,
      host: profile.host,
      port: profile.port,
      username: profile.username,
      isLocal: profile.isLocal ?? false,
      password: null,
      privateKeyPath: profile.isLocal ? null : profile.privateKeyPath ?? null,
      proxyJumpId: profile.isLocal ? null : profile.proxyJumpId ?? null,
      proxyJumpPassword: null,
      // Credentials stay in memory for the process lifetime, so a reconnect
      // does not re-prompt for a password that already worked.
      reuseStoredCredentials: true,
      cols: source?.cols ?? 80,
      rows: source?.rows ?? 24,
    };

    setReconnectingPane(terminalId);
    try {
      const info = await withHostKeyPrompt(() =>
        invoke<TerminalSessionInfo>("connect_terminal", { request }),
      );
      const nextTerminalId = info.terminalId ?? info.sessionId;
      addConnectedSession(info.sessionId, nextTerminalId);
      setClosedPanes((current) => {
        const { [terminalId]: _closed, ...rest } = current;
        return rest;
      });
      focusPane(nextTerminalId, info.sessionId);
      setMessage({ kind: "success", text: `${info.profile.name} reconnected` });
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
    } finally {
      setReconnectingPane(null);
    }
  };

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
        {visible && searchOpen && terminalId === focusedTerminalId && (
          <div
            className="terminal-search-bar"
            role="search"
            onMouseDown={(event) => event.stopPropagation()}
          >
            <input
              ref={searchInputRef}
              value={searchQuery}
              type="search"
              autoFocus
              aria-label="Search terminal output"
              placeholder="Find in terminal"
              onChange={(event) => {
                setSearchQuery(event.target.value);
                runSearch(event.target.value, "next");
              }}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  runSearch(searchQuery, event.shiftKey ? "previous" : "next");
                } else if (event.key === "Escape") {
                  event.preventDefault();
                  closeSearch();
                }
              }}
            />
            <span className="terminal-search-status" aria-live="polite">
              {searchQuery === ""
                ? ""
                : searchMatches == null
                  ? "…"
                  : searchMatches.total === 0
                    ? "No matches"
                    : `${searchMatches.current}/${searchMatches.total}`}
            </span>
            <button
              type="button"
              aria-label="Previous match"
              title="Previous match (Shift+Enter)"
              disabled={searchQuery === ""}
              onClick={() => runSearch(searchQuery, "previous")}
            >
              <ChevronUp size={14} />
            </button>
            <button
              type="button"
              aria-label="Next match"
              title="Next match (Enter)"
              disabled={searchQuery === ""}
              onClick={() => runSearch(searchQuery, "next")}
            >
              <ChevronDown size={14} />
            </button>
            <button
              type="button"
              aria-label="Close search"
              title="Close search (Escape)"
              onClick={closeSearch}
            >
              <X size={14} />
            </button>
          </div>
        )}
        {visible && closedPanes[terminalId] && (
          <div className="terminal-pane-closed" role="status">
            <strong>Disconnected</strong>
            <span>{closedPanes[terminalId]}</span>
            <button
              className="primary-button"
              type="button"
              disabled={reconnectingPane === terminalId}
              onClick={(event) => {
                event.stopPropagation();
                void reconnectPane(terminalId);
              }}
            >
              <PlugZap size={15} />
              {reconnectingPane === terminalId ? "Reconnecting…" : "Reconnect"}
            </button>
          </div>
        )}
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
              ? activeProfile.isLocal
                ? `${activeProfile.name} (local)`
                : `${activeProfile.name} (${activeProfile.host})`
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
          <div className="terminal-tmux-picker-anchor" ref={tmuxPickerRef}>
            <button
              className="icon-button"
              type="button"
              disabled={
                !isFocusedConnected ||
                tmuxBusy ||
                visiblePaneIds.length >= MAX_TERMINAL_PANES
              }
              aria-label="tmux sessions"
              title="tmux sessions on the focused server"
              aria-expanded={tmuxPickerOpen}
              onClick={() => {
                if (tmuxPickerOpen) {
                  closeTmuxPicker();
                } else {
                  setSplitPickerOpen(false);
                  closeSessionPicker();
                  setTmuxPickerOpen(true);
                }
              }}
            >
              {tmuxBusy ? (
                <LoaderCircle className="spin" size={16} />
              ) : (
                <SquareTerminal size={16} />
              )}
            </button>
            {tmuxPickerOpen && (
              <div
                className="terminal-tmux-picker"
                role="dialog"
                aria-label="tmux sessions"
              >
                <div className="terminal-tmux-picker-title">
                  <span className="terminal-session-picker-heading">
                    tmux sessions
                  </span>
                  <button
                    className="icon-button ghost"
                    type="button"
                    aria-label="Refresh tmux sessions"
                    title="Refresh"
                    disabled={tmuxLoading}
                    onClick={() => void loadTmuxSessions()}
                  >
                    <RefreshCw
                      className={tmuxLoading ? "spin" : undefined}
                      size={14}
                    />
                  </button>
                </div>
                {tmuxLoading ? (
                  <div className="terminal-tmux-status">
                    <LoaderCircle className="spin" size={14} />
                    Loading sessions…
                  </div>
                ) : tmuxError ? (
                  <div className="terminal-tmux-status error" role="alert">
                    {tmuxError}
                  </div>
                ) : tmuxListing?.status === "unsupported-os" ? (
                  <div className="terminal-tmux-status">
                    tmux is not available on Windows hosts
                  </div>
                ) : tmuxListing?.status === "not-installed" ? (
                  <div className="terminal-tmux-status">
                    tmux was not found on this host's login PATH
                  </div>
                ) : tmuxListing && tmuxListing.sessions.length === 0 ? (
                  <div className="terminal-tmux-status">
                    No tmux sessions are running
                  </div>
                ) : (
                  <div className="terminal-session-picker-list">
                    {(tmuxListing?.sessions ?? []).map((session) => (
                      <div className="terminal-tmux-row" key={session.id}>
                        <button
                          type="button"
                          disabled={tmuxBusy}
                          title={`Attach to ${session.name}`}
                          onClick={() => void attachTmuxSession(session)}
                        >
                          <SquareTerminal size={15} />
                          <span>
                            <strong>{session.name}</strong>
                            <small>
                              {session.windows}{" "}
                              {session.windows === 1 ? "window" : "windows"}
                              {session.created == null
                                ? ""
                                : ` · ${formatTmuxAge(session.created)}`}
                            </small>
                          </span>
                          {session.attached && <i title="Attached">Attached</i>}
                        </button>
                        <button
                          className="icon-button ghost"
                          type="button"
                          aria-label={`Kill tmux session ${session.name}`}
                          title="Kill session"
                          disabled={tmuxKillingId != null}
                          onClick={() => void killTmuxSession(session)}
                        >
                          {tmuxKillingId === session.id ? (
                            <LoaderCircle className="spin" size={13} />
                          ) : (
                            <X size={13} />
                          )}
                        </button>
                      </div>
                    ))}
                  </div>
                )}
                <form onSubmit={(event) => void createTmuxSession(event)}>
                  <label>
                    <span>New session name</span>
                    <input
                      value={newTmuxName}
                      placeholder="Leave blank to auto-number"
                      onChange={(event) => {
                        setNewTmuxName(event.target.value);
                        setNewTmuxNameError(null);
                      }}
                    />
                  </label>
                  {newTmuxNameError && (
                    <div className="terminal-tmux-status error" role="alert">
                      {newTmuxNameError}
                    </div>
                  )}
                  <button
                    className="primary-button"
                    type="submit"
                    disabled={tmuxBusy || tmuxListing?.status === "unsupported-os"}
                  >
                    {tmuxBusy ? (
                      <LoaderCircle className="spin" size={15} />
                    ) : (
                      <Plus size={15} />
                    )}
                    Create &amp; attach
                  </button>
                </form>
              </div>
            )}
          </div>
          <TerminalSettingsPopover />
          <SnippetsPicker
            profileId={splitSourceSessionId}
            profileName={
              sessions.find((session) => session.id === splitSourceSessionId)
                ?.name ?? null
            }
            disabled={!isFocusedConnected}
            onSend={sendSnippet}
          />
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
                          {selectedOtherProfile.isLocal
                            ? "Local shell"
                            : `${selectedOtherProfile.username}@${selectedOtherProfile.host}`}
                        </small>
                      </span>
                    </div>
                    {!selectedOtherProfile.isLocal && (
                      <label>
                        <span>Password / key passphrase</span>
                        <input
                          autoFocus
                          type="password"
                          autoComplete="current-password"
                          value={otherSessionPassword}
                          placeholder={
                            otherSessionCredentialStored
                              ? "•••••••• (saved securely)"
                              : "Leave blank for key/agent auth"
                          }
                          onChange={(event) =>
                            setOtherSessionPassword(event.target.value)
                          }
                        />
                      </label>
                    )}
                    {!selectedOtherProfile.isLocal &&
                      selectedOtherProfile.proxyJumpId && (
                        <section
                          className="terminal-proxy-credentials"
                          aria-label="ProxyJump credentials"
                        >
                          <div className="terminal-proxy-credentials-title">
                            <KeyRound size={15} />
                            <span>
                              <strong>ProxyJump credentials</strong>
                              <small>
                                {selectedOtherProxyProfile
                                  ? `${selectedOtherProxyProfile.name} (${selectedOtherProxyProfile.username}@${selectedOtherProxyProfile.host})`
                                  : "Saved jump host"}
                              </small>
                            </span>
                          </div>
                          <label>
                            <span>Proxy password / key passphrase</span>
                            <input
                              type="password"
                              autoComplete="off"
                              value={otherProxyPassword}
                              placeholder={
                                otherProxyCredentialStored
                                  ? "•••••••• (saved securely)"
                                  : "Leave blank for key/agent auth"
                              }
                              onChange={(event) =>
                                setOtherProxyPassword(event.target.value)
                              }
                            />
                          </label>
                          <small className="terminal-proxy-credentials-hint">
                            Enter this when the jump host cannot authenticate
                            with its saved key or SSH agent.
                          </small>
                        </section>
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
                          {session.isLocal ? (
                            <Laptop size={15} />
                          ) : (
                            <Server size={15} />
                          )}
                          <span>
                            <strong>{session.name}</strong>
                            <small>
                              {session.isLocal
                                ? "Local shell"
                                : `${session.username}@${session.host}`}
                              {!session.isLocal &&
                                session.proxyJumpId &&
                                ` · via ${
                                  sessions.find(
                                    (candidate) =>
                                      candidate.id === session.proxyJumpId,
                                  )?.name ?? "jump host"
                                }`}
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
      <div
        className="terminal-surface"
        ref={surfaceRef}
        style={
          {
            "--terminal-bg": terminalAppearance.theme.xterm.background,
          } as CSSProperties
        }
      >
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
