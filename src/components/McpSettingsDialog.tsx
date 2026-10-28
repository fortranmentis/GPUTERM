import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Check, Copy, Plug } from "lucide-react";
import { useSessionStore } from "../stores/sessionStore";
import type { McpSettings, McpStatus } from "../types/mcp";

/** How often the dialog refreshes the connected-client list. */
const STATUS_POLL_MS = 3_000;

type ClientId = "claude-code" | "claude-desktop" | "codex" | "cursor";

const CLIENTS: Array<{ id: ClientId; label: string; where: string }> = [
  { id: "claude-code", label: "Claude Code", where: "Run in a terminal:" },
  {
    id: "claude-desktop",
    label: "Claude Desktop",
    where: "Add to claude_desktop_config.json (Settings → Developer → Edit Config), then restart Claude:",
  },
  { id: "codex", label: "Codex", where: "Add to ~/.codex/config.toml:" },
  { id: "cursor", label: "Cursor", where: "Add to ~/.cursor/mcp.json:" },
];

/** The snippet that registers GpuTerm with an MCP client. */
export function clientConfig(client: ClientId, executable: string) {
  const json = JSON.stringify(
    { mcpServers: { gputerm: { command: executable, args: ["--mcp-stdio"] } } },
    null,
    2,
  );
  switch (client) {
    case "claude-code":
      return `claude mcp add --scope user gputerm -- "${executable}" --mcp-stdio`;
    case "codex":
      // A TOML literal string: Windows backslashes need no escaping.
      return `[mcp_servers.gputerm]\ncommand = '${executable}'\nargs = ["--mcp-stdio"]`;
    default:
      return json;
  }
}

export function McpSettingsDialog({ onClose }: { onClose: () => void }) {
  const sessions = useSessionStore((state) => state.sessions);
  const [status, setStatus] = useState<McpStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [client, setClient] = useState<ClientId>("claude-code");
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    let cancelled = false;
    const load = () =>
      invoke<McpStatus>("get_mcp_status")
        .then((next) => {
          if (!cancelled) setStatus(next);
        })
        .catch((loadError) => {
          if (!cancelled) setError(String(loadError));
        });
    void load();
    const timer = window.setInterval(() => void load(), STATUS_POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, []);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  const save = async (change: Partial<McpSettings>) => {
    if (!status) return;
    setSaving(true);
    setError(null);
    try {
      const next = await invoke<McpStatus>("update_mcp_settings", {
        settings: { ...status.settings, ...change },
      });
      setStatus(next);
      if (next.error) setError(next.error);
    } catch (saveError) {
      setError(String(saveError));
    } finally {
      setSaving(false);
    }
  };

  const toggleHost = (id: string, exposed: boolean) => {
    if (!status) return;
    const hidden = new Set(status.settings.hiddenProfiles);
    if (exposed) hidden.delete(id);
    else hidden.add(id);
    void save({ hiddenProfiles: [...hidden] });
  };

  const snippet = status?.executable ? clientConfig(client, status.executable) : "";
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(snippet);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch (copyError) {
      setError(`Could not copy: ${String(copyError)}`);
    }
  };

  const settings = status?.settings;
  const remoteSessions = sessions.filter((profile) => !profile.isLocal);
  const clientInfo = CLIENTS.find((entry) => entry.id === client)!;

  return (
    <div className="app-modal-backdrop" role="presentation">
      <div className="app-modal mcp-settings-dialog" role="dialog" aria-label="MCP server">
        <div className="app-modal-header">
          <Plug size={18} />
          <strong>MCP server</strong>
          {status && (
            <span className={`mcp-status ${status.running ? "on" : "off"}`}>
              {status.running
                ? status.clients.length > 0
                  ? `Connected: ${status.clients.join(", ")}`
                  : "Running, no clients"
                : "Off"}
            </span>
          )}
        </div>
        <p className="app-modal-text">
          Lets AI assistants such as Claude Code, Claude Desktop, Codex and Cursor read the
          telemetry, processes and containers of the hosts you have connected here. Clients reach
          GpuTerm only through this computer, and only while GpuTerm is open.
        </p>
        {!settings ? (
          <p className="app-modal-text">Loading…</p>
        ) : (
          <>
            <label className="mcp-toggle">
              <input
                type="checkbox"
                checked={settings.enabled}
                disabled={saving}
                onChange={(event) => void save({ enabled: event.target.checked })}
              />
              <span>Allow MCP clients</span>
            </label>
            <label className="mcp-toggle">
              <input
                type="checkbox"
                checked={settings.allowActions}
                disabled={saving}
                onChange={(event) => void save({ allowActions: event.target.checked })}
              />
              <span>
                Allow actions: run commands, read files and end processes.
                <small> Each one still asks you here first.</small>
              </span>
            </label>
            {remoteSessions.length > 0 && (
              <fieldset className="mcp-hosts">
                <legend>Hosts clients can see</legend>
                {remoteSessions.map((profile) => (
                  <label key={profile.id} className="mcp-toggle">
                    <input
                      type="checkbox"
                      checked={!settings.hiddenProfiles.includes(profile.id)}
                      disabled={saving}
                      onChange={(event) => toggleHost(profile.id, event.target.checked)}
                    />
                    <span>
                      {profile.name}
                      <small>
                        {" "}
                        {profile.username}@{profile.host}
                      </small>
                    </span>
                  </label>
                ))}
              </fieldset>
            )}
            {status?.executable && (
              <section className="mcp-clients" aria-label="Connect a client">
                <div className="mcp-client-tabs" role="tablist">
                  {CLIENTS.map((entry) => (
                    <button
                      key={entry.id}
                      type="button"
                      role="tab"
                      aria-selected={entry.id === client}
                      className={entry.id === client ? "selected" : undefined}
                      onClick={() => {
                        setClient(entry.id);
                        setCopied(false);
                      }}
                    >
                      {entry.label}
                    </button>
                  ))}
                </div>
                <small>{clientInfo.where}</small>
                <div className="mcp-snippet">
                  <pre aria-label={`${clientInfo.label} configuration`}>{snippet}</pre>
                  <button
                    className="icon-button"
                    type="button"
                    aria-label="Copy configuration"
                    title="Copy"
                    onClick={() => void copy()}
                  >
                    {copied ? <Check size={14} /> : <Copy size={14} />}
                  </button>
                </div>
              </section>
            )}
            <small className="mcp-audit">Every request is logged to {status?.auditLog}</small>
          </>
        )}
        {error && (
          <p className="update-error" role="alert">
            {error}
          </p>
        )}
        <div className="app-modal-actions">
          <button className="primary-button" type="button" onClick={onClose}>
            Done
          </button>
        </div>
      </div>
    </div>
  );
}
