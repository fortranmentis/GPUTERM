import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { RemoteTelemetryBar } from "./RemoteTelemetryBar";
import { useSessionStore } from "../stores/sessionStore";
import type { AgentMetric, DiskMetric, GpuMetric, RemoteTelemetry } from "../types/gpu";
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

function telemetry(disks: DiskMetric[], sessionId = "session-1"): RemoteTelemetry {
  return {
    sessionId,
    timestamp: "2026-06-16T00:00:00.000Z",
    hostname: "lab",
    cpu: null,
    memory: null,
    disks,
    gpu: [],
    users: [],
    agents: [],
    errors: {},
  };
}

const codexAgent: AgentMetric = {
  provider: "codex",
  displayName: "Codex",
  status: "active",
  rootPid: 4242,
  processCount: 3,
  user: "tester",
  cpuPercent: 12.5,
  memoryBytes: 512 * 1024 * 1024,
  elapsedSeconds: 600,
  sessionId: "session-codex",
  cwd: "/workspace",
  model: "gpt-test",
  inputTokens: 1000,
  outputTokens: 200,
  totalTokens: 1200,
  contextUsedTokens: 500,
  contextWindowTokens: 10000,
  contextUsedPercent: 5,
  contextRemainingTokens: 9500,
  contextRemainingPercent: 95,
  lastRequestInputTokens: null,
  lastRequestOutputTokens: null,
  lastRequestCacheCreationTokens: null,
  lastRequestCacheReadTokens: null,
  costUsd: null,
  sessionDurationSeconds: null,
  snapshotAgeSeconds: null,
  quota: {
    status: "available",
    source: "codex-app-server",
    capturedAt: 1_800_000_000,
    snapshotAgeSeconds: 0,
    message: null,
    history: [],
    limits: [
      {
        label: "primary",
        group: null,
        modelNames: [],
        remainingPercent: 60,
        usedPercent: 40,
        windowMinutes: 7 * 24 * 60,
        resetsAt: null,
        stale: false,
      },
    ],
  },
  subagents: [],
  backgroundTasks: [],
};

const agyAgent: AgentMetric = {
  ...codexAgent,
  provider: "agy",
  displayName: "AGY",
  rootPid: 4343,
  sessionId: "session-agy",
  model: "Gemini 2.5 Pro",
  contextRemainingTokens: 7500,
  contextRemainingPercent: 75,
  quota: {
    status: "available",
    source: "agy-usage-tui",
    capturedAt: 1_800_000_000,
    snapshotAgeSeconds: 0,
    message: null,
    history: [],
    limits: [
      {
        label: "weekly_limit",
        group: "Gemini models",
        modelNames: ["Gemini Flash", "Gemini Pro"],
        remainingPercent: 99.95,
        usedPercent: 0.05,
        windowMinutes: 7 * 24 * 60,
        resetsAt: null,
        stale: false,
      },
      {
        label: "five_hour_limit",
        group: "Gemini models",
        modelNames: ["Gemini Flash", "Gemini Pro"],
        remainingPercent: 99.4,
        usedPercent: 0.6,
        windowMinutes: 5 * 60,
        resetsAt: null,
        stale: false,
      },
      {
        label: "weekly_limit",
        group: "Claude and GPT models",
        modelNames: ["Claude Opus", "Claude Sonnet", "GPT-OSS"],
        remainingPercent: 100,
        usedPercent: 0,
        windowMinutes: 7 * 24 * 60,
        resetsAt: null,
        stale: false,
      },
      {
        label: "five_hour_limit",
        group: "Claude and GPT models",
        modelNames: ["Claude Opus", "Claude Sonnet", "GPT-OSS"],
        remainingPercent: 100,
        usedPercent: 0,
        windowMinutes: 5 * 60,
        resetsAt: null,
        stale: false,
      },
    ],
  },
};

