import { invoke } from "@tauri-apps/api/core";
import { AlertTriangle, XCircle } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import type {
  KillTarget,
  ProcessIdentity,
  SignalOutcome,
} from "../types/processControl";

/** How long to watch for the process to exit after a signal. */
const EXIT_WAIT_MS = 10_000;
const EXIT_POLL_MS = 1_000;

export function KillProcessButton({
  process,
  onKill,
}: {
  process: KillTarget;
  onKill: (target: KillTarget) => void;
}) {
  return (
    <button
      type="button"
      className="process-kill-button"
      aria-label={`End process ${process.pid}`}
      title="End process"
      onClick={() => onKill({ pid: process.pid, user: process.user, command: process.command })}
    >
      <XCircle size={13} />
    </button>
  );
}

/** The program name: the first word of a command line, without its path. */
function programName(command: string | null | undefined) {
  const first = command?.trim().split(/\s+/)[0] ?? "";
  return first.split(/[\\/]/).pop()?.toLowerCase() ?? "";
}

/**
 * True when the live process runs something other than what the table row
 * showed: the PID was reused, or the row was stale.
 */
export function commandMismatch(rowCommand: string | null, liveCommand: string | null) {
  const row = programName(rowCommand);
  const live = programName(liveCommand);
  if (!row || !live) {
    return false;
  }
  // Rows sometimes hold only the short process name.
  return row !== live && !live.startsWith(row) && !row.startsWith(live);
}

type Phase = "inspecting" | "ready" | "working" | "done";

const OUTCOME_TEXT: Record<string, string> = {
  gone: "The process has already exited.",
  identity_changed:
    "This PID now belongs to a different process, so nothing was sent. Refresh the list and try again.",
};

export function KillProcessDialog({
  sessionId,
  target,
  onClose,
  onDone,
}: {
  sessionId: string;
  target: KillTarget;
  onClose: () => void;
  /** Called once the process is gone, so the owner can refresh its list. */
  onDone: () => void;
}) {
  const [phase, setPhase] = useState<Phase>("inspecting");
  const [identity, setIdentity] = useState<ProcessIdentity | null>(null);
  const [notice, setNotice] = useState<{ tone: "info" | "error"; text: string } | null>(null);
  const [forceOffered, setForceOffered] = useState(false);
  const disposed = useRef(false);

  useEffect(() => {
    disposed.current = false;
    invoke<ProcessIdentity>("inspect_process", { sessionId, pid: target.pid })
      .then((found) => {
        if (disposed.current) return;
        setIdentity(found);
        setPhase(found.alive ? "ready" : "done");
        if (!found.alive) {
          setNotice({
            tone: "info",
            text: "This process no longer exists, or this login cannot see it (for example, it runs inside a container).",
          });
        }
      })
      .catch((error) => {
        if (disposed.current) return;
        setPhase("done");
        setNotice({ tone: "error", text: String(error) });
      });
    return () => {
      disposed.current = true;
    };
  }, [sessionId, target.pid]);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        onClose();
      }
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  /** Whether the inspected process is still the one running at this PID. */
  const stillRunning = useCallback(async () => {
    const now = await invoke<ProcessIdentity>("inspect_process", { sessionId, pid: target.pid });
    return now.alive && now.startMarker === identity?.startMarker;
  }, [identity?.startMarker, sessionId, target.pid]);

  const waitForExit = useCallback(async () => {
    const deadline = Date.now() + EXIT_WAIT_MS;
    while (Date.now() < deadline) {
      await new Promise((resolve) => setTimeout(resolve, EXIT_POLL_MS));
      if (disposed.current) return true;
      try {
        if (!(await stillRunning())) return true;
      } catch {
        // Keep waiting; a single failed check says nothing about the process.
      }
    }
    return false;
  }, [stillRunning]);

  const send = async (signal: "term" | "kill") => {
    if (!identity?.startMarker) return;
    setPhase("working");
    setNotice(null);
    try {
      const outcome = await invoke<SignalOutcome>("signal_process", {
        sessionId,
        pid: target.pid,
        signal,
        expectedStartMarker: identity.startMarker,
      });
      if (disposed.current) return;
      if (outcome.status === "sent") {
        if (await waitForExit()) {
          if (disposed.current) return;
          setPhase("done");
          setNotice({ tone: "info", text: `Process ${target.pid} has ended.` });
          onDone();
          return;
        }
        setPhase("ready");
        setForceOffered(true);
        setNotice({
          tone: "info",
          text: `It is still running after ${EXIT_WAIT_MS / 1000} seconds. You can force it to stop.`,
        });
        return;
      }
      if (outcome.status === "needs_force") {
        setPhase("ready");
        setForceOffered(true);
        setNotice({ tone: "info", text: "This program only stops when forced." });
        return;
      }
      if (outcome.status === "gone") {
        onDone();
      }
      setPhase(outcome.status === "permission_denied" || outcome.status === "failed" ? "ready" : "done");
      setNotice({
        tone: outcome.status === "gone" ? "info" : "error",
        text: OUTCOME_TEXT[outcome.status] ?? outcome.message ?? "The process could not be ended.",
      });
    } catch (error) {
      if (disposed.current) return;
      setPhase("ready");
      setNotice({ tone: "error", text: String(error) });
    }
  };

  const mismatch = identity?.alive && commandMismatch(target.command, identity.command);
  const command = identity?.command ?? target.command ?? "—";

  return createPortal(
    <div className="app-modal-backdrop" data-keep-popover role="presentation" onClick={onClose}>
      <div
        className="app-modal kill-process-dialog"
        role="alertdialog"
        aria-label={`End process ${target.pid}`}
        onClick={(event) => event.stopPropagation()}
      >
        <div className="app-modal-header">
          <AlertTriangle size={18} />
          <strong>End process {target.pid}?</strong>
        </div>
        <dl className="kill-process-facts">
          <dt>User</dt>
          <dd>{identity?.user ?? target.user ?? "—"}</dd>
          <dt>Command</dt>
          <dd title={command}>{command}</dd>
        </dl>
        {phase === "inspecting" && <p className="app-modal-text">Checking the process…</p>}
        {mismatch && (
          <p className="kill-process-warning" role="status">
            This PID now runs a different program than the list showed.
          </p>
        )}
        {notice && (
          <p className={`kill-process-notice ${notice.tone}`} role={notice.tone === "error" ? "alert" : "status"}>
            {notice.text}
          </p>
        )}
        <div className="app-modal-actions">
          {phase !== "done" && (
            <>
              <button
                className="primary-button danger"
                type="button"
                disabled={phase !== "ready"}
                onClick={() => void send("term")}
              >
                {phase === "working" ? "Waiting…" : "End process"}
              </button>
              {forceOffered && (
                <button
                  className="secondary-button danger"
                  type="button"
                  disabled={phase !== "ready"}
                  onClick={() => void send("kill")}
                >
                  Force stop
                </button>
              )}
            </>
          )}
          <button className="secondary-button" type="button" autoFocus onClick={onClose}>
            {phase === "done" ? "Close" : "Cancel"}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
