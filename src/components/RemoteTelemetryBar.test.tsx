import { fireEvent, render, screen } from "@testing-library/react";
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

  it("opens disk detail popover and shows the full non-hidden mount list", () => {
    useSessionStore.setState({
      remoteTelemetry: telemetry([
        disk("/", 46),
        disk("/data", 43),
        disk("/mnt/storage", 39),
        { ...disk("/run", 1), fsType: "tmpfs" },
      ]),
    });

    render(<RemoteTelemetryBar />);

    fireEvent.click(screen.getByRole("button", { name: /disk/i }));

    expect(screen.getByRole("dialog", { name: /disk details/i })).toBeInTheDocument();
    expect(screen.getAllByText("/mnt/storage").length).toBeGreaterThan(0);
    expect(screen.queryByText("/run")).not.toBeInTheDocument();
  });

  it("closes disk detail popover with Escape or outside click", () => {
    useSessionStore.setState({
      remoteTelemetry: telemetry([disk("/", 46)]),
    });

    render(<RemoteTelemetryBar />);

    fireEvent.click(screen.getByRole("button", { name: /disk/i }));
    expect(screen.getByRole("dialog", { name: /disk details/i })).toBeInTheDocument();

    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.queryByRole("dialog", { name: /disk details/i })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /disk/i }));
    fireEvent.mouseDown(document.body);
    expect(screen.queryByRole("dialog", { name: /disk details/i })).not.toBeInTheDocument();
  });

  it("marks warning and critical disks in the detail popover", () => {
    useSessionStore.setState({
      remoteTelemetry: telemetry([disk("/warn", 82), disk("/critical", 93)]),
    });

    render(<RemoteTelemetryBar />);

    fireEvent.click(screen.getByRole("button", { name: /disk/i }));

    expect(document.querySelector('[data-usage-level="warning"]')).toBeTruthy();
    expect(document.querySelector('[data-usage-level="critical"]')).toBeTruthy();
  });
});
