import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SshConfigImport } from "./SshConfigImport";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const mockInvoke = vi.mocked(invoke);

describe("SshConfigImport", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation(async (command) => {
      if (command === "preview_ssh_config") {
        return {
          sourcePath: "/home/me/.ssh/config",
          warnings: [],
          hosts: [
            { alias: "gpu1", host: "10.0.0.21", user: "alice", port: 22, warnings: [] },
            {
              alias: "db",
              host: "db.internal",
              user: null,
              port: 22,
              conflictProfileId: "saved",
              warnings: ["No User; the import uses the default user"],
            },
          ],
        };
      }
      if (command === "import_ssh_config") {
        return { created: 1, updated: 0, warnings: [], profiles: [] };
      }
      return undefined;
    });
  });

  it("selects new hosts and imports them", async () => {
    const onImported = vi.fn();
    render(<SshConfigImport onClose={vi.fn()} onImported={onImported} />);

    const gpu = await screen.findByRole("checkbox", { name: /gpu1/ });
    expect(gpu).toBeChecked();
    // Hosts matching a saved profile are opt-in.
    expect(screen.getByRole("checkbox", { name: /db/ })).not.toBeChecked();

    fireEvent.click(screen.getByRole("button", { name: /^import 1$/i }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("import_ssh_config", {
        request: { text: null, aliases: ["gpu1"], defaultUser: null },
      }),
    );
    expect(onImported).toHaveBeenCalled();
  });

  it("asks for a default user when a selected host has none", async () => {
    render(<SshConfigImport onClose={vi.fn()} onImported={vi.fn()} />);
    fireEvent.click(await screen.findByRole("checkbox", { name: /db/ }));
    const importButton = screen.getByRole("button", { name: /^import 2$/i });
    expect(importButton).toBeDisabled();
    fireEvent.change(screen.getByLabelText(/user for hosts without one/i), {
      target: { value: "ubuntu" },
    });
    expect(importButton).toBeEnabled();
  });
});
