import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { RemoteTelemetryBar } from "./RemoteTelemetryBar";
import { useSessionStore } from "../stores/sessionStore";
import type { DiskMetric, GpuMetric, RemoteTelemetry } from "../types/gpu";
import type { GpuDetailMetric, ResourceDetails } from "../types/resourceDetails";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

const mockInvoke = vi.mocked(invoke);

const resourceDetails: ResourceDetails = {
  cpu: {
    modelName: "Test CPU",
    usagePercent: 42,
    loadAvg1: 1.2,
    loadAvg5: 1.0,
    loadAvg15: 0.8,
    totalCores: 8,
    onlineCores: 8,
    avgClockGhz: 3.4,
    uptimeSeconds: 90061,
    logicalCoreUsagePercent: [40, 44],
    topProcesses: [],
  },
  memory: {
    totalMiB: 32768,
    usedMiB: 16384,
    availableMiB: 16384,
    freeMiB: 4096,
    buffersMiB: 512,
    cachedMiB: 8192,
    swapTotalMiB: 4096,
    swapUsedMiB: 0,
    swapFreeMiB: 4096,
    usagePercent: 50,
    topProcesses: [],
  },
  gpus: [
    {
      index: 0,
      name: "Test GPU",
      uuid: "GPU-test",
      driverVersion: "550.1",
      gpuUtilPercent: 70,
      memoryUtilPercent: 50,
      memoryTotalMiB: 24576,
      memoryUsedMiB: 12288,
      memoryFreeMiB: 12288,
      temperatureC: 65,
      powerDrawW: 200,
      powerLimitW: 300,
      fanSpeedPercent: 40,
      graphicsClockMHz: 1800,
      memoryClockMHz: 1200,
      pciBusId: "0000:01:00.0",
      persistenceMode: "Enabled",
      migMode: "Disabled",
      processes: [],
    },
  ],
  errors: {},
};

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

function gpuSummary(metric: GpuDetailMetric): GpuMetric {
  return {
    index: metric.index,
    name: metric.name,
    uuid: metric.uuid,
    driverVersion: metric.driverVersion ?? "",
    powerDrawW: metric.powerDrawW,
    powerLimitW: metric.powerLimitW,
    temperatureC: metric.temperatureC,
    gpuUtilPercent: metric.gpuUtilPercent,
    memUtilPercent: metric.memoryUtilPercent,
    memoryTotalMiB: metric.memoryTotalMiB,
    memoryUsedMiB: metric.memoryUsedMiB,
    memoryFreeMiB: metric.memoryFreeMiB,
  };
}

