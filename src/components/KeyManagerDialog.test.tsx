import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { confirm, save } from "@tauri-apps/plugin-dialog";
import {
  exportProblem,
  keyFileName,
  KeyManagerDialog,
  keyUsage,
  kindTitle,
} from "./KeyManagerDialog";
import { useKeyStore } from "../stores/keyStore";
import { useSessionStore } from "../stores/sessionStore";
import type { StoredKey } from "../types/keys";
import type { SessionProfile } from "../types/session";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ confirm: vi.fn(), save: vi.fn() }));

const mockInvoke = vi.mocked(invoke);

const profiles: SessionProfile[] = [
  { id: "p1", name: "lab-a100", host: "10.0.0.21", port: 22, username: "ubuntu" },
  { id: "p2", name: "bastion", host: "1.2.3.4", port: 22, username: "jump" },
];

const laptopKey: StoredKey = {
  id: "k1",
  name: "Lab laptop",
  kind: "ed25519",
  publicKey: "ssh-ed25519 AAAAC3Nza me@lab",
  fingerprint: "SHA256:abcdef",
  createdAt: 1_792_000_000,
  profileIds: ["p1"],
};

function renderWith(keys: StoredKey[]) {
  mockInvoke.mockImplementation((command) =>
    Promise.resolve(command === "list_keys" ? keys : undefined),
  );
  const onClose = vi.fn();
  render(<KeyManagerDialog onClose={onClose} />);
  return onClose;
}

