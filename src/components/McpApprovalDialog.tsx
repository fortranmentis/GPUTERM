import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ShieldAlert } from "lucide-react";
import { useEffect, useState } from "react";
import type { McpApprovalRequest } from "../types/mcp";

const TOOL_TITLES: Record<string, string> = {
  run_command: "Run a command",
  read_file: "Read a file",
  kill_process: "End a process",
};

/**
 * Asks whether an MCP client may run an action (a command, a file read, a
 * process kill). One request at a time; Deny is the default, and a request
 * the backend gave up on is dropped.
 */
export function McpApprovalDialog() {
  const [queue, setQueue] = useState<McpApprovalRequest[]>([]);
  const current = queue[0] ?? null;

  useEffect(() => {
    let disposed = false;
    const unlisteners: Array<() => void> = [];
    const keep = (unlisten: () => void) => {
      if (disposed) unlisten();
      else unlisteners.push(unlisten);
    };
    void listen<McpApprovalRequest>("mcp-approval-request", (event) => {
      setQueue((pending) => [...pending, event.payload]);
    }).then(keep);
    void listen<string>("mcp-approval-closed", (event) => {
      setQueue((pending) => pending.filter((request) => request.requestId !== event.payload));
    }).then(keep);
    return () => {
      disposed = true;
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, []);

  const answer = (approved: boolean) => {
    if (!current) return;
    void invoke("answer_mcp_approval", { requestId: current.requestId, approved }).catch(
      () => undefined,
    );
    setQueue((pending) => pending.slice(1));
  };

  useEffect(() => {
    if (!current) return;
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") answer(false);
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  });

  if (!current) {
    return null;
  }

  return (
    <div className="app-modal-backdrop mcp-approval-backdrop" role="presentation">
      <div className="app-modal mcp-approval-dialog" role="alertdialog" aria-label="MCP request">
        <div className="app-modal-header">
          <ShieldAlert size={18} />
          <strong>{TOOL_TITLES[current.tool] ?? current.tool}?</strong>
        </div>
        <p className="app-modal-text">
          <strong>{current.client}</strong> asks to: {current.summary}
        </p>
        <pre className="mcp-approval-detail" aria-label="Requested action">
          {current.detail}
        </pre>
        <p className="mcp-approval-hint">
          Only allow it if you expected this request. The result is sent back to {current.client}.
          {queue.length > 1 && ` ${queue.length - 1} more waiting.`}
        </p>
        <div className="app-modal-actions">
          <button className="secondary-button" type="button" autoFocus onClick={() => answer(false)}>
            Deny
          </button>
          <button className="primary-button danger" type="button" onClick={() => answer(true)}>
            Allow
          </button>
        </div>
      </div>
    </div>
  );
}
