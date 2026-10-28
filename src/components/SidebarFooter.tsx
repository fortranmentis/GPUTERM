import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Plug, RefreshCw } from "lucide-react";
import { useUpdateStore } from "../stores/updateStore";
import { McpSettingsDialog } from "./McpSettingsDialog";
import type { McpStatus } from "../types/mcp";

/** The app version, the update check and the MCP switch, at the bottom of the sidebar. */
export function SidebarFooter() {
  const currentVersion = useUpdateStore((state) => state.currentVersion);
  const phase = useUpdateStore((state) => state.phase);
  const update = useUpdateStore((state) => state.update);
  const checkForUpdates = useUpdateStore((state) => state.checkForUpdates);
  const openDialog = useUpdateStore((state) => state.openDialog);
  const pending = update != null && (phase === "available" || phase === "ready");
  const [mcpOpen, setMcpOpen] = useState(false);
  const [mcpRunning, setMcpRunning] = useState(false);

  const refreshMcp = useCallback(() => {
    invoke<McpStatus>("get_mcp_status")
      .then((status) => setMcpRunning(Boolean(status?.running)))
      .catch(() => undefined);
  }, []);
  useEffect(refreshMcp, [refreshMcp]);

  return (
    <div className="sidebar-footer">
      <span className="sidebar-version">{currentVersion ? `v${currentVersion}` : "GpuTerm"}</span>
      <button
        className={`sidebar-update-button${mcpRunning ? " mcp-on" : ""}`}
        type="button"
        title={mcpRunning ? "MCP server is on" : "MCP server is off"}
        onClick={() => setMcpOpen(true)}
      >
        <Plug size={12} />
        MCP
      </button>
      {pending ? (
        <button
          className="sidebar-update-button available"
          type="button"
          title={`GpuTerm ${update.version} is available`}
          onClick={openDialog}
        >
          <span className="update-dot" aria-hidden="true" />
          Update to {update.version}
        </button>
      ) : (
        <button
          className="sidebar-update-button"
          type="button"
          disabled={phase === "checking" || phase === "downloading"}
          onClick={() => void checkForUpdates(true)}
        >
          <RefreshCw size={12} className={phase === "checking" ? "spin" : undefined} />
          Check for updates
        </button>
      )}
      {mcpOpen && (
        <McpSettingsDialog
          onClose={() => {
            setMcpOpen(false);
            refreshMcp();
          }}
        />
      )}
    </div>
  );
}
