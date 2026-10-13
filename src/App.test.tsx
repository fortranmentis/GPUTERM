import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { listen } from "@tauri-apps/api/event";
import App from "./App";
import { useSessionStore } from "./stores/sessionStore";
import type { AlertEvent } from "./types/alerts";

const storedPanelState = new Map<string, string>();
const localStorageMock: Storage = {
  get length() {
    return storedPanelState.size;
  },
  clear: () => storedPanelState.clear(),
  getItem: (key) => storedPanelState.get(key) ?? null,
  key: (index) => [...storedPanelState.keys()][index] ?? null,
  removeItem: (key) => {
    storedPanelState.delete(key);
  },
  setItem: (key, value) => {
    storedPanelState.set(key, String(value));
  },
};

vi.stubGlobal("localStorage", localStorageMock);

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn((command: string) =>
    Promise.resolve(command === "get_telemetry_settings" ? {
      telemetryIntervalSecs: 2,
      displayMode: "gpu-system",
      diskIgnoreFsTypes: [],
    } : command === "get_alert_settings" ? null : []),
  ),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => undefined)),
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    onCloseRequested: vi.fn(() => Promise.resolve(() => undefined)),
  }),
}));

vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getAllWebviewWindows: vi.fn(() => Promise.resolve([])),
}));

vi.mock("./components/AppMessage", () => ({
  AppMessageOverlay: () => null,
}));

vi.mock("./components/CredentialVaultGate", () => ({
  CredentialVaultGate: () => null,
}));

vi.mock("./components/SessionSidebar", () => ({
  SessionSidebar: () => <aside>Host selector</aside>,
}));

vi.mock("./components/TerminalPane", () => ({
  TerminalPane: () => <div>Terminal</div>,
}));

vi.mock("./components/SftpBrowser", () => ({
  SftpBrowser: ({ onClose }: { onClose: () => void }) => (
    <aside aria-label="SFTP panel">
      <button type="button" onClick={onClose}>Close SFTP panel</button>
    </aside>
  ),
}));

vi.mock("./components/RemoteTelemetryBar", () => ({
  RemoteTelemetryBar: ({ onClose }: { onClose: () => void }) => (
    <footer aria-label="Monitoring panel">
      <button type="button" onClick={onClose}>Close monitoring panel</button>
    </footer>
  ),
}));

describe("workspace panel visibility", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("closes and restores SFTP while expanding the terminal grid", () => {
    const { container } = render(<App />);

    fireEvent.click(screen.getByRole("button", { name: "Close SFTP panel" }));

    expect(screen.queryByRole("complementary", { name: "SFTP panel" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Open SFTP panel" })).toBeInTheDocument();
    expect(container.querySelector<HTMLElement>(".workspace-grid")?.style.gridTemplateColumns)
      .toBe("minmax(0, 1fr)");
    expect(localStorage.getItem("gputerm.sftpOpen")).toBe("false");

    fireEvent.click(screen.getByRole("button", { name: "Open SFTP panel" }));

    expect(screen.getByRole("complementary", { name: "SFTP panel" })).toBeInTheDocument();
    expect(localStorage.getItem("gputerm.sftpOpen")).toBe("true");
  });

  it("restores hidden panel states from local storage", () => {
    localStorage.setItem("gputerm.sftpOpen", "false");
    localStorage.setItem("gputerm.monitoringOpen", "false");

    render(<App />);

    expect(screen.getByRole("button", { name: "Open SFTP panel" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Open monitoring panel" })).toBeInTheDocument();
    expect(screen.queryByRole("contentinfo", { name: "Monitoring panel" })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Open monitoring panel" }));

    expect(screen.getByRole("contentinfo", { name: "Monitoring panel" })).toBeInTheDocument();
    expect(localStorage.getItem("gputerm.monitoringOpen")).toBe("true");
  });
});

describe("alerts", () => {
  const handlers = new Map<string, (event: { payload: unknown }) => void>();
  const alert: AlertEvent = {
    id: "1",
    scope: "s",
    hostLabel: "lab-a100",
    kind: "gpu-temp",
    subject: "GPU-a",
    state: "fired",
    value: 88,
    threshold: 85,
    at: 1_900_000_000,
    title: "GPU 0 (A100) is hot",
    body: "88°C (alert at 85°C)",
    notify: true,
    osDelivered: false,
  };

  beforeEach(() => {
    handlers.clear();
    vi.mocked(listen).mockImplementation(async (event, handler) => {
      handlers.set(event, handler as (event: { payload: unknown }) => void);
      return () => undefined;
    });
    useSessionStore.setState({ message: null });
  });

  async function fire(payload: AlertEvent) {
    await waitFor(() => expect(handlers.has("telemetry-alert")).toBe(true));
    act(() => handlers.get("telemetry-alert")!({ payload }));
  }

  it("shows an alert the OS did not display as a warning toast", async () => {
    render(<App />);
    await fire(alert);
    expect(useSessionStore.getState().message).toEqual({
      kind: "warning",
      text: "lab-a100 — GPU 0 (A100) is hot — 88°C (alert at 85°C)",
    });
  });

  it("stays quiet when the OS showed it or it is a repeat", async () => {
    render(<App />);
    await fire({ ...alert, osDelivered: true });
    await fire({ ...alert, id: "2", notify: false });
    expect(useSessionStore.getState().message).toBeNull();
  });

  it("never replaces an error the user has not acknowledged", async () => {
    render(<App />);
    act(() => useSessionStore.setState({ message: { kind: "error", text: "boom" } }));
    await fire(alert);
    expect(useSessionStore.getState().message).toEqual({ kind: "error", text: "boom" });
  });
});
