import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  clearDirectorySizeCache,
  DIRECTORY_SIZE_CACHE_TTL_MS,
  DirectorySizesView,
} from "./DirectorySizesView";
import type { DirectorySizes } from "../types/dirSizes";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const mockInvoke = vi.mocked(invoke);

const GiB = 1024 ** 3;
const listings: Record<string, DirectorySizes> = {
  "/data": {
    path: "/data",
    totalBytes: 100 * GiB,
    filesBytes: 2 * GiB,
    entries: [
      { name: "datasets", path: "/data/datasets", bytes: 60 * GiB },
      { name: "checkpoints", path: "/data/checkpoints", bytes: 38 * GiB },
    ],
    moreEntries: 0,
    complete: true,
    unreadable: 0,
  },
  "/data/datasets": {
    path: "/data/datasets",
    totalBytes: null,
    filesBytes: null,
    entries: [{ name: "imagenet", path: "/data/datasets/imagenet", bytes: 50 * GiB }],
    moreEntries: 12,
    complete: false,
    unreadable: 2,
  },
};

describe("DirectorySizesView", () => {
  beforeEach(() => {
    clearDirectorySizeCache();
    mockInvoke.mockReset();
    mockInvoke.mockImplementation(async (_command, args) => {
      const { path } = args as { path: string };
      return listings[path];
    });
  });

  it("lists folders, drills in and steps back from the cache", async () => {
    const onBack = vi.fn();
    render(<DirectorySizesView sessionId="gpu" root="/data" onBack={onBack} />);
    expect(await screen.findByText("datasets")).toBeInTheDocument();
    expect(screen.getByText("Files in this folder")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /datasets/ }));
    expect(await screen.findByText("imagenet")).toBeInTheDocument();
    expect(screen.getByText(/Stopped after the time limit/)).toBeInTheDocument();
    expect(screen.getByText(/2 paths could not be read/)).toBeInTheDocument();
    expect(screen.getByText("and 12 smaller folders")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Up one folder" }));
    expect(await screen.findByText("checkpoints")).toBeInTheDocument();
    expect(mockInvoke).toHaveBeenCalledTimes(2);

    fireEvent.click(screen.getByRole("button", { name: "Back to disks" }));
    expect(onBack).toHaveBeenCalled();
  });

  it("rescans on request and opens a folder in SFTP", async () => {
    const onOpenInSftp = vi.fn();
    render(
      <DirectorySizesView sessionId="gpu" root="/data" onBack={() => {}} onOpenInSftp={onOpenInSftp} />,
    );
    await screen.findByText("datasets");
    fireEvent.click(screen.getByRole("button", { name: "Rescan this folder" }));
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledTimes(2));
    fireEvent.click(screen.getByRole("button", { name: "Open this folder in SFTP" }));
    expect(onOpenInSftp).toHaveBeenCalledWith("/data");
  });

  it("shows scan errors", async () => {
    mockInvoke.mockRejectedValue("A folder size scan is still running on this host.");
    render(<DirectorySizesView sessionId="gpu" root="/data" onBack={() => {}} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("still running");
  });

  it("expires cached entries after TTL and clears only the disconnected session", async () => {
    const nowSpy = vi.spyOn(Date, "now").mockReturnValue(1_000_000);
    try {
      const { unmount } = render(<DirectorySizesView sessionId="gpu" root="/data" onBack={() => {}} />);
      expect(await screen.findByText("datasets")).toBeInTheDocument();
      unmount();

      const second = render(<DirectorySizesView sessionId="other" root="/data" onBack={() => {}} />);
      expect(await screen.findByText("datasets")).toBeInTheDocument();
      second.unmount();
      expect(mockInvoke).toHaveBeenCalledTimes(2);

      // Clearing only "gpu" leaves "other" cached.
      clearDirectorySizeCache("gpu");
      const otherAgain = render(<DirectorySizesView sessionId="other" root="/data" onBack={() => {}} />);
      expect(await screen.findByText("datasets")).toBeInTheDocument();
      expect(mockInvoke).toHaveBeenCalledTimes(2);
      otherAgain.unmount();

      // Advancing past TTL forces a fresh scan for "other".
      nowSpy.mockReturnValue(1_000_000 + DIRECTORY_SIZE_CACHE_TTL_MS + 1);
      render(<DirectorySizesView sessionId="other" root="/data" onBack={() => {}} />);
      await waitFor(() => expect(mockInvoke).toHaveBeenCalledTimes(3));
    } finally {
      nowSpy.mockRestore();
    }
  });
});
