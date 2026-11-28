import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { RemoteFilePanel } from "./RemoteFilePanel";
import type { SftpEntry } from "../types/session";

const entries: SftpEntry[] = [
  { name: "zeta.log", path: "/data/zeta.log", type: "file", size: 2048, modifiedTime: 1_700_000_200 },
  { name: "alpha.txt", path: "/data/alpha.txt", type: "file", size: 512, modifiedTime: 1_700_000_100 },
  { name: "models", path: "/data/models", type: "directory", size: 0, modifiedTime: 1_700_000_000 },
];

function createProps(overrides: Partial<Parameters<typeof RemoteFilePanel>[0]> = {}) {
  return {
    connected: true,
    loading: false,
    path: "/data",
    pathDraft: "/data",
    entries,
    selectedPaths: [],
    dropActive: false,
    creatingFolder: false,
    newFolderName: "",
    onPathDraftChange: vi.fn(),
    onOpenPath: vi.fn(),
    onRefresh: vi.fn(),
    onGoUp: vi.fn(),
    onSelectEntry: vi.fn(),
    onToggleAll: vi.fn(),
    onSelectAll: vi.fn(),
    onOpenDirectory: vi.fn(),
    onPointerDragStart: vi.fn(),
    onConsumePointerDragClick: vi.fn(() => false),
    onDropLocalFiles: vi.fn(),
    onDragOverLocalFiles: vi.fn(),
    onDragLeaveLocalFiles: vi.fn(),
    onDropRemoteOnDirectory: vi.fn(),
    onPasteLocalFiles: vi.fn(),
    onBeginCreateFolder: vi.fn(),
    onNewFolderNameChange: vi.fn(),
    onCreateFolder: vi.fn(),
    onCancelCreateFolder: vi.fn(),
    renamingPath: null,
    renameDraft: "",
    onRenameDraftChange: vi.fn(),
    onConfirmRename: vi.fn(),
    onCancelRename: vi.fn(),
    ...overrides,
  };
}

describe("RemoteFilePanel", () => {
  it("renders disconnected empty state with disabled navigation", () => {
    render(<RemoteFilePanel {...createProps({ connected: false, entries: [] })} />);
    expect(screen.getByText("Disconnected")).toBeInTheDocument();
    expect(screen.getByText("No SFTP session")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Refresh directory" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Parent directory" })).toBeDisabled();
  });

  it("keeps directories first and sorts files when clicking column headers", () => {
    render(<RemoteFilePanel {...createProps()} />);
    const rowNames = () =>
      screen.getAllByRole("row").map((row) => row.querySelector(".file-name")?.textContent);

    // Default: name ascending, directory first
    expect(rowNames()).toEqual(["models", "alpha.txt", "zeta.log"]);

    // Click Size -> descending by default for size
    fireEvent.click(screen.getByRole("button", { name: "Sort by Size" }));
    expect(rowNames()).toEqual(["models", "zeta.log", "alpha.txt"]);

    // Click Size again -> ascending
    fireEvent.click(screen.getByRole("button", { name: "Sort by Size" }));
    expect(rowNames()).toEqual(["models", "alpha.txt", "zeta.log"]);
  });

  it("navigates paths, opens directories on double-click, and handles selection", () => {
    const props = createProps();
    render(<RemoteFilePanel {...props} />);

    fireEvent.click(screen.getByRole("button", { name: "Parent directory" }));
    expect(props.onGoUp).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("button", { name: "Open remote path" }));
    expect(props.onOpenPath).toHaveBeenCalledWith("/data");

    const modelsButton = screen.getByRole("button", { name: /models/ });
    fireEvent.doubleClick(modelsButton);
    expect(props.onOpenDirectory).toHaveBeenCalledWith("/data/models");

    fireEvent.click(screen.getByRole("checkbox", { name: "Select alpha.txt" }));
    expect(props.onSelectEntry).toHaveBeenCalledWith(
      entries[1],
      expect.objectContaining({ checkbox: true }),
      ["/data/models", "/data/alpha.txt", "/data/zeta.log"],
    );

    fireEvent.keyDown(screen.getByTestId("remote-drop-zone"), { key: "a", ctrlKey: true });
    expect(props.onSelectAll).toHaveBeenCalledWith([
      "/data/models",
      "/data/alpha.txt",
      "/data/zeta.log",
    ]);
  });

  it("supports inline folder creation and inline rename with Escape cancel", () => {
    const createPropsObj = createProps({ creatingFolder: true, newFolderName: "checkpoints" });
    const { rerender } = render(<RemoteFilePanel {...createPropsObj} />);

    const folderInput = screen.getByLabelText("New remote folder name");
    fireEvent.keyDown(folderInput, { key: "Escape" });
    expect(createPropsObj.onCancelCreateFolder).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("button", { name: "Confirm new folder" }));
    expect(createPropsObj.onCreateFolder).toHaveBeenCalledTimes(1);

    const renamePropsObj = createProps({
      renamingPath: "/data/alpha.txt",
      renameDraft: "beta.txt",
    });
    rerender(<RemoteFilePanel {...renamePropsObj} />);

    const renameInput = screen.getByLabelText("New name for alpha.txt");
    fireEvent.keyDown(renameInput, { key: "Escape" });
    expect(renamePropsObj.onCancelRename).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("button", { name: "Confirm rename" }));
    expect(renamePropsObj.onConfirmRename).toHaveBeenCalledTimes(1);
  });
});
