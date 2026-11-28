import { fireEvent, render, screen } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { clearDirectorySizeCache } from "./DirectorySizesView";
import { DiskDetailContent, DiskUsagePopover } from "./DiskUsagePopover";
import type { DiskMetric } from "../types/gpu";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const mockInvoke = vi.mocked(invoke);
const GiB = 1024 ** 3;

const disks: DiskMetric[] = [
  {
    filesystem: "/dev/nvme0n1p1",
    fsType: "ext4",
    mountPoint: "/data",
    totalBytes: 500 * GiB,
    usedBytes: 425 * GiB,
    availableBytes: 75 * GiB,
    usagePercent: 85,
  },
  {
    filesystem: "tmpfs",
    fsType: "tmpfs",
    mountPoint: "/run",
    totalBytes: 16 * GiB,
    usedBytes: 1 * GiB,
    availableBytes: 15 * GiB,
    usagePercent: 6.25,
  },
];

describe("DiskUsagePopover", () => {
  beforeEach(() => {
    clearDirectorySizeCache();
    mockInvoke.mockReset();
    mockInvoke.mockResolvedValue({
      path: "/data",
      totalBytes: 100 * GiB,
      filesBytes: 10 * GiB,
      entries: [{ name: "checkpoints", path: "/data/checkpoints", bytes: 90 * GiB }],
      moreEntries: 0,
      complete: true,
      unreadable: 0,
    });
  });

  it("hides ignored filesystems by default and reveals them when toggled", () => {
    render(
      <DiskDetailContent
        disks={disks}
        thermal={null}
        ignoredFsTypes={["tmpfs"]}
      />,
    );
    expect(screen.getByText("/data")).toBeInTheDocument();
    expect(screen.queryByText("/run")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Show folder sizes/ })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("checkbox", { name: "Show hidden filesystems" }));
    expect(screen.getByText("/run")).toBeInTheDocument();
  });

  it("shows an empty state when no visible disks exist", () => {
    render(
      <DiskDetailContent
        disks={[disks[1]]}
        thermal={null}
        ignoredFsTypes={["tmpfs"]}
      />,
    );
    expect(screen.getByText("Disk metrics unavailable")).toBeInTheDocument();
  });

  it("drills into DirectorySizesView on click or keyboard activation and returns on back", async () => {
    const anchor = document.createElement("button");
    document.body.appendChild(anchor);
    const onOpenInSftp = vi.fn();

    render(
      <DiskUsagePopover
        disks={disks}
        thermal={null}
        ignoredFsTypes={["tmpfs"]}
        anchorRef={{ current: anchor }}
        onClose={() => {}}
        sessionId="gpu-1"
        onOpenInSftp={onOpenInSftp}
      />,
    );

    const mountRow = screen.getByRole("button", { name: "Show folder sizes in /data" });
    fireEvent.keyDown(mountRow, { key: "Enter" });

    expect(await screen.findByText("checkpoints")).toBeInTheDocument();
    expect(mockInvoke).toHaveBeenCalledWith("directory_sizes", {
      sessionId: "gpu-1",
      path: "/data",
    });

    fireEvent.click(screen.getByRole("button", { name: "Back to disks" }));
    expect(screen.getByText("/dev/nvme0n1p1")).toBeInTheDocument();
    anchor.remove();
  });
});