const claudeAgent: AgentMetric = {
  ...codexAgent,
  provider: "claude",
  displayName: "Claude Code",
  rootPid: 4444,
  sessionId: "session-claude",
  model: "Claude Sonnet",
  inputTokens: null,
  outputTokens: null,
  totalTokens: null,
  contextRemainingTokens: 5000,
  contextRemainingPercent: 50,
  lastRequestInputTokens: 8500,
  lastRequestOutputTokens: 1200,
  lastRequestCacheCreationTokens: 5000,
  lastRequestCacheReadTokens: 2000,
  quota: {
    status: "available",
    source: "claude-statusline",
    capturedAt: 1_800_000_000,
    snapshotAgeSeconds: 0,
    message: null,
    history: [],
    limits: [
      {
        label: "five_hour",
        group: null,
        modelNames: [],
        remainingPercent: 80,
        usedPercent: 20,
        windowMinutes: 5 * 60,
        resetsAt: null,
        stale: false,
      },
      {
        label: "seven_day",
        group: null,
        modelNames: [],
        remainingPercent: 60,
        usedPercent: 40,
        windowMinutes: 7 * 24 * 60,
        resetsAt: null,
        stale: false,
      },
    ],
  },
};

function setTelemetry(payload: RemoteTelemetry) {
  useSessionStore.setState({
    telemetryBySession: { [payload.sessionId]: payload },
  });
}

