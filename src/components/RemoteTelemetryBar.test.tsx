import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { RemoteTelemetryBar } from "./RemoteTelemetryBar";
import { useSessionStore } from "../stores/sessionStore";
import type { DiskMetric, RemoteTelemetry } from "../types/gpu";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

function disk(mountPoint: string, usagePercent: number | null): DiskMetric {
  return {
    filesystem: `/dev/${mountPoint.replace(/\W/g, "") || "root"}`,
    fsType: "ext4",
    mountPoint,
    totalBytes: 100,
    usedBytes: 50,
    availableBytes: 50,
    usagePercent,
  };
}

function telemetry(disks: DiskMetric[]): RemoteTelemetry {
  return {
    timestamp: "2026-06-16T00:00:00.000Z",
    hostname: "lab",
    cpu: null,
    memory: null,
    disks,
    gpu: [],
    errors: {},
  };
}

describe("RemoteTelemetryBar disk summary", () => {
  beforeEach(() => {
    useSessionStore.setState({
      connected: true,
      remoteTelemetry: telemetry([]),
      telemetrySettings: {
        telemetryIntervalSecs: 2,
        displayMode: "system-only",
        diskIgnoreFsTypes: ["tmpfs"],
      },
      message: null,
    });
  });

  it("renders at most two mount points and hidden count", () => {
    useSessionStore.setState({
      remoteTelemetry: telemetry([
        disk("/mnt/storage", 39),
        disk("/", 46),
        disk("/data", 43),
        disk("/media/backup", 70),
      ]),
    });

    render(<RemoteTelemetryBar />);

    expect(screen.getByText("/")).toBeInTheDocument();
    expect(screen.getByText("46%")).toBeInTheDocument();
    expect(screen.getByText("/data")).toBeInTheDocument();
    expect(screen.getByText("43%")).toBeInTheDocument();
    expect(screen.getByText("+2")).toBeInTheDocument();
    expect(screen.queryByText("/mnt/storage")).not.toBeInTheDocument();
  });

  it("renders ? when usage percent is null", () => {
    useSessionStore.setState({
      remoteTelemetry: telemetry([disk("/", null)]),
    });

    render(<RemoteTelemetryBar />);

    expect(screen.getByText("?")).toBeInTheDocument();
  });
});
