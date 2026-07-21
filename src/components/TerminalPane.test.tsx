import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { TerminalPane } from "./TerminalPane";
import { useSessionStore } from "../stores/sessionStore";
import type { SessionConnectRequest, SessionProfile } from "../types/session";

const terminalMocks = vi.hoisted(() => ({
  open: vi.fn(),
  refresh: vi.fn(),
  write: vi.fn(),
}));
const eventHandlers = vi.hoisted(
  () => new Map<string, (event: { payload: unknown }) => void>(),
);

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(
    (eventName: string, handler: (event: { payload: unknown }) => void) => {
      eventHandlers.set(eventName, handler);
      return Promise.resolve(() => eventHandlers.delete(eventName));
    },
  ),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  confirm: vi.fn(),
}));

vi.mock("@xterm/addon-fit", () => ({
  FitAddon: class {
    fit() {}
  },
}));

vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    cols = 100;
    rows = 30;
    element: HTMLElement | undefined;
    textarea = null;

    loadAddon() {}
    open(host: HTMLElement) {
      this.element = document.createElement("div");
      host.appendChild(this.element);
      terminalMocks.open(host);
    }
    write(data: string) {
      terminalMocks.write(data);
    }
    refresh(start: number, end: number) {
      terminalMocks.refresh(start, end);
    }
    dispose() {}
    focus() {}
    input() {}
    onData() {
      return { dispose() {} };
    }
  },
}));

const mockInvoke = vi.mocked(invoke);

const alpha: SessionProfile = {
  id: "alpha",
  name: "Alpha",
  host: "10.0.0.1",
  port: 22,
  username: "alice",
  privateKeyPath: null,
};

const beta: SessionProfile = {
  id: "beta",
  name: "Beta",
  host: "10.0.0.2",
  port: 2222,
  username: "bob",
  privateKeyPath: null,
};

