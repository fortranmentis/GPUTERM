import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { ProfileKeyRow } from "./ProfileKeyRow";
import { useKeyStore } from "../stores/keyStore";
import { useSessionStore } from "../stores/sessionStore";
import type { StoredKey } from "../types/keys";
import type { SessionProfile } from "../types/session";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ confirm: vi.fn(), save: vi.fn() }));

const mockInvoke = vi.mocked(invoke);

const profile: SessionProfile = {
  id: "p1",
  name: "lab-a100",
  host: "10.0.0.21",
  port: 22,
  username: "ubuntu",
};

const key: StoredKey = {
  id: "k1",
  name: "Lab laptop",
  kind: "ecdsa-p256",
  publicKey: "ecdsa-sha2-nistp256 AAAA me@lab",
  fingerprint: "SHA256:abc",
  createdAt: 1,
  profileIds: [],
};

describe("ProfileKeyRow", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    useKeyStore.setState({ keys: [], loaded: false });
    useSessionStore.setState({ sessions: [profile], message: null });
  });

  it("assigns a key, installs it, and can go back to none", async () => {
    mockInvoke.mockImplementation((command, args) => {
      if (command === "list_keys") return Promise.resolve([key]);
      if (command === "assign_key") {
        const { keyId } = args as { keyId: string | null };
        return Promise.resolve([{ ...key, profileIds: keyId ? ["p1"] : [] }]);
      }
      if (command === "install_key") return Promise.resolve([{ ...key, profileIds: ["p1"] }]);
      return Promise.resolve(undefined);
    });
    render(<ProfileKeyRow profile={profile} onManageKeys={vi.fn()} />);

    const select = screen.getByRole("combobox", { name: /Sign in with key/ });
    await screen.findByRole("option", { name: "Lab laptop (ECDSA P-256)" });
    expect(select).toHaveValue("");
    expect(screen.queryByRole("button", { name: "Install on server" })).toBeNull();

    fireEvent.change(select, { target: { value: "k1" } });
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("assign_key", { profileId: "p1", keyId: "k1" }),
    );
    expect(
      screen.getByText("Adds the public key to authorized_keys and checks it signs in."),
    ).toBeInTheDocument();

    fireEvent.click(await screen.findByRole("button", { name: "Install on server" }));
    await waitFor(() =>
      expect(useSessionStore.getState().message).toEqual({
        kind: "success",
        text: "Lab laptop installed on 10.0.0.21; GpuTerm signs in with it from now on",
      }),
    );
    expect(mockInvoke).toHaveBeenCalledWith("install_key", { profileId: "p1", keyId: "k1" });

    fireEvent.change(select, { target: { value: "" } });
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("assign_key", { profileId: "p1", keyId: null }),
    );
  });

  it("opens the key manager from the menu without assigning", async () => {
    mockInvoke.mockResolvedValue([key]);
    const onManageKeys = vi.fn();
    render(<ProfileKeyRow profile={profile} onManageKeys={onManageKeys} />);
    await screen.findByRole("option", { name: "Lab laptop (ECDSA P-256)" });
    fireEvent.change(screen.getByRole("combobox", { name: /Sign in with key/ }), {
      target: { value: "__manage__" },
    });
    expect(onManageKeys).toHaveBeenCalledOnce();
    expect(mockInvoke).not.toHaveBeenCalledWith("assign_key", expect.anything());
  });

  it("reports a failed install", async () => {
    useKeyStore.setState({ keys: [{ ...key, profileIds: ["p1"] }], loaded: true });
    mockInvoke.mockRejectedValue("The key was added to authorized_keys, but the server did not accept it.");
    render(<ProfileKeyRow profile={profile} onManageKeys={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "Install on server" }));
    await waitFor(() => expect(useSessionStore.getState().message?.kind).toBe("error"));
  });
});
