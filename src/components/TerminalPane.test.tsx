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
  findNext: vi.fn(),
  findPrevious: vi.fn(),
  dataHandlers: [] as Array<(data: string) => void>,
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

vi.mock("@xterm/addon-search", () => ({
  SearchAddon: class {
    findNext(query: string) {
      terminalMocks.findNext(query);
      return true;
    }
    findPrevious(query: string) {
      terminalMocks.findPrevious(query);
      return true;
    }
    clearDecorations() {}
    onDidChangeResults() {
      return { dispose() {} };
    }
    dispose() {}
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
    onData(handler: (data: string) => void) {
      terminalMocks.dataHandlers.push(handler);
      return {
        dispose() {
          const index = terminalMocks.dataHandlers.indexOf(handler);
          if (index >= 0) {
            terminalMocks.dataHandlers.splice(index, 1);
          }
        },
      };
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

const bastion: SessionProfile = {
  id: "bastion",
  name: "Bastion",
  host: "10.0.0.10",
  port: 22,
  username: "jump-user",
  privateKeyPath: null,
};

const local: SessionProfile = {
  id: "local",
  name: "Local terminal",
  host: "localhost",
  port: 0,
  username: "local",
  isLocal: true,
  privateKeyPath: null,
};

describe("TerminalPane multi-session split", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockResolvedValue(undefined);
    terminalMocks.open.mockReset();
    terminalMocks.refresh.mockReset();
    terminalMocks.write.mockReset();
    terminalMocks.findNext.mockReset();
    terminalMocks.findPrevious.mockReset();
    terminalMocks.dataHandlers.length = 0;
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

  it("serializes simultaneous key input before invoking the SSH writer", async () => {
    let releaseFirstWrite: (() => void) | undefined;
    const firstWrite = new Promise<void>((resolve) => {
      releaseFirstWrite = resolve;
    });
    let terminalWriteCount = 0;
    mockInvoke.mockImplementation((command) => {
      if (command !== "terminal_write") {
        return Promise.resolve(undefined);
      }
      terminalWriteCount += 1;
      return terminalWriteCount === 1 ? firstWrite : Promise.resolve(undefined);
    });

    render(<TerminalPane />);
    await waitFor(() => expect(terminalMocks.dataHandlers).toHaveLength(1));

    act(() => {
      terminalMocks.dataHandlers[0]("a");
      terminalMocks.dataHandlers[0]("s");
    });

    const terminalWriteCalls = () =>
      mockInvoke.mock.calls.filter(([command]) => command === "terminal_write");
    expect(terminalWriteCalls()).toHaveLength(1);
    expect(terminalWriteCalls()[0]).toEqual([
      "terminal_write",
      { terminalId: "terminal-alpha", data: "a" },
    ]);

    releaseFirstWrite?.();
    await waitFor(() => expect(terminalWriteCalls()).toHaveLength(2));
    expect(terminalWriteCalls()[1]).toEqual([
      "terminal_write",
      { terminalId: "terminal-alpha", data: "s" },
    ]);
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

  it("asks for ProxyJump credentials and sends them when adding a saved session", async () => {
    const proxiedBeta = { ...beta, proxyJumpId: bastion.id };
    useSessionStore.setState({ sessions: [alpha, proxiedBeta, bastion] });
    mockInvoke.mockImplementation((command) => {
      if (command === "connect_terminal") {
        return Promise.resolve({
          sessionId: "beta",
          terminalId: "terminal-beta",
          profile: proxiedBeta,
        });
      }
      return Promise.resolve(undefined);
    });

    render(<TerminalPane />);
    fireEvent.click(
      screen.getByRole("button", { name: "Add another session" }),
    );
    const picker = screen.getByRole("dialog", { name: "Add another session" });
    expect(within(picker).getByText(/via Bastion/i)).toBeInTheDocument();
    fireEvent.click(within(picker).getByRole("button", { name: /Beta/i }));

    const proxyCredentials = screen.getByRole("region", {
      name: "ProxyJump credentials",
    });
    expect(
      within(proxyCredentials).getByText(/jump-user@10\.0\.0\.10/i),
    ).toBeInTheDocument();
    fireEvent.change(
      within(proxyCredentials).getByLabelText(
        "Proxy password / key passphrase",
      ),
      { target: { value: "jump-secret" } },
    );
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
        password: "beta-secret",
        proxyJumpId: "bastion",
        proxyJumpPassword: "jump-secret",
        reuseStoredCredentials: true,
      });
    });
  });

  it("shows secure masks for saved target and ProxyJump credentials", async () => {
    const proxiedBeta = { ...beta, proxyJumpId: bastion.id };
    useSessionStore.setState({ sessions: [alpha, proxiedBeta, bastion] });
    mockInvoke.mockImplementation((command, args) => {
      if (command === "has_saved_credential") {
        const id = (args as { sessionId?: string } | undefined)?.sessionId;
        return Promise.resolve(id === "beta" || id === "bastion");
      }
      return Promise.resolve(undefined);
    });

    render(<TerminalPane />);
    fireEvent.click(screen.getByRole("button", { name: "Add another session" }));
    fireEvent.click(
      within(screen.getByRole("dialog", { name: "Add another session" })).getByRole(
        "button",
        { name: /Beta/i },
      ),
    );

    const targetPassword = screen.getByLabelText("Password / key passphrase");
    const proxyPassword = screen.getByLabelText("Proxy password / key passphrase");
    await waitFor(() => {
      expect(targetPassword).toHaveAttribute(
        "placeholder",
        "•••••••• (saved securely)",
      );
      expect(proxyPassword).toHaveAttribute(
        "placeholder",
        "•••••••• (saved securely)",
      );
    });
    expect(targetPassword).toHaveValue("");
    expect(proxyPassword).toHaveValue("");
  });

  it("adds a saved local terminal without showing SSH credentials", async () => {
    useSessionStore.setState({ sessions: [alpha, local] });
    mockInvoke.mockImplementation((command) => {
      if (command === "connect_terminal") {
        return Promise.resolve({
          sessionId: local.id,
          terminalId: "terminal-local",
          profile: local,
        });
      }
      return Promise.resolve(undefined);
    });

    render(<TerminalPane />);
    fireEvent.click(
      screen.getByRole("button", { name: "Add another session" }),
    );
    const picker = screen.getByRole("dialog", { name: "Add another session" });
    fireEvent.click(
      within(picker).getByRole("button", { name: /Local terminal/i }),
    );
    expect(
      screen.queryByLabelText("Password / key passphrase"),
    ).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Add pane" }));

    await waitFor(() => {
      const connectCall = mockInvoke.mock.calls.find(
        ([command]) => command === "connect_terminal",
      );
      const request = (connectCall?.[1] as { request: SessionConnectRequest })
        .request;
      expect(request).toMatchObject({
        id: "local",
        host: "localhost",
        port: 0,
        isLocal: true,
        password: null,
        proxyJumpId: null,
      });
    });
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
  it("shows the reported close reason and reconnects the pane", async () => {
    mockInvoke.mockImplementation((command) => {
      if (command === "connect_terminal") {
        return Promise.resolve({
          sessionId: "alpha",
          terminalId: "terminal-alpha-2",
          profile: alpha,
        });
      }
      return Promise.resolve(undefined);
    });

    render(<TerminalPane />);
    await waitFor(() => expect(eventHandlers.has("terminal-closed")).toBe(true));

    act(() => {
      eventHandlers.get("terminal-closed")?.({
        payload: {
          sessionId: "alpha",
          terminalId: "terminal-alpha",
          sessionClosed: true,
          message: "Remote shell closed",
        },
      });
    });

    // The backend already computes this reason; before, it was discarded and the
    // pane just stopped accepting input.
    expect(await screen.findByText("Remote shell closed")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /reconnect/i }));

    await waitFor(() => {
      const connectCall = mockInvoke.mock.calls.find(
        ([command]) => command === "connect_terminal",
      );
      const request = (connectCall?.[1] as { request: SessionConnectRequest }).request;
      expect(request).toMatchObject({ id: "alpha", reuseStoredCredentials: true });
    });
    await waitFor(() =>
      expect(screen.queryByText("Remote shell closed")).not.toBeInTheDocument(),
    );
  });

  it("searches terminal output from the Ctrl+F bar", async () => {
    render(<TerminalPane />);
    await waitFor(() => expect(terminalMocks.open).toHaveBeenCalled());

    expect(
      screen.queryByRole("searchbox", { name: /search terminal output/i }),
    ).toBeNull();

    fireEvent.keyDown(window, { key: "f", ctrlKey: true });

    const input = await screen.findByRole("searchbox", {
      name: /search terminal output/i,
    });
    fireEvent.change(input, { target: { value: "error" } });
    expect(terminalMocks.findNext).toHaveBeenCalledWith("error");

    fireEvent.keyDown(input, { key: "Enter", shiftKey: true });
    expect(terminalMocks.findPrevious).toHaveBeenCalledWith("error");

    fireEvent.keyDown(input, { key: "Escape" });
    await waitFor(() =>
      expect(
        screen.queryByRole("searchbox", { name: /search terminal output/i }),
      ).toBeNull(),
    );
  });

  it("leaves Ctrl+F to a text field the user is typing in", async () => {
    render(<TerminalPane />);
    await waitFor(() => expect(terminalMocks.open).toHaveBeenCalled());

    const field = document.createElement("input");
    document.body.appendChild(field);
    field.focus();
    fireEvent.keyDown(field, { key: "f", ctrlKey: true });

    expect(
      screen.queryByRole("searchbox", { name: /search terminal output/i }),
    ).toBeNull();
    field.remove();
  });
});