describe("TerminalPane multi-session split", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockResolvedValue(undefined);
    terminalMocks.open.mockReset();
    terminalMocks.refresh.mockReset();
    terminalMocks.write.mockReset();
    eventHandlers.clear();
    vi.stubGlobal(
      "ResizeObserver",
      class {
        observe() {}
        disconnect() {}
      },
    );
    useSessionStore.setState({
      sessions: [alpha, beta],
      activeSessionId: "alpha",
      terminalViewRevision: 1,
      connectedSessionIds: ["alpha"],
      terminalIdsBySession: { alpha: ["terminal-alpha"] },
      message: null,
    });
  });

  it("mounts a newly connected terminal in the visible pane and replays early output", async () => {
    useSessionStore.setState({
      activeSessionId: null,
      terminalViewRevision: 0,
      connectedSessionIds: [],
      terminalIdsBySession: {},
    });

    render(<TerminalPane />);
    await waitFor(() =>
      expect(eventHandlers.has("terminal-output")).toBe(true),
    );

    act(() => {
      eventHandlers.get("terminal-output")?.({
        payload: {
          sessionId: "alpha",
          terminalId: "terminal-alpha",
          data: "welcome\r\n",
        },
      });
      useSessionStore.getState().addConnectedSession("alpha", "terminal-alpha");
    });

    expect(terminalMocks.open).not.toHaveBeenCalled();

    act(() => {
      useSessionStore.getState().showSession("alpha");
    });

    await waitFor(() => expect(terminalMocks.open).toHaveBeenCalledOnce());
    const host = terminalMocks.open.mock.calls[0][0] as HTMLElement;
    expect(host.closest(".terminal-split-cell-hidden")).toBeNull();
    expect(terminalMocks.write).toHaveBeenCalledWith("welcome\r\n");
  });

  it("keeps the original same-session split behavior", async () => {
    mockInvoke.mockImplementation((command) => {
      if (command === "create_terminal_split") {
        return Promise.resolve({
          sessionId: "alpha",
          terminalId: "terminal-alpha-2",
        });
      }
      return Promise.resolve(undefined);
    });

    render(<TerminalPane />);
    await waitFor(() => expect(terminalMocks.open).toHaveBeenCalledOnce());
    const originalTerminalElement = (
      terminalMocks.open.mock.calls[0][0] as HTMLElement
    ).firstElementChild;
    fireEvent.click(screen.getByRole("button", { name: "Split terminal" }));
    const splitOptions = screen.getByRole("dialog", {
      name: "Split terminal options",
    });
    fireEvent.click(
      within(splitOptions).getByRole("button", { name: "Split pane" }),
    );

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("create_terminal_split", {
        sessionId: "alpha",
        cols: 100,
        rows: 30,
      }),
    );
    await waitFor(() =>
      expect(
        screen.getAllByRole("group", { name: "Alpha terminal pane" }),
      ).toHaveLength(2),
    );
    expect(useSessionStore.getState().terminalIdsBySession.alpha).toEqual([
      "terminal-alpha",
      "terminal-alpha-2",
    ]);
    expect(originalTerminalElement).toBeInTheDocument();
    expect(
      originalTerminalElement?.closest(".terminal-split-cell-hidden"),
    ).toBeNull();
  });

  it("adds an already connected session beside the current session", async () => {
    useSessionStore.setState({
      connectedSessionIds: ["alpha", "beta"],
      terminalIdsBySession: {
        alpha: ["terminal-alpha"],
        beta: ["terminal-beta"],
      },
    });

    render(<TerminalPane />);
    fireEvent.click(
      screen.getByRole("button", { name: "Add another session" }),
    );
    const picker = screen.getByRole("dialog", { name: "Add another session" });
    fireEvent.click(within(picker).getByRole("button", { name: /Beta/i }));

    await waitFor(() => {
      expect(
        screen.getByRole("group", { name: "Alpha terminal pane" }),
      ).toBeInTheDocument();
      expect(
        screen.getByRole("group", { name: "Beta terminal pane" }),
      ).toBeInTheDocument();
    });
    expect(screen.getByText("2 sessions · 2 panes")).toBeInTheDocument();
    expect(useSessionStore.getState().activeSessionId).toBe("beta");
    expect(
      mockInvoke.mock.calls.some(([command]) => command === "connect_terminal"),
    ).toBe(false);
  });

  it("connects a saved session with its password and adds it beside the current one", async () => {
    mockInvoke.mockImplementation((command) => {
      if (command === "connect_terminal") {
        return Promise.resolve({
          sessionId: "beta",
          terminalId: "terminal-beta",
          profile: beta,
        });
      }
      return Promise.resolve(undefined);
    });

    render(<TerminalPane />);
    fireEvent.click(
      screen.getByRole("button", { name: "Add another session" }),
    );
    const picker = screen.getByRole("dialog", { name: "Add another session" });
    fireEvent.click(within(picker).getByRole("button", { name: /Beta/i }));
    fireEvent.change(screen.getByLabelText("Password / key passphrase"), {
      target: { value: "beta-secret" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add pane" }));

    await waitFor(() => {
      const connectCall = mockInvoke.mock.calls.find(
        ([command]) => command === "connect_terminal",
      );
      const request = (connectCall?.[1] as { request: SessionConnectRequest })
        .request;
      expect(request).toMatchObject({
        id: "beta",
        host: "10.0.0.2",
        port: 2222,
        username: "bob",
        password: "beta-secret",
        cols: 100,
        rows: 30,
      });
    });
    await waitFor(() =>
      expect(
        screen.getByRole("group", { name: "Beta terminal pane" }),
      ).toBeInTheDocument(),
    );
    expect(useSessionStore.getState().connectedSessionIds).toEqual([
      "alpha",
      "beta",
    ]);
  });

  it("places a new split below the focused pane with the selected size", async () => {
    mockInvoke.mockImplementation((command) => {
      if (command === "create_terminal_split") {
        return Promise.resolve({
          sessionId: "alpha",
          terminalId: "terminal-alpha-bottom",
        });
      }
      return Promise.resolve(undefined);
    });

    render(<TerminalPane />);
    fireEvent.click(screen.getByRole("button", { name: "Split terminal" }));
    const splitOptions = screen.getByRole("dialog", {
      name: "Split terminal options",
    });
    fireEvent.click(
      within(splitOptions).getByRole("button", {
        name: "Place new pane bottom",
      }),
    );
    fireEvent.change(
      within(splitOptions).getByRole("slider", { name: "New pane size" }),
      { target: { value: "35" } },
    );
    fireEvent.click(
      within(splitOptions).getByRole("button", { name: "Split pane" }),
    );

    const separator = await screen.findByRole("separator", {
      name: "Resize terminal panes",
    });
    expect(separator).toHaveAttribute("aria-orientation", "horizontal");
    expect(separator.parentElement).toHaveStyle({
      gridTemplateRows: "65fr 6px 35fr",
    });
    expect(
      screen.getAllByRole("group", { name: "Alpha terminal pane" }),
    ).toHaveLength(2);
  });
});