function gpuSummary(metric: GpuDetailMetric): GpuMetric {
  return {
    index: metric.index,
    name: metric.name,
    uuid: metric.uuid,
    vendor: "nvidia",
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
      connectedSessionIds: ["session-1"],
      telemetryBySession: { "session-1": telemetry([]) },
      telemetrySettings: {
        telemetryIntervalSecs: 2,
        displayMode: "gpu-system",
        diskIgnoreFsTypes: ["tmpfs"],
      },
      message: null,
    });
  });

  it("renders at most two mount points and hidden count", () => {
    setTelemetry(
      telemetry([
        disk("/mnt/storage", 39),
        disk("/", 46),
        disk("/data", 43),
        disk("/media/backup", 70),
      ]),
    );

    render(<RemoteTelemetryBar />);

    expect(screen.getByText("/")).toBeInTheDocument();
    expect(screen.getByText("46%")).toBeInTheDocument();
    expect(screen.getByText("/data")).toBeInTheDocument();
    expect(screen.getByText("43%")).toBeInTheDocument();
    expect(screen.getByText("+2")).toBeInTheDocument();
    expect(screen.queryByText("/mnt/storage")).not.toBeInTheDocument();
  });

  it("renders telemetry for a connected local terminal", () => {
    const localTelemetry = telemetry([], "local-1");
    useSessionStore.setState({
      sessions: [
        {
          id: "local-1",
          name: "My computer",
          host: "localhost",
          port: 0,
          username: "local",
          isLocal: true,
        },
      ],
      activeSessionId: "local-1",
      connectedSessionIds: ["local-1"],
      telemetryBySession: { "local-1": localTelemetry },
    });

    render(<RemoteTelemetryBar />);

    expect(screen.getByText("lab")).toBeInTheDocument();
    expect(screen.queryByText("Telemetry unavailable")).not.toBeInTheDocument();
  });

  it("renders ? when usage percent is null", () => {
    setTelemetry(telemetry([disk("/", null)]));

    render(<RemoteTelemetryBar />);

    expect(screen.getByText("?")).toBeInTheDocument();
  });

  it("opens disk detail popover and shows the full non-hidden mount list", () => {
    setTelemetry(
      telemetry([
        disk("/", 46),
        disk("/data", 43),
        disk("/mnt/storage", 39),
        { ...disk("/run", 1), fsType: "tmpfs" },
      ]),
    );

    render(<RemoteTelemetryBar />);

    fireEvent.click(screen.getByRole("button", { name: /disk/i }));

    expect(screen.getByRole("dialog", { name: /disk details/i })).toBeInTheDocument();
    expect(screen.getAllByText("/mnt/storage").length).toBeGreaterThan(0);
    expect(screen.queryByText("/run")).not.toBeInTheDocument();
  });

  it("closes disk detail popover with Escape or outside click", () => {
    setTelemetry(telemetry([disk("/", 46)]));

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
    setTelemetry(telemetry([disk("/warn", 82), disk("/critical", 93)]));

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

  it("shows logged-in users and opens the users popover without a details request", async () => {
    setTelemetry({
      ...telemetry([]),
      users: [
        { user: "alice", tty: "pts/0", loginTime: "2026-07-15 09:12", from: "10.0.0.5" },
        { user: "alice", tty: "pts/1", loginTime: "2026-07-15 09:40", from: "10.0.0.5" },
        { user: "bob", tty: "tty1", loginTime: "2026-07-14 22:03", from: null },
      ],
    });

    render(<RemoteTelemetryBar />);

    expect(screen.getByText("2 users")).toBeInTheDocument();
    expect(screen.getByText("3 sessions")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /users/i }));

    const dialog = screen.getByRole("dialog", { name: /logged-in users/i });
    expect(within(dialog).getByText("pts/0")).toBeInTheDocument();
    expect(within(dialog).getByText("tty1")).toBeInTheDocument();
    expect(
      mockInvoke.mock.calls.some(([command]) => command === "get_resource_details"),
    ).toBe(false);
  });

  it("renders an Apple GPU card with the vendor tag and n/a fields", () => {
    setTelemetry({
      ...telemetry([]),
      gpu: [
        {
          index: 0,
          name: "Apple M2 Pro GPU",
          uuid: "apple-gpu",
          vendor: "apple",
          driverVersion: "",
          powerDrawW: null,
          powerLimitW: null,
          temperatureC: null,
          gpuUtilPercent: 42,
          memUtilPercent: null,
          memoryTotalMiB: null,
          memoryUsedMiB: 2048,
          memoryFreeMiB: null,
        },
      ],
    });

    render(<RemoteTelemetryBar />);

    const tag = screen.getByText("APPLE");
    expect(tag).toHaveClass("gpu-vendor-tag", "apple");
    expect(screen.getByText("Apple M2 Pro GPU")).toBeInTheDocument();
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

  it("restores the previous interval when saving telemetry settings fails", async () => {
    useSessionStore.setState({
      telemetrySettings: {
        telemetryIntervalSecs: 2,
        displayMode: "gpu-system",
        diskIgnoreFsTypes: [],
      },
    });
    mockInvoke.mockImplementation((command) => {
      if (command === "update_telemetry_settings") {
        return Promise.reject("settings file is read-only");
      }
      return Promise.resolve(resourceDetails);
    });

    render(<RemoteTelemetryBar />);
    const interval = screen.getByRole("combobox", { name: /interval/i });
    fireEvent.change(interval, { target: { value: "10" } });

    // The control must not keep claiming an interval the backend refused.
    await waitFor(() =>
      expect(useSessionStore.getState().telemetrySettings.telemetryIntervalSecs).toBe(2),
    );
    expect(interval).toHaveValue("2");
    expect(useSessionStore.getState().message?.kind).toBe("error");
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
      telemetryBySession: {
        "session-1": {
          ...telemetry([]),
          gpu: [gpuSummary(sessionOneGpu0), gpuSummary(sessionOneGpu1)],
        },
      },
    });

    render(<RemoteTelemetryBar />);

    fireEvent.click(screen.getByRole("button", { name: /GPU0 NVIDIA Session One GPU Zero/i }));
    fireEvent.click(await screen.findByRole("tab", { name: "GPU1" }));
    expect(
      within(screen.getByRole("dialog", { name: /GPU details/i })).getByText("Session One GPU One"),
    ).toBeInTheDocument();

    fireEvent.keyDown(document, { key: "Escape" });
    fireEvent.click(screen.getByRole("button", { name: /GPU0 NVIDIA Session One GPU Zero/i }));
    await waitFor(() =>
      expect(
        within(screen.getByRole("dialog", { name: /GPU details/i })).getByText("Session One GPU One"),
      ).toBeInTheDocument(),
    );

    act(() => {
      useSessionStore.setState({
        activeSessionId: "session-2",
        connectedSessionIds: ["session-1", "session-2"],
        telemetryBySession: {
          "session-2": {
            ...telemetry([], "session-2"),
            gpu: [gpuSummary(sessionTwoGpu)],
          },
        },
      });
    });

    await waitFor(() =>
      expect(screen.queryByRole("dialog", { name: /GPU details/i })).not.toBeInTheDocument(),
    );
    fireEvent.click(screen.getByRole("button", { name: /GPU3 NVIDIA Session Two GPU/i }));

    const sessionTwoDialog = await screen.findByRole("dialog", { name: /GPU details/i });
    expect(within(sessionTwoDialog).getByText("Session Two GPU")).toBeInTheDocument();
    expect(within(sessionTwoDialog).queryByText("Session One GPU One")).not.toBeInTheDocument();
    expect(screen.queryByRole("tablist", { name: "GPU selector" })).not.toBeInTheDocument();
  });

  it("exposes the monitoring panel close control", () => {
    const onClose = vi.fn();

    render(<RemoteTelemetryBar onClose={onClose} />);
    const closeButton = screen.getByRole("button", { name: "Close monitoring panel" });
    expect(closeButton).toHaveClass("ghost");
    expect(closeButton.querySelector(".lucide-panel-bottom-close")).toBeInTheDocument();
    fireEvent.click(closeButton);

    expect(onClose).toHaveBeenCalledOnce();
  });

  it("summarizes AI quotas and opens their provider details", async () => {
    setTelemetry({
      ...telemetry([]),
      agents: [codexAgent],
    });

    render(<RemoteTelemetryBar />);
    const agentsButton = screen.getByRole("button", { name: /ai dash/i });
    expect(agentsButton).toHaveClass("agent-summary-section");
    expect(within(agentsButton).getByText("1 session")).toBeInTheDocument();
    expect(within(agentsButton).getByText("Codex")).toBeInTheDocument();
    expect(
      within(agentsButton).getByRole("progressbar", {
        name: "Codex 5-hour: n/a",
      }),
    ).not.toHaveAttribute("aria-valuenow");
    expect(
      within(agentsButton).getByRole("progressbar", {
        name: "Codex weekly: 60%",
      }),
    ).toHaveAttribute("aria-valuenow", "60");
    expect(within(agentsButton).queryByText(/CPU/)).not.toBeInTheDocument();
    expect(within(agentsButton).queryByText(/RAM/)).not.toBeInTheDocument();

    fireEvent.click(agentsButton);
    const dialog = await screen.findByRole("dialog", { name: "AI DASH" });
    expect(within(dialog).getByText("gpt-test")).toBeInTheDocument();
    const contextSummary = within(dialog).getByText("Context details");
    expect(contextSummary.closest("details")).not.toHaveAttribute("open");
    fireEvent.click(contextSummary);
    expect(contextSummary.closest("details")).toHaveAttribute("open");
    expect(
      within(dialog).getByRole("progressbar", {
        name: "Context remaining: 95% remaining",
      }),
    ).toHaveAttribute("aria-valuenow", "95");
    expect(within(dialog).getByText("9.5K of 10K tokens left")).toBeInTheDocument();
    expect(within(dialog).getByText("Weekly limit")).toBeInTheDocument();
    expect(within(dialog).getByText("60% remaining")).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: /Refresh/i }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("refresh_agent_quota", {
        sessionId: "session-1",
        provider: "codex",
      }),
    );
  });

  it("shows every AGY group, Claude, and Codex period in the AI DASH card", () => {
    setTelemetry({
      ...telemetry([]),
      agents: [agyAgent, claudeAgent, codexAgent],
    });

    render(<RemoteTelemetryBar />);
    const card = screen.getByRole("button", { name: /ai dash/i });
    const sessionCount = within(card).getByText("3 sessions");
    expect(sessionCount).toHaveClass("agent-summary-session-count");
    expect(sessionCount.parentElement).toHaveClass("telemetry-section-title");
    expect(card.querySelector(".telemetry-section-body > strong")).not.toBeInTheDocument();
    expect(within(card).getByText("AGY · Gemini")).toBeInTheDocument();
    expect(within(card).getByText("AGY · Claude/GPT")).toBeInTheDocument();
    expect(within(card).getByText("Claude Code")).toBeInTheDocument();
    expect(within(card).getByText("Codex")).toBeInTheDocument();
    expect(
      within(card).getByRole("progressbar", {
        name: "AGY · Gemini 5-hour: 99.4%",
      }),
    ).toHaveAttribute("aria-valuenow", "99.4");
    expect(
      within(card).getByRole("progressbar", {
        name: "AGY · Gemini weekly: 99.95%",
      }),
    ).toHaveAttribute("aria-valuenow", "99.95");
    expect(
      within(card).getByRole("progressbar", {
        name: "AGY · Claude/GPT weekly: 100%",
      }),
    ).toHaveAttribute("aria-valuenow", "100");
    expect(
      within(card).getByRole("progressbar", {
        name: "Claude Code 5-hour: 80%",
      }),
    ).toHaveAttribute("aria-valuenow", "80");
    expect(
      within(card).getByRole("progressbar", {
        name: "Codex weekly: 60%",
      }),
    ).toHaveAttribute("aria-valuenow", "60");
  });

  it("deduplicates provider sessions and uses the newest account snapshot", () => {
    const newerCodex: AgentMetric = {
      ...codexAgent,
      rootPid: 5252,
      quota: {
        ...codexAgent.quota,
        capturedAt: (codexAgent.quota.capturedAt ?? 0) + 60,
        limits: [
          ...codexAgent.quota.limits.map((limit) => ({
            ...limit,
            remainingPercent: 20,
            usedPercent: 80,
          })),
          {
            ...codexAgent.quota.limits[0],
            label: "primary",
            windowMinutes: 300,
            remainingPercent: 8,
            usedPercent: 92,
          },
        ],
      },
    };
    setTelemetry({
      ...telemetry([]),
      agents: [codexAgent, newerCodex],
    });

    render(<RemoteTelemetryBar />);
    const card = screen.getByRole("button", { name: /ai dash/i });
    expect(within(card).getAllByText("Codex")).toHaveLength(1);
    const weekly = within(card).getByRole("progressbar", {
      name: "Codex weekly: 20%",
    });
    expect(weekly).toHaveClass("warning");
    expect(weekly).toHaveAttribute("title", expect.stringContaining("Live Codex account"));
    expect(
      within(card).getByRole("progressbar", {
        name: "Codex 5-hour: 8%",
      }),
    ).toHaveClass("critical");
  });

  it("groups AGY quotas and labels Claude 5-hour and weekly gauges", async () => {
    setTelemetry({
      ...telemetry([]),
      agents: [agyAgent, claudeAgent],
    });

    render(<RemoteTelemetryBar />);
    fireEvent.click(screen.getByRole("button", { name: /ai dash/i }));

    const dialog = await screen.findByRole("dialog", { name: "AI DASH" });
    expect(within(dialog).getByText("Gemini models")).toBeInTheDocument();
    expect(within(dialog).getByText("Claude and GPT models")).toBeInTheDocument();
    expect(within(dialog).getByText("Gemini Flash · Gemini Pro")).toBeInTheDocument();
    expect(
      within(dialog).getByText("Claude Opus · Claude Sonnet · GPT-OSS"),
    ).toBeInTheDocument();
    expect(
      within(dialog).getByRole("progressbar", {
        name: "Weekly limit: 99.95% remaining",
      }),
    ).toHaveAttribute("aria-valuenow", "99.95");
    expect(
      within(dialog).getByRole("progressbar", {
        name: "5-hour limit: 80% remaining",
      }),
    ).toHaveAttribute("aria-valuenow", "80");
    expect(
      within(dialog).getByRole("progressbar", {
        name: "Weekly limit: 60% remaining",
      }),
    ).toHaveAttribute("aria-valuenow", "60");
  });

  it("renders AGY 24-hour quota history and breaks lines at failed samples", async () => {
    const now = Math.floor(Date.now() / 1000);
    const historyAgent: AgentMetric = {
      ...agyAgent,
      quota: {
        ...agyAgent.quota,
        history: [
          {
            capturedAt: now - 600,
            status: "available",
            limits: [
              {
                group: "Gemini models",
                windowMinutes: 300,
                remainingPercent: 90,
              },
              {
                group: "Claude and GPT models",
                windowMinutes: 300,
                remainingPercent: 80,
              },
              {
                group: "Gemini models",
                windowMinutes: 10_080,
                remainingPercent: 70,
              },
              {
                group: "Claude and GPT models",
                windowMinutes: 10_080,
                remainingPercent: 60,
              },
            ],
          },
          {
            capturedAt: now - 300,
            status: "unavailable",
            limits: [],
          },
          {
            capturedAt: now,
            status: "available",
            limits: [
              {
                group: "Gemini models",
                windowMinutes: 300,
                remainingPercent: 88,
              },
              {
                group: "Claude and GPT models",
                windowMinutes: 300,
                remainingPercent: 78,
              },
              {
                group: "Gemini models",
                windowMinutes: 10_080,
                remainingPercent: 68,
              },
              {
                group: "Claude and GPT models",
                windowMinutes: 10_080,
                remainingPercent: 58,
              },
            ],
          },
        ],
      },
    };
    setTelemetry({
      ...telemetry([]),
      agents: [historyAgent],
    });

    render(<RemoteTelemetryBar />);
    fireEvent.click(screen.getByRole("button", { name: /ai dash/i }));
    const dialog = await screen.findByRole("dialog", { name: "AI DASH" });
    expect(
      within(dialog).getByRole("img", {
        name: "AGY 5-hour remaining trend over the last 24 hours",
      }),
    ).toBeInTheDocument();
    expect(
      within(dialog).getByRole("img", {
        name: "AGY Weekly remaining trend over the last 24 hours",
      }),
    ).toBeInTheDocument();
    expect(within(dialog).getByText("Gemini 88%")).toBeInTheDocument();
    expect(within(dialog).getByText("Claude/GPT 58%")).toBeInTheDocument();
    expect(
      dialog.querySelectorAll(".agy-history-series.gemini path").length,
    ).toBeGreaterThanOrEqual(4);
  });

  it("shows Claude per-request tokens instead of an unmeasured session total", async () => {
    setTelemetry({
      ...telemetry([]),
      agents: [claudeAgent],
    });

    render(<RemoteTelemetryBar />);
    fireEvent.click(screen.getByRole("button", { name: /ai dash/i }));

    const dialog = await screen.findByRole("dialog", { name: "AI DASH" });
    fireEvent.click(within(dialog).getAllByText("Context details")[0]);
    expect(within(dialog).getByText("Last request")).toBeInTheDocument();
    expect(within(dialog).queryByText("Session tokens")).not.toBeInTheDocument();
    expect(within(dialog).getByText("Cache read")).toBeInTheDocument();
    expect(within(dialog).getByText("8.5K")).toBeInTheDocument();
    expect(within(dialog).getByText("2K")).toBeInTheDocument();
  });

  it("explains how to enable Claude usage limits when no quota is reported", async () => {
    setTelemetry({
      ...telemetry([]),
      agents: [
        {
          ...claudeAgent,
          quota: {
            status: "setup-required",
            source: "none",
            capturedAt: null,
            snapshotAgeSeconds: null,
            message: "Set up the GpuTerm Claude status line.",
            history: [],
            limits: [],
          },
        },
      ],
    });

    render(<RemoteTelemetryBar />);
    fireEvent.click(screen.getByRole("button", { name: /ai dash/i }));

    const dialog = await screen.findByRole("dialog", { name: "AI DASH" });
    expect(within(dialog).getByText(/Set up the GpuTerm Claude status line/)).toBeInTheDocument();
    mockInvoke.mockResolvedValueOnce({
      status: "configured",
      message: "Claude quota monitoring is configured.",
    });
    fireEvent.click(within(dialog).getByRole("button", { name: /Set up/i }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("configure_claude_quota_monitor", {
        sessionId: "session-1",
      }),
    );
    expect(
      await within(dialog).findByText("Claude quota monitoring is configured."),
    ).toBeInTheDocument();
  });

  it("reports a rolled-over window and a stale snapshot instead of an old balance", async () => {
    setTelemetry({
      ...telemetry([]),
      agents: [
        {
          ...claudeAgent,
          quota: {
            status: "stale",
            source: "claude-statusline",
            capturedAt: 1_700_000_000,
            snapshotAgeSeconds: 900,
            message: "Waiting for a fresh provider snapshot.",
            history: [],
            limits: [
              {
                label: "five_hour",
                group: null,
                modelNames: [],
                remainingPercent: 20,
                usedPercent: 80,
                windowMinutes: 5 * 60,
                resetsAt: 1_700_000_000,
                stale: true,
              },
              {
                label: "seven_day",
                group: null,
                modelNames: [],
                remainingPercent: 60,
                usedPercent: 40,
                windowMinutes: 7 * 24 * 60,
                resetsAt: null,
                stale: false,
              },
            ],
          },
        },
      ],
    });

    render(<RemoteTelemetryBar />);
    const card = screen.getByRole("button", { name: /ai dash/i });
    expect(
      within(card).getByRole("progressbar", {
        name: "Claude Code 5-hour: reset",
      }),
    ).not.toHaveAttribute("aria-valuenow");
    fireEvent.click(screen.getByRole("button", { name: /ai dash/i }));

    const dialog = await screen.findByRole("dialog", { name: "AI DASH" });
    expect(
      within(dialog).getByRole("progressbar", { name: "5-hour limit: window reset" }),
    ).not.toHaveAttribute("aria-valuenow");
    expect(within(dialog).getByText("window reset")).toBeInTheDocument();
    expect(within(dialog).getByText(/as of 15m ago/)).toBeInTheDocument();
  });
});
