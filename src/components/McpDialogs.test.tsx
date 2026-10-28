import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
const listeners = new Map<string, (event: { payload: unknown }) => void>();

vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invoke(...args) }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: (name: string, handler: (event: { payload: unknown }) => void) => {
    listeners.set(name, handler);
    return Promise.resolve(() => listeners.delete(name));
  },
}));

import { McpApprovalDialog } from "./McpApprovalDialog";
import { McpSettingsDialog, clientConfig } from "./McpSettingsDialog";
import { useSessionStore } from "../stores/sessionStore";
import type { McpApprovalRequest, McpStatus } from "../types/mcp";

function request(id: string, detail = "nvidia-smi"): McpApprovalRequest {
  return {
    requestId: id,
    client: "claude-code",
    tool: "run_command",
    host: "gpu-01",
    summary: "Run a command on gpu-01 as ubuntu (time limit 30s)",
    detail,
  };
}

async function emit(name: string, payload: unknown) {
  await waitFor(() => expect(listeners.has(name)).toBe(true));
  act(() => listeners.get(name)!({ payload }));
}

beforeEach(() => {
  invoke.mockReset();
  invoke.mockResolvedValue(undefined);
  listeners.clear();
});

describe("McpApprovalDialog", () => {
  it("shows requests one at a time and sends the answers", async () => {
    render(<McpApprovalDialog />);
    await emit("mcp-approval-request", request("r1"));
    await emit("mcp-approval-request", request("r2", "rm -rf /tmp/x"));

    const dialog = screen.getByRole("alertdialog", { name: "MCP request" });
    expect(within(dialog).getByText("Run a command?")).toBeInTheDocument();
    expect(within(dialog).getByLabelText("Requested action")).toHaveTextContent("nvidia-smi");
    expect(within(dialog).getByText(/1 more waiting/)).toBeInTheDocument();
    // Deny is the focused default.
    expect(within(dialog).getByRole("button", { name: "Deny" })).toHaveFocus();

    fireEvent.click(within(dialog).getByRole("button", { name: "Allow" }));
    expect(invoke).toHaveBeenCalledWith("answer_mcp_approval", { requestId: "r1", approved: true });
    expect(screen.getByLabelText("Requested action")).toHaveTextContent("rm -rf /tmp/x");

    fireEvent.keyDown(document, { key: "Escape" });
    expect(invoke).toHaveBeenCalledWith("answer_mcp_approval", { requestId: "r2", approved: false });
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
  });

  it("drops a request the backend stopped waiting for", async () => {
    render(<McpApprovalDialog />);
    await emit("mcp-approval-request", request("r1"));
    await emit("mcp-approval-closed", "r1");
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalled();
  });
});

describe("McpSettingsDialog", () => {
  const status = (overrides: Partial<McpStatus["settings"]> = {}, running = false): McpStatus => ({
    settings: { enabled: false, allowActions: false, hiddenProfiles: [], ...overrides },
    running,
    port: running ? 50123 : null,
    clients: running ? ["claude-code"] : [],
    executable: "/Applications/GpuTerm.app/Contents/MacOS/gputerm",
    auditLog: "/Users/me/Library/Application Support/GpuTerm/mcp-audit.log",
    error: null,
  });

  beforeEach(() => {
    useSessionStore.setState({
      sessions: [
        { id: "p1", name: "gpu-01", host: "10.0.0.1", port: 22, username: "ubuntu" },
        { id: "p2", name: "gpu-02", host: "10.0.0.2", port: 22, username: "ubuntu" },
      ],
    });
  });

  it("turns the server on and hides a host", async () => {
    invoke.mockImplementation((command: string, args?: { settings: McpStatus["settings"] }) =>
      Promise.resolve(
        command === "update_mcp_settings" ? status(args!.settings, args!.settings.enabled) : status(),
      ),
    );
    render(<McpSettingsDialog onClose={() => undefined} />);
    expect(await screen.findByText("Off")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("switch", { name: /Allow MCP clients/ }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("update_mcp_settings", {
        settings: { enabled: true, allowActions: false, hiddenProfiles: [] },
      }),
    );
    expect(await screen.findByText("Connected: claude-code")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("checkbox", { name: /gpu-02/ }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("update_mcp_settings", {
        settings: { enabled: true, allowActions: false, hiddenProfiles: ["p2"] },
      }),
    );
  });

  it("shows a ready-to-paste configuration per client", async () => {
    invoke.mockResolvedValue(status({ enabled: true }, true));
    render(<McpSettingsDialog onClose={() => undefined} />);
    expect(await screen.findByLabelText("Claude Code configuration")).toHaveTextContent(
      'claude mcp add --scope user gputerm -- "/Applications/GpuTerm.app/Contents/MacOS/gputerm" --mcp-stdio',
    );
    fireEvent.click(screen.getByRole("tab", { name: "Codex" }));
    expect(screen.getByLabelText("Codex configuration")).toHaveTextContent("[mcp_servers.gputerm]");
  });
});

describe("clientConfig", () => {
  it("keeps Windows paths valid in JSON and TOML", () => {
    const exe = "C:\\Program Files\\GpuTerm\\gputerm.exe";
    const desktop = JSON.parse(clientConfig("claude-desktop", exe));
    expect(desktop.mcpServers.gputerm).toEqual({ command: exe, args: ["--mcp-stdio"] });
    expect(clientConfig("codex", exe)).toContain(`command = '${exe}'`);
  });
});
