import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { confirm, save } from "@tauri-apps/plugin-dialog";
import {
  exportFileName,
  ManagedKeyPanel,
  passphraseProblem,
} from "./ManagedKeyPanel";
import { useSessionStore } from "../stores/sessionStore";
import type { SessionProfile } from "../types/session";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ confirm: vi.fn(), save: vi.fn() }));

const mockInvoke = vi.mocked(invoke);

const profile: SessionProfile = {
  id: "gpu",
  name: "lab a100",
  host: "10.0.0.21",
  port: 22,
  username: "ubuntu",
};
const withKey: SessionProfile = {
  ...profile,
  managedKey: {
    publicKey: "ssh-ed25519 AAAAC3Nza gputerm:lab-a100:2026-10-17",
    fingerprint: "SHA256:abcdef",
    createdAt: 1_792_000_000,
    installedAt: 1_792_000_000,
  },
};

function openPanel(target: SessionProfile) {
  render(<ManagedKeyPanel profile={target} />);
  fireEvent.click(screen.getByRole("button", { name: /ssh key/i }));
}

describe("ManagedKeyPanel", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    vi.mocked(confirm).mockReset();
    vi.mocked(save).mockReset();
    useSessionStore.setState({ sessions: [profile], message: null });
  });

  it("sets up a key and shows the updated profile", async () => {
    mockInvoke.mockResolvedValue([withKey]);
    openPanel(profile);

    fireEvent.click(screen.getByRole("button", { name: "Set up key" }));

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("setup_managed_key", { profileId: "gpu" }),
    );
    await waitFor(() => expect(useSessionStore.getState().sessions).toEqual([withKey]));
    expect(useSessionStore.getState().message?.kind).toBe("success");
  });

  it("reports a failed setup", async () => {
    mockInvoke.mockRejectedValue("The server did not accept the new key");
    openPanel(profile);
    fireEvent.click(screen.getByRole("button", { name: "Set up key" }));
    await waitFor(() =>
      expect(useSessionStore.getState().message).toEqual({
        kind: "error",
        text: "The server did not accept the new key",
      }),
    );
  });

  it("asks before replacing a key", async () => {
    vi.mocked(confirm).mockResolvedValue(false);
    openPanel(withKey);
    expect(screen.getByText("SHA256:abcdef")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Replace" }));
    await waitFor(() => expect(confirm).toHaveBeenCalled());
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it("exports only with a long enough, repeated passphrase", async () => {
    vi.mocked(save).mockResolvedValue("/home/me/id_gputerm");
    mockInvoke.mockResolvedValue(undefined);
    openPanel(withKey);
    fireEvent.click(screen.getByRole("button", { name: "Export…" }));

    const [first, second] = screen.getAllByLabelText(/passphrase/i);
    const submit = screen.getByRole("button", { name: "Choose file…" });
    fireEvent.change(first, { target: { value: "short" } });
    expect(submit).toBeDisabled();
    expect(screen.getByText("Use at least 8 characters")).toBeInTheDocument();
    fireEvent.change(first, { target: { value: "correct horse" } });
    fireEvent.change(second, { target: { value: "correct hors" } });
    expect(screen.getByText("The passphrases do not match")).toBeInTheDocument();
    fireEvent.change(second, { target: { value: "correct horse" } });
    fireEvent.click(submit);

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("export_managed_key", {
        profileId: "gpu",
        path: "/home/me/id_gputerm",
        passphrase: "correct horse",
      }),
    );
    expect(save).toHaveBeenCalledWith({ defaultPath: "id_ed25519_gputerm_lab-a100" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("does not export when no file is chosen", async () => {
    vi.mocked(save).mockResolvedValue(null);
    openPanel(withKey);
    fireEvent.click(screen.getByRole("button", { name: "Export…" }));
    const [first, second] = screen.getAllByLabelText(/passphrase/i);
    fireEvent.change(first, { target: { value: "correct horse" } });
    fireEvent.change(second, { target: { value: "correct horse" } });
    fireEvent.click(screen.getByRole("button", { name: "Choose file…" }));
    await waitFor(() => expect(save).toHaveBeenCalled());
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it("removes the key, from the server unless unticked", async () => {
    mockInvoke.mockResolvedValue([profile]);
    openPanel(withKey);
    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    fireEvent.click(screen.getByRole("checkbox", { name: /authorized_keys/ }));
    const dialog = screen.getByRole("alertdialog", { name: "Remove SSH key" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Remove" }));

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("remove_managed_key", {
        profileId: "gpu",
        removeFromServer: false,
      }),
    );
  });

  it("names exports after the profile and checks passphrases", () => {
    expect(exportFileName("lab a100 (가)")).toBe("id_ed25519_gputerm_lab-a100");
    expect(exportFileName("")).toBe("id_ed25519_gputerm");
    expect(passphraseProblem("12345678", "12345678")).toBeNull();
    expect(passphraseProblem("1234567", "1234567")).toMatch(/at least 8/);
  });
});
