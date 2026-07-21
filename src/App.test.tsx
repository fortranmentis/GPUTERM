import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";

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
    } : []),
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