describe("KeyManagerDialog", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    vi.mocked(confirm).mockReset();
    vi.mocked(save).mockReset();
    useKeyStore.setState({ keys: [], loaded: false });
    useSessionStore.setState({ sessions: profiles, message: null });
  });

  it("lists keys with their type and the servers using them", async () => {
    renderWith([laptopKey, { ...laptopKey, id: "k2", name: "Spare", profileIds: [] }]);
    const dialog = screen.getByRole("dialog", { name: "SSH keys" });
    expect(
      within(dialog).getByText(
        "Key pairs made on this computer. Private keys stay encrypted in GpuTerm's vault.",
      ),
    ).toBeInTheDocument();
    expect(await screen.findByText("Ed25519 · Used by lab-a100")).toBeInTheDocument();
    expect(screen.getByText("Ed25519 · Not used by a server yet")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /Lab laptop/ }));
    expect(screen.getByText("SHA256:abcdef")).toBeInTheDocument();
    expect(screen.getByLabelText("Public key")).toHaveTextContent("ssh-ed25519 AAAAC3Nza me@lab");
  });

  it("generates a key pair with the chosen type and comment", async () => {
    renderWith([]);
    await screen.findByText("No keys yet.");
    fireEvent.click(screen.getByRole("button", { name: "Generate key pair" }));
    const form = screen.getByRole("form", { name: "Generate key pair" });
    const generate = within(form).getByRole("button", { name: "Generate" });
    expect(generate).toBeDisabled();

    fireEvent.change(within(form).getByPlaceholderText("Lab laptop"), {
      target: { value: " Desk " },
    });
    fireEvent.change(within(form).getByRole("combobox"), { target: { value: "rsa-4096" } });
    expect(screen.getByText("RSA keys can take several seconds to generate.")).toBeInTheDocument();
    fireEvent.change(within(form).getByLabelText("Comment (optional)"), {
      target: { value: "me@desk" },
    });

    let finish: (keys: StoredKey[]) => void = () => undefined;
    mockInvoke.mockImplementation((command) =>
      command === "generate_key"
        ? new Promise((resolve) => {
            finish = resolve as (keys: StoredKey[]) => void;
          })
        : Promise.resolve([]),
    );
    fireEvent.click(generate);
    expect(await within(form).findByRole("button", { name: "Generating…" })).toBeDisabled();
    expect(mockInvoke).toHaveBeenCalledWith("generate_key", {
      name: "Desk",
      kind: "rsa-4096",
      comment: "me@desk",
    });
    finish([{ ...laptopKey, name: "Desk", kind: "rsa-4096", profileIds: [] }]);
    expect(await screen.findByText("Key pair Desk generated")).toBeInTheDocument();
    expect(screen.getByText("RSA 4096 · Not used by a server yet")).toBeInTheDocument();
  });

  it("shows why generation was refused, such as a locked vault", async () => {
    renderWith([]);
    await screen.findByText("No keys yet.");
    mockInvoke.mockRejectedValue("Unlock the credential vault with the GpuTerm master password first");
    fireEvent.click(screen.getByRole("button", { name: "Generate key pair" }));
    fireEvent.change(screen.getByPlaceholderText("Lab laptop"), { target: { value: "x" } });
    fireEvent.click(screen.getByRole("button", { name: "Generate" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Unlock the credential vault");
  });

  it("exports with a repeated passphrase, or without one only when asked", async () => {
    renderWith([laptopKey]);
    fireEvent.click(await screen.findByRole("button", { name: /Lab laptop/ }));
    fireEvent.click(screen.getByRole("button", { name: "Export private…" }));
    const exportDialog = screen.getByRole("dialog", { name: "Export Lab laptop" });
    const submit = within(exportDialog).getByRole("button", { name: "Choose file…" });
    expect(submit).toBeDisabled();

    const [first, second] = within(exportDialog).getAllByLabelText(/passphrase/i);
    fireEvent.change(first, { target: { value: "short" } });
    expect(within(exportDialog).getByText("Use at least 8 characters")).toBeInTheDocument();
    fireEvent.change(first, { target: { value: "correct horse" } });
    fireEvent.change(second, { target: { value: "correct hors" } });
    expect(within(exportDialog).getByText("The passphrases do not match")).toBeInTheDocument();
    expect(submit).toBeDisabled();

    fireEvent.click(within(exportDialog).getByRole("checkbox", { name: "Save without a passphrase" }));
    expect(
      within(exportDialog).getByText(/The file will hold the key in plain text/),
    ).toBeInTheDocument();
    expect(submit).toBeEnabled();

    vi.mocked(save).mockResolvedValue("/home/me/id_ed25519_Lab-laptop");
    fireEvent.click(submit);
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("export_private_key", {
        keyId: "k1",
        path: "/home/me/id_ed25519_Lab-laptop",
        passphrase: null,
      }),
    );
    expect(save).toHaveBeenCalledWith({ defaultPath: "id_ed25519_Lab-laptop" });
    expect(
      await screen.findByText("Private key exported to /home/me/id_ed25519_Lab-laptop"),
    ).toBeInTheDocument();
  });

  it("sends the passphrase when exporting encrypted", async () => {
    renderWith([laptopKey]);
    fireEvent.click(await screen.findByRole("button", { name: /Lab laptop/ }));
    fireEvent.click(screen.getByRole("button", { name: "Export private…" }));
    const [first, second] = screen.getAllByLabelText(/passphrase/i);
    fireEvent.change(first, { target: { value: "correct horse" } });
    fireEvent.change(second, { target: { value: "correct horse" } });
    vi.mocked(save).mockResolvedValue("/tmp/k");
    fireEvent.click(screen.getByRole("button", { name: "Choose file…" }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("export_private_key", {
        keyId: "k1",
        path: "/tmp/k",
        passphrase: "correct horse",
      }),
    );
  });

  it("deletes a key only after confirming", async () => {
    renderWith([laptopKey]);
    fireEvent.click(await screen.findByRole("button", { name: /Lab laptop/ }));

    vi.mocked(confirm).mockResolvedValue(false);
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(confirm).toHaveBeenCalledTimes(1));
    expect(confirm).toHaveBeenCalledWith(
      "The private key is erased from this computer. Servers keep it in authorized_keys until you remove it there. Used by lab-a100.",
      { title: "Delete Lab laptop?", kind: "warning", okLabel: "Delete" },
    );
    expect(mockInvoke).not.toHaveBeenCalledWith("delete_key", expect.anything());

    vi.mocked(confirm).mockResolvedValue(true);
    mockInvoke.mockResolvedValue([]);
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("delete_key", { keyId: "k1" }),
    );
    expect(await screen.findByText("No keys yet.")).toBeInTheDocument();
    expect(screen.getByText("Lab laptop deleted")).toBeInTheDocument();
  });

  it("renames a key and saves the public key file", async () => {
    renderWith([laptopKey]);
    fireEvent.click(await screen.findByRole("button", { name: /Lab laptop/ }));
    fireEvent.click(screen.getByRole("button", { name: "Rename" }));
    mockInvoke.mockResolvedValue([{ ...laptopKey, name: "Desk" }]);
    fireEvent.change(screen.getByLabelText("New name for Lab laptop"), {
      target: { value: "Desk" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save name" }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("rename_key", { keyId: "k1", name: "Desk" }),
    );

    vi.mocked(save).mockResolvedValue("/tmp/id_ed25519_Desk.pub");
    mockInvoke.mockResolvedValue(undefined);
    fireEvent.click(await screen.findByRole("button", { name: "Save .pub…" }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("save_public_key", {
        keyId: "k1",
        path: "/tmp/id_ed25519_Desk.pub",
      }),
    );
    expect(save).toHaveBeenCalledWith({ defaultPath: "id_ed25519_Desk.pub" });
  });

  it("closes from the button and with Escape", async () => {
    const onClose = renderWith([]);
    await screen.findByText("No keys yet.");
    fireEvent.click(screen.getByRole("button", { name: "Close SSH keys" }));
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(2);
  });

  it("formats types, usage, file names and export checks", () => {
    expect(kindTitle("ed25519")).toBe("Ed25519");
    expect(kindTitle("ecdsa-p384")).toBe("ECDSA P-384");
    expect(keyUsage({ ...laptopKey, profileIds: ["p1", "p2"] }, profiles)).toBe(
      "Used by 2 servers",
    );
    expect(keyUsage({ ...laptopKey, profileIds: ["gone"] }, profiles)).toBe(
      "Not used by a server yet",
    );
    expect(keyFileName({ kind: "rsa-3072", name: "lab a100 (가)" })).toBe("id_rsa_lab-a100");
    expect(keyFileName({ kind: "ecdsa-p521", name: "" })).toBe("id_ecdsa");
    expect(exportProblem("", "", true)).toBeNull();
    expect(exportProblem("correct horse", "correct horse", false)).toBeNull();
    expect(exportProblem("short", "short", false)).toBe("Use at least 8 characters");
  });
});
