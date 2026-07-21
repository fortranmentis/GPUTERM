import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { confirm } from "@tauri-apps/plugin-dialog";
import { CredentialVaultGate } from "./CredentialVaultGate";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  confirm: vi.fn(),
}));

const mockInvoke = vi.mocked(invoke);
const mockConfirm = vi.mocked(confirm);

describe("CredentialVaultGate", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockConfirm.mockReset();
  });

  it("does not offer unlock actions until vault status finishes loading", () => {
    mockInvoke.mockReturnValue(new Promise(() => undefined));
    render(<CredentialVaultGate />);

    expect(screen.getByRole("status")).toHaveTextContent(/checking local vault/i);
    expect(screen.queryByRole("button", { name: "Unlock" })).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Master password")).not.toBeInTheDocument();
  });

  it("creates a new vault without importing Keychain passwords", async () => {
    mockInvoke.mockImplementation((command) => {
      if (command === "get_credential_vault_status") {
        return Promise.resolve({ exists: false, unlocked: false, hasCredentials: false });
      }
      if (command === "initialize_credential_vault") {
        return Promise.resolve({ exists: true, unlocked: true, hasCredentials: false });
      }
      return Promise.resolve(undefined);
    });

    render(<CredentialVaultGate />);
    const dialog = await screen.findByRole("dialog", { name: "Create credential vault" });
    expect(dialog).toHaveTextContent(/Keychain passwords are not imported/i);
    fireEvent.change(screen.getByLabelText("Master password"), {
      target: { value: "correct horse battery staple" },
    });
    fireEvent.change(screen.getByLabelText("Confirm master password"), {
      target: { value: "correct horse battery staple" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Create vault" }));

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("initialize_credential_vault", {
        masterPassword: "correct horse battery staple",
      }),
    );
    await waitFor(() =>
      expect(screen.queryByRole("dialog", { name: "Create credential vault" }))
        .not.toBeInTheDocument(),
    );
  });

  it("validates master password length and confirmation before initialization", async () => {
    mockInvoke.mockResolvedValue({ exists: false, unlocked: false, hasCredentials: false });
    render(<CredentialVaultGate />);
    await screen.findByRole("dialog", { name: "Create credential vault" });

    fireEvent.change(screen.getByLabelText("Master password"), {
      target: { value: "short" },
    });
    fireEvent.change(screen.getByLabelText("Confirm master password"), {
      target: { value: "different" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Create vault" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(/at least 8/i);
    expect(
      mockInvoke.mock.calls.some(([command]) => command === "initialize_credential_vault"),
    ).toBe(false);
  });

  it("keeps the vault locked after a wrong password and unlocks on retry", async () => {
    let attempts = 0;
    mockInvoke.mockImplementation((command) => {
      if (command === "get_credential_vault_status") {
        return Promise.resolve({ exists: true, unlocked: false, hasCredentials: true });
      }
      if (command === "unlock_credential_vault") {
        attempts += 1;
        return attempts === 1
          ? Promise.reject("Incorrect master password or the credential vault is corrupted")
          : Promise.resolve({ exists: true, unlocked: true, hasCredentials: true });
      }
      return Promise.resolve(undefined);
    });

    render(<CredentialVaultGate />);
    await screen.findByRole("dialog", { name: "Unlock credential vault" });
    fireEvent.change(screen.getByLabelText("Master password"), {
      target: { value: "wrong password" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Unlock" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/incorrect master password/i);

    fireEvent.change(screen.getByLabelText("Master password"), {
      target: { value: "correct horse battery staple" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Unlock" }));
    await waitFor(() =>
      expect(screen.queryByRole("dialog", { name: "Unlock credential vault" }))
        .not.toBeInTheDocument(),
    );
    expect(attempts).toBe(2);
  });

  it("resets a forgotten vault only after confirmation", async () => {
    mockConfirm.mockResolvedValue(true);
    mockInvoke.mockImplementation((command) => {
      if (command === "get_credential_vault_status") {
        return Promise.resolve({ exists: true, unlocked: false, hasCredentials: true });
      }
      if (command === "reset_credential_vault") {
        return Promise.resolve({ exists: false, unlocked: false, hasCredentials: false });
      }
      return Promise.resolve(undefined);
    });

    render(<CredentialVaultGate />);
    await screen.findByRole("dialog", { name: "Unlock credential vault" });
    fireEvent.click(screen.getByRole("button", { name: "Reset vault" }));

    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith("reset_credential_vault"));
    expect(await screen.findByRole("dialog", { name: "Create credential vault" }))
      .toBeInTheDocument();
  });
});
