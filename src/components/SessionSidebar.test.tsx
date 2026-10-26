import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { confirm } from "@tauri-apps/plugin-dialog";
import { SessionSidebar } from "./SessionSidebar";
import { useSessionStore } from "../stores/sessionStore";
import { useKeyStore } from "../stores/keyStore";
import type { SessionConnectRequest, SessionProfile } from "../types/session";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  confirm: vi.fn(),
}));

const mockInvoke = vi.mocked(invoke);
const mockConfirm = vi.mocked(confirm);

const savedProfile: SessionProfile = {
  id: "profile-1",
  name: "lab-a100",
  host: "10.0.0.21",
  port: 22,
  username: "ubuntu",
  privateKeyPath: null,
};

const bastionProfile: SessionProfile = {
  id: "profile-2",
  name: "bastion",
  host: "1.2.3.4",
  port: 22,
  username: "jump",
  privateKeyPath: null,
};

const localProfile: SessionProfile = {
  id: "local-1",
  name: "My Mac",
  host: "localhost",
  port: 0,
  username: "local",
  isLocal: true,
  privateKeyPath: null,
};

describe("SessionSidebar profile form", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation((command) => {
      if (command === "save_session") {
        return Promise.resolve([savedProfile]);
      }
      return Promise.resolve([]);
    });
    useSessionStore.setState({
      sessions: [savedProfile],
      activeSessionId: null,
      connectedSessionIds: [],
      terminalIdsBySession: {},
      message: null,
    });
  });

  it("shows profile fields only after New while keeping saved-session auth available", () => {
    render(<SessionSidebar />);

    expect(screen.queryByPlaceholderText("10.0.0.21")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: /lab-a100/i }));
    expect(screen.queryByPlaceholderText("10.0.0.21")).toBeNull();
    expect(screen.getByLabelText(/password \/ key passphrase/i)).toBeInTheDocument();
    expect(screen.getByText("lab-a100", { selector: ".selected-session-actions > span" }))
      .toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /^new$/i }));
    expect(screen.getByText("New profile")).toBeInTheDocument();
    expect(screen.getByPlaceholderText("10.0.0.21")).toHaveValue("");
  });

  it("opens the SSH key manager and offers key sign-in for a saved profile", async () => {
    useKeyStore.setState({ keys: [], loaded: false });
    render(<SessionSidebar />);

    fireEvent.click(screen.getByRole("button", { name: /lab-a100/i }));
    expect(await screen.findByRole("combobox", { name: /Sign in with key/ })).toHaveValue("");
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith("list_keys"));

    fireEvent.click(screen.getByRole("button", { name: "SSH keys" }));
    const dialog = screen.getByRole("dialog", { name: "SSH keys" });
    expect(within(dialog).getByRole("button", { name: "Generate key pair" })).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Close SSH keys" }));
    expect(screen.queryByRole("dialog", { name: "SSH keys" })).toBeNull();
  });

  it("sends a password entered for a saved session", async () => {
    mockInvoke.mockImplementation((command) => {
      if (command === "connect_terminal") {
        return Promise.resolve({
          sessionId: "profile-1",
          terminalId: "terminal-1",
          profile: savedProfile,
        });
      }
      if (command === "load_sessions") {
        return Promise.resolve([savedProfile]);
      }
      return Promise.resolve([]);
    });

    render(<SessionSidebar />);
    fireEvent.click(screen.getByRole("button", { name: /lab-a100/i }));
    fireEvent.change(screen.getByLabelText(/password \/ key passphrase/i), {
      target: { value: "saved-secret" },
    });
    fireEvent.click(screen.getByRole("button", { name: /^connect$/i }));

    await waitFor(() => {
      const connectCall = mockInvoke.mock.calls.find(
        ([command]) => command === "connect_terminal",
      );
      const request = (connectCall?.[1] as { request: SessionConnectRequest }).request;
      expect(request.password).toBe("saved-secret");
      expect(request.reuseStoredCredentials).toBe(true);
    });
  });

  it("shows a secure mask when the selected host already has a saved credential", async () => {
    mockInvoke.mockImplementation((command, args) => {
      if (command === "has_saved_credential") {
        return Promise.resolve(
          (args as { sessionId?: string } | undefined)?.sessionId === "profile-1",
        );
      }
      return Promise.resolve([]);
    });

    render(<SessionSidebar />);
    fireEvent.click(screen.getByRole("button", { name: /lab-a100/i }));

    const password = screen.getByLabelText(/password \/ key passphrase/i);
    await waitFor(() =>
      expect(password).toHaveAttribute(
        "placeholder",
        "•••••••• (saved securely)",
      ),
    );
    expect(password).toHaveValue("");
  });

  it("reports when a successful connection could not persist its password", async () => {
    mockInvoke.mockImplementation((command) => {
      if (command === "connect_terminal") {
        return Promise.resolve({
          sessionId: "profile-1",
          terminalId: "terminal-1",
          profile: savedProfile,
          credentialWarning: "vault locked",
        });
      }
      if (command === "load_sessions") {
        return Promise.resolve([savedProfile]);
      }
      return Promise.resolve([]);
    });

    render(<SessionSidebar />);
    fireEvent.click(screen.getByRole("button", { name: /lab-a100/i }));
    fireEvent.change(screen.getByLabelText(/password \/ key passphrase/i), {
      target: { value: "saved-secret" },
    });
    fireEvent.click(screen.getByRole("button", { name: /^connect$/i }));

    await waitFor(() => {
      expect(useSessionStore.getState().message).toMatchObject({
        kind: "info",
        text: expect.stringContaining("vault locked"),
      });
    });
  });

  it("connects a saved session immediately on double-click using memory credentials", async () => {
    mockInvoke.mockImplementation((command) => {
      if (command === "connect_terminal") {
        return Promise.resolve({
          sessionId: "profile-1",
          terminalId: "terminal-1",
          profile: savedProfile,
        });
      }
      if (command === "load_sessions") {
        return Promise.resolve([savedProfile]);
      }
      return Promise.resolve([]);
    });

    render(<SessionSidebar />);
    fireEvent.doubleClick(screen.getByRole("button", { name: /lab-a100/i }));

    await waitFor(() => {
      const connectCall = mockInvoke.mock.calls.find(
        ([command]) => command === "connect_terminal",
      );
      const request = (connectCall?.[1] as { request: SessionConnectRequest })
        .request;
      expect(request).toMatchObject({
        id: "profile-1",
        password: null,
        reuseStoredCredentials: true,
      });
    });
    expect(useSessionStore.getState().activeSessionId).toBe("profile-1");
  });

  it("opens a native local terminal from the New profile form", async () => {
    mockInvoke.mockImplementation((command) => {
      if (command === "connect_terminal") {
        return Promise.resolve({
          sessionId: localProfile.id,
          terminalId: "local-terminal-1",
          profile: localProfile,
        });
      }
      if (command === "load_sessions") {
        return Promise.resolve([savedProfile, localProfile]);
      }
      return Promise.resolve([]);
    });

    render(<SessionSidebar />);
    fireEvent.click(screen.getByRole("button", { name: /^new$/i }));
    fireEvent.click(screen.getByRole("checkbox", { name: /local host/i }));

    expect(screen.getByRole("textbox", { name: "Host" })).toHaveValue(
      "localhost",
    );
    expect(screen.getByRole("textbox", { name: "Host" })).toBeDisabled();
    expect(screen.queryByPlaceholderText("ubuntu")).not.toBeInTheDocument();
    expect(
      screen.queryByLabelText(/password \/ passphrase/i),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /^test$/i }),
    ).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /^connect$/i }));
    await waitFor(() => {
      const connectCall = mockInvoke.mock.calls.find(
        ([command]) => command === "connect_terminal",
      );
      const request = (connectCall?.[1] as { request: SessionConnectRequest })
        .request;
      expect(request).toMatchObject({
        name: "Local terminal",
        host: "localhost",
        port: 0,
        username: "local",
        isLocal: true,
        password: null,
        proxyJumpId: null,
      });
    });
    expect(useSessionStore.getState().terminalIdsBySession[localProfile.id]).toEqual([
      "local-terminal-1",
    ]);
  });

  it("saves a fresh profile with a new id after New", async () => {
    render(<SessionSidebar />);

    fireEvent.click(screen.getByRole("button", { name: /^new$/i }));
    fireEvent.change(screen.getByPlaceholderText("10.0.0.21"), {
      target: { value: "10.0.0.99" },
    });
    fireEvent.change(screen.getByPlaceholderText("ubuntu"), {
      target: { value: "sang" },
    });
    fireEvent.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() => {
      const saveCall = mockInvoke.mock.calls.find(
        ([command]) => command === "save_session",
      );
      expect(saveCall).toBeTruthy();
      const profile = (saveCall?.[1] as { profile: SessionProfile }).profile;
      expect(profile.host).toBe("10.0.0.99");
      expect(profile.id).not.toBe(savedProfile.id);
      expect(profile.id.length).toBeGreaterThan(0);
    });
  });

  it("prompts for an unknown host key with its type and retries after trust", async () => {
    mockConfirm.mockResolvedValue(true);
    let connectAttempts = 0;
    mockInvoke.mockImplementation((command) => {
      if (command === "connect_terminal") {
        connectAttempts += 1;
        if (connectAttempts === 1) {
          return Promise.reject(
            "UNKNOWN_HOST_KEY:aabbcc|ecdsa-sha2-nistp256|10.0.0.21:22",
          );
        }
        return Promise.resolve({
          sessionId: "profile-1",
          terminalId: "terminal-1",
          profile: savedProfile,
        });
      }
      return Promise.resolve([]);
    });

    render(<SessionSidebar />);
    fireEvent.click(screen.getByRole("button", { name: /lab-a100/i }));
    fireEvent.click(screen.getByRole("button", { name: /^connect$/i }));

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("trust_host_key", {
        host: "10.0.0.21",
        port: 22,
        keyType: "ecdsa-sha2-nistp256",
        fingerprint: "aabbcc",
      }),
    );
    await waitFor(() => expect(connectAttempts).toBe(2));
    expect(useSessionStore.getState().terminalIdsBySession["profile-1"]).toEqual([
      "terminal-1",
    ]);
    expect(mockConfirm.mock.calls[0][0]).toContain("ecdsa-sha2-nistp256");
  });

  it("handles two sequential unknown host keys (jump host then target)", async () => {
    mockConfirm.mockResolvedValue(true);
    let connectAttempts = 0;
    mockInvoke.mockImplementation((command) => {
      if (command === "connect_terminal") {
        connectAttempts += 1;
        if (connectAttempts === 1) {
          return Promise.reject("UNKNOWN_HOST_KEY:bast01|ssh-ed25519|bastion:22");
        }
        if (connectAttempts === 2) {
          return Promise.reject("UNKNOWN_HOST_KEY:targ02|ssh-ed25519|10.0.0.21:22");
        }
        return Promise.resolve({
          sessionId: "profile-1",
          terminalId: "terminal-1",
          profile: savedProfile,
        });
      }
      return Promise.resolve([]);
    });

    render(<SessionSidebar />);
    fireEvent.click(screen.getByRole("button", { name: /lab-a100/i }));
    fireEvent.click(screen.getByRole("button", { name: /^connect$/i }));

    await waitFor(() => expect(connectAttempts).toBe(3));
    const trustCalls = mockInvoke.mock.calls.filter(
      ([command]) => command === "trust_host_key",
    );
    expect(trustCalls).toHaveLength(2);
    expect(trustCalls[0][1]).toMatchObject({ host: "bastion", fingerprint: "bast01" });
    expect(trustCalls[1][1]).toMatchObject({ host: "10.0.0.21", fingerprint: "targ02" });
  });

  it("offers other saved profiles as jump hosts and sends the selection", async () => {
    useSessionStore.setState({
      sessions: [savedProfile, bastionProfile],
      activeSessionId: null,
      connectedSessionIds: [],
      terminalIdsBySession: {},
      message: null,
    });
    mockInvoke.mockImplementation((command) => {
      if (command === "connect_terminal") {
        return Promise.resolve({
          sessionId: "profile-1",
          terminalId: "terminal-1",
          profile: savedProfile,
        });
      }
      return Promise.resolve([]);
    });

    render(<SessionSidebar />);
    fireEvent.click(screen.getByRole("button", { name: /^new$/i }));
    fireEvent.change(screen.getByPlaceholderText("10.0.0.21"), {
      target: { value: "10.0.0.50" },
    });
    fireEvent.change(screen.getByPlaceholderText("ubuntu"), {
      target: { value: "worker" },
    });

    const jumpSelect = screen.getByRole("combobox", { name: /jump host/i });
    expect(
      within(jumpSelect).getByRole("option", { name: /bastion/i }),
    ).toBeInTheDocument();
    fireEvent.change(jumpSelect, { target: { value: "profile-2" } });
    // The jump-host password field appears only once a jump host is chosen.
    fireEvent.change(screen.getByLabelText(/jump host password/i), {
      target: { value: "bastion-pw" },
    });
    fireEvent.click(screen.getByRole("button", { name: /^connect$/i }));

    await waitFor(() => {
      const connectCall = mockInvoke.mock.calls.find(
        ([command]) => command === "connect_terminal",
      );
      const request = (connectCall?.[1] as { request: SessionConnectRequest }).request;
      expect(request.proxyJumpId).toBe("profile-2");
      expect(request.proxyJumpPassword).toBe("bastion-pw");
    });
  });

  it("shows the jump host name in the session list", () => {
    useSessionStore.setState({
      sessions: [{ ...savedProfile, proxyJumpId: "profile-2" }, bastionProfile],
      activeSessionId: null,
      connectedSessionIds: [],
      terminalIdsBySession: {},
      message: null,
    });

    render(<SessionSidebar />);
    expect(screen.getByText(/via bastion/i)).toBeInTheDocument();
  });

  it("saves a typed password along with a new profile", async () => {
    render(<SessionSidebar />);
    fireEvent.click(screen.getByRole("button", { name: /^new$/i }));
    fireEvent.change(screen.getByPlaceholderText("10.0.0.21"), {
      target: { value: "10.0.0.99" },
    });
    fireEvent.change(screen.getByPlaceholderText("ubuntu"), {
      target: { value: "sang" },
    });
    fireEvent.change(screen.getByLabelText(/^password \/ passphrase$/i), {
      target: { value: "pw" },
    });
    fireEvent.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith(
        "save_session",
        expect.objectContaining({ password: "pw", proxyJumpPassword: null }),
      ),
    );
  });
});