describe("RemoteTelemetryBar disk summary", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation((command) => {
      if (command === "get_resource_details") {
        return Promise.resolve(resourceDetails);
      }
      return Promise.resolve(undefined);
    });
    useSessionStore.setState({
      activeSessionId: "session-1",
      connected: true,
      remoteTelemetry: telemetry([]),
      telemetrySettings: {
        telemetryIntervalSecs: 2,
        displayMode: "gpu-system",
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

  it("opens CPU, RAM, and GPU detail popovers from compact summaries", async () => {
    render(<RemoteTelemetryBar />);

    fireEvent.click(screen.getByRole("button", { name: /cpu/i }));
    expect(await screen.findByRole("dialog", { name: /cpu details/i })).toBeInTheDocument();
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("get_resource_details", {
        sessionId: "session-1",
        resourceType: "cpu",
      }),
    );
    fireEvent.keyDown(document, { key: "Escape" });

    fireEvent.click(screen.getByRole("button", { name: /ram/i }));
    expect(await screen.findByRole("dialog", { name: /memory details/i })).toBeInTheDocument();
    fireEvent.keyDown(document, { key: "Escape" });

    fireEvent.click(screen.getByRole("button", { name: /gpu/i }));
    expect(await screen.findByRole("dialog", { name: /gpu details/i })).toBeInTheDocument();
  });

  it("closes a resource detail popover on outside click", async () => {
    render(<RemoteTelemetryBar />);

    fireEvent.click(screen.getByRole("button", { name: /cpu/i }));
    expect(await screen.findByRole("dialog", { name: /cpu details/i })).toBeInTheDocument();

    fireEvent.mouseDown(document.body);
    expect(screen.queryByRole("dialog", { name: /cpu details/i })).not.toBeInTheDocument();
  });

  it("shows unavailable reason when detail collection fails", async () => {
    mockInvoke.mockImplementation((command) => {
      if (command === "get_resource_details") {
        return Promise.resolve({
          cpu: null,
          memory: null,
          gpus: [],
          errors: { cpu: "Metrics unavailable: ps permission denied" },
        });
      }
      return Promise.resolve(undefined);
    });
    render(<RemoteTelemetryBar />);

    fireEvent.click(screen.getByRole("button", { name: /cpu/i }));

    expect(await screen.findByText("Metrics unavailable")).toBeInTheDocument();
    expect(screen.getByText(/ps permission denied/i)).toBeInTheDocument();
  });

  it("polls resource details on the configured telemetry interval", async () => {
    const detailCalls = () =>
      mockInvoke.mock.calls.filter(([command]) => command === "get_resource_details").length;
    useSessionStore.setState({
      telemetrySettings: {
        telemetryIntervalSecs: 5,
        displayMode: "gpu-system",
        diskIgnoreFsTypes: [],
      },
    });
    vi.useFakeTimers();
    try {
      render(<RemoteTelemetryBar />);
      fireEvent.click(screen.getByRole("button", { name: /cpu/i }));
      await act(async () => {
        await Promise.resolve();
      });
      expect(detailCalls()).toBe(1);

      await act(async () => {
        vi.advanceTimersByTime(3_000);
        await Promise.resolve();
      });
      expect(detailCalls()).toBe(1);

      await act(async () => {
        vi.advanceTimersByTime(2_000);
        await Promise.resolve();
      });
      expect(detailCalls()).toBe(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it("clears the previous GPU selection when the active session changes", async () => {
    const sessionOneGpu0 = {
      ...resourceDetails.gpus[0],
      name: "Session One GPU Zero",
      uuid: "GPU-session-one-zero",
    };
    const sessionOneGpu1 = {
      ...resourceDetails.gpus[0],
      index: 1,
      name: "Session One GPU One",
      uuid: "GPU-session-one-one",
      temperatureC: 77,
    };
    const sessionTwoGpu = {
      ...resourceDetails.gpus[0],
      index: 3,
      name: "Session Two GPU",
      uuid: "GPU-session-two",
      temperatureC: 61,
    };
    const sessionOneDetails = {
      ...resourceDetails,
      gpus: [sessionOneGpu0, sessionOneGpu1],
    };
    const sessionTwoDetails = {
      ...resourceDetails,
      gpus: [sessionTwoGpu],
    };
    mockInvoke.mockImplementation((command, args) => {
      if (command === "get_resource_details") {
        const sessionId = (args as { sessionId?: string } | undefined)?.sessionId;
        return Promise.resolve(sessionId === "session-2" ? sessionTwoDetails : sessionOneDetails);
      }
      return Promise.resolve(undefined);
    });
    useSessionStore.setState({
      activeSessionId: "session-1",
      remoteTelemetry: {
        ...telemetry([]),
        gpu: [gpuSummary(sessionOneGpu0), gpuSummary(sessionOneGpu1)],
      },
    });

    render(<RemoteTelemetryBar />);

    fireEvent.click(screen.getByRole("button", { name: /GPU0 Session One GPU Zero/i }));
    fireEvent.click(await screen.findByRole("tab", { name: "GPU1" }));
    expect(
      within(screen.getByRole("dialog", { name: /GPU details/i })).getByText("Session One GPU One"),
    ).toBeInTheDocument();

    fireEvent.keyDown(document, { key: "Escape" });
    fireEvent.click(screen.getByRole("button", { name: /GPU0 Session One GPU Zero/i }));
    await waitFor(() =>
      expect(
        within(screen.getByRole("dialog", { name: /GPU details/i })).getByText("Session One GPU One"),
      ).toBeInTheDocument(),
    );

    act(() => {
      useSessionStore.setState({
        activeSessionId: "session-2",
        remoteTelemetry: {
          ...telemetry([]),
          gpu: [gpuSummary(sessionTwoGpu)],
        },
      });
    });

    await waitFor(() =>
      expect(screen.queryByRole("dialog", { name: /GPU details/i })).not.toBeInTheDocument(),
    );
    fireEvent.click(screen.getByRole("button", { name: /GPU3 Session Two GPU/i }));

    const sessionTwoDialog = await screen.findByRole("dialog", { name: /GPU details/i });
    expect(within(sessionTwoDialog).getByText("Session Two GPU")).toBeInTheDocument();
    expect(within(sessionTwoDialog).queryByText("Session One GPU One")).not.toBeInTheDocument();
    expect(screen.queryByRole("tablist", { name: "GPU selector" })).not.toBeInTheDocument();
  });
});
