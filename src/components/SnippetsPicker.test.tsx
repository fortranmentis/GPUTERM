import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SnippetsPicker } from "./SnippetsPicker";
import type { Snippet } from "../types/snippet";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const mockInvoke = vi.mocked(invoke);

const sampleSnippets: Snippet[] = [
  {
    id: "global-1",
    name: "GPU watch",
    command: "watch -n 1 nvidia-smi",
    appendReturn: true,
  },
  {
    id: "scoped-gpu",
    name: "Train job",
    command: "python train.py",
    appendReturn: true,
    profileIds: ["gpu-1", "gpu-2"],
  },
  {
    id: "scoped-other",
    name: "Other server only",
    command: "htop",
    appendReturn: true,
    profileIds: ["other-host"],
  },
];

describe("SnippetsPicker", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation(async (command) => {
      if (command === "list_snippets") {
        return sampleSnippets;
      }
      return [];
    });
  });

  it("loads snippets, filters by profile, and sends the clicked snippet", async () => {
    const onSend = vi.fn();
    render(
      <SnippetsPicker
        profileId="gpu-1"
        profileName="GPU Box"
        disabled={false}
        onSend={onSend}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Snippets" }));
    expect(await screen.findByText("GPU watch")).toBeInTheDocument();
    expect(screen.getByText("Train job")).toBeInTheDocument();
    expect(screen.queryByText("Other server only")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /^GPU watch/ }));
    expect(onSend).toHaveBeenCalledWith(sampleSnippets[0]);
    expect(screen.queryByRole("dialog", { name: "Snippets" })).not.toBeInTheDocument();
  });

  it("creates a new scoped snippet and updates the list", async () => {
    const created: Snippet = {
      id: "new-1",
      name: "Disk check",
      command: "df -h",
      appendReturn: false,
      profileIds: ["gpu-1"],
    };
    mockInvoke.mockImplementation(async (command) => {
      if (command === "list_snippets") return sampleSnippets;
      if (command === "save_snippet") return [...sampleSnippets, created];
      return [];
    });

    render(
      <SnippetsPicker
        profileId="gpu-1"
        profileName="GPU Box"
        disabled={false}
        onSend={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Snippets" }));
    await screen.findByText("GPU watch");

    fireEvent.click(screen.getByRole("button", { name: "New snippet" }));
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Disk check" } });
    fireEvent.change(screen.getByLabelText("Command"), { target: { value: "df -h" } });
    fireEvent.click(screen.getByLabelText("Press Return after sending"));
    fireEvent.click(screen.getByLabelText("Only for GPU Box"));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    expect(await screen.findByText("Disk check")).toBeInTheDocument();
    expect(mockInvoke).toHaveBeenCalledWith("save_snippet", {
      snippet: {
        id: "",
        name: "Disk check",
        command: "df -h",
        appendReturn: false,
        profileIds: ["gpu-1"],
      },
    });
  });

  it("preserves other server scopes when editing and deletes a snippet", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "list_snippets") return sampleSnippets;
      if (command === "save_snippet") return [sampleSnippets[0]];
      if (command === "delete_snippet") return [];
      return [];
    });

    render(
      <SnippetsPicker
        profileId="gpu-1"
        profileName="GPU Box"
        disabled={false}
        onSend={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Snippets" }));
    await screen.findByText("Train job");

    // Edit "Train job" (scoped to gpu-1 and gpu-2) and uncheck "Only for GPU Box".
    fireEvent.click(screen.getByRole("button", { name: "Edit snippet Train job" }));
    fireEvent.click(screen.getByLabelText("Only for GPU Box"));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("save_snippet", {
        snippet: {
          id: "scoped-gpu",
          name: "Train job",
          command: "python train.py",
          appendReturn: true,
          profileIds: ["gpu-2"],
        },
      }),
    );

    fireEvent.click(screen.getByRole("button", { name: "Delete snippet GPU watch" }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("delete_snippet", { id: "global-1" }),
    );
    expect(await screen.findByText(/No snippets yet/)).toBeInTheDocument();
  });

  it("shows errors and closes on Escape", async () => {
    mockInvoke.mockRejectedValue("Snippet storage unavailable");
    render(
      <SnippetsPicker
        profileId="gpu-1"
        profileName="GPU Box"
        disabled={false}
        onSend={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Snippets" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Snippet storage unavailable");

    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "Snippets" })).not.toBeInTheDocument();
  });
});