describe("SessionSidebar editing a saved profile", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation((command, args) => {
      if (command === "save_session") {
        return Promise.resolve([(args as { profile: SessionProfile }).profile]);
      }
      if (command === "has_saved_credential") {
        return Promise.resolve(true);
      }
      return Promise.resolve([]);
    });
    useSessionStore.setState({
      sessions: [savedProfile, bastionProfile],
      activeSessionId: null,
      connectedSessionIds: [],
      terminalIdsBySession: {},
      message: null,
    });
  });

  function openEditor() {
    render(<SessionSidebar />);
    fireEvent.click(screen.getByTitle(/(connect|show) lab-a100$/i));
    fireEvent.click(screen.getByRole("button", { name: "Edit lab-a100" }));
  }

  it("fills the form from the profile and keeps a stored password when left blank", async () => {
    openEditor();

    expect(screen.getByText("Edit profile")).toBeInTheDocument();
    expect(screen.getByLabelText("Host")).toHaveValue("10.0.0.21");
    expect(screen.getByRole("checkbox", { name: /local host/i })).toBeDisabled();
    await waitFor(() =>
      expect(screen.getByLabelText(/^password \/ passphrase$/i)).toHaveAttribute(
        "placeholder",
        expect.stringContaining("leave blank to keep"),
      ),
    );

    fireEvent.change(screen.getByLabelText("Host"), {
      target: { value: "10.0.0.22" },
    });
    fireEvent.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("save_session", {
        profile: expect.objectContaining({ id: "profile-1", host: "10.0.0.22" }),
        password: null,
        proxyJumpPassword: null,
      }),
    );
    expect(useSessionStore.getState().message).toEqual({
      kind: "success",
      text: "Session updated",
    });
    expect(screen.queryByText("Edit profile")).not.toBeInTheDocument();
  });

  it("explains that a connected profile changes only on the next connect", () => {
    useSessionStore.setState({ connectedSessionIds: ["profile-1"] });
    openEditor();

    expect(screen.getByText(/changes apply on the next connect/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^connect$/i })).toBeDisabled();
  });

  it("does not offer a jump host whose chain leads back to the profile", () => {
    useSessionStore.setState({
      sessions: [savedProfile, { ...bastionProfile, proxyJumpId: "profile-1" }],
    });
    openEditor();

    const jumpSelect = screen.getByRole("combobox", { name: /jump host/i });
    expect(within(jumpSelect).queryByRole("option", { name: /bastion/i })).toBeNull();
    expect(within(jumpSelect).queryByRole("option", { name: /lab-a100/i })).toBeNull();
  });

  it("returns to the profile on Cancel", () => {
    openEditor();
    fireEvent.change(screen.getByLabelText("Host"), {
      target: { value: "typo" },
    });
    fireEvent.click(screen.getByRole("button", { name: /cancel/i }));

    expect(screen.queryByText("Edit profile")).not.toBeInTheDocument();
    expect(
      screen.getByText("lab-a100", { selector: ".selected-session-actions > span" }),
    ).toBeInTheDocument();
    expect(mockInvoke).not.toHaveBeenCalledWith("save_session", expect.anything());
  });

  it("marks a profile that has a GpuTerm key", () => {
    useSessionStore.setState({
      sessions: [
        {
          ...savedProfile,
          managedKey: {
            publicKey: "ssh-ed25519 AAAA gputerm:x",
            fingerprint: "SHA256:x",
            createdAt: 1,
            installedAt: 1,
          },
        },
      ],
    });
    render(<SessionSidebar />);
    expect(screen.getByLabelText("GpuTerm key")).toBeInTheDocument();
  });
});
