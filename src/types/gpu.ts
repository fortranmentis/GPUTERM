export type GpuVendor = "nvidia" | "amd" | "intel" | "apple";

export type GpuMetric = {
  index: number;
  name: string;
  uuid: string;
  vendor: GpuVendor;
  driverVersion: string;
  powerDrawW: number | null;
  powerLimitW: number | null;
  temperatureC: number | null;
  gpuUtilPercent: number | null;
  memUtilPercent: number | null;
  memoryTotalMiB: number | null;
  memoryUsedMiB: number | null;
  memoryFreeMiB: number | null;
};

export type CpuMetric = {
  modelName: string | null;
  usagePercent: number | null;
  loadAvg1: number | null;
  loadAvg5: number | null;
  loadAvg15: number | null;
  totalCores: number | null;
  onlineCores: number | null;
  avgClockGhz: number | null;
};

export type MemoryMetric = {
  totalMiB: number | null;
  usedMiB: number | null;
  availableMiB: number | null;
  freeMiB: number | null;
  usagePercent: number | null;
  swapTotalMiB: number | null;
  swapUsedMiB: number | null;
  swapFreeMiB: number | null;
};

export type DiskMetric = {
  filesystem: string;
  fsType: string | null;
  mountPoint: string;
  totalBytes: number | null;
  usedBytes: number | null;
  availableBytes: number | null;
  usagePercent: number | null;
};

export type RemoteUserSession = {
  user: string;
  tty: string;
  loginTime: string;
  from: string | null;
};

export type AgentRateLimitMetric = {
  label: string;
  group: string | null;
  modelNames: string[];
  remainingPercent: number | null;
  usedPercent: number | null;
  windowMinutes: number | null;
  resetsAt: number | null;
  stale: boolean;
};

export type AgentQuotaHistoryLimit = {
  group: string | null;
  windowMinutes: number;
  remainingPercent: number | null;
};

export type AgentQuotaHistoryPoint = {
  capturedAt: number;
  status: "available" | "unavailable";
  limits: AgentQuotaHistoryLimit[];
};

export type AgentQuotaSnapshot = {
  status: "available" | "setup-required" | "unsupported" | "stale" | "error";
  source:
    | "codex-app-server"
    | "codex-session-log"
    | "claude-statusline"
    | "agy-usage-tui"
    | "none";
  capturedAt: number | null;
  snapshotAgeSeconds: number | null;
  message: string | null;
  limits: AgentRateLimitMetric[];
  history: AgentQuotaHistoryPoint[];
};

export type AgentWorkMetric = {
  name: string;
  role: string | null;
  status: string | null;
};

export type AgentMetric = {
  provider: "agy" | "codex" | "claude";
  displayName: string;
  status: string;
  rootPid: number;
  processCount: number;
  user: string | null;
  cpuPercent: number | null;
  memoryBytes: number | null;
  elapsedSeconds: number | null;
  sessionId: string | null;
  cwd: string | null;
  model: string | null;
  inputTokens: number | null;
  outputTokens: number | null;
  totalTokens: number | null;
  contextUsedTokens: number | null;
  contextWindowTokens: number | null;
  contextUsedPercent: number | null;
  contextRemainingTokens: number | null;
  contextRemainingPercent: number | null;
  lastRequestInputTokens: number | null;
  lastRequestOutputTokens: number | null;
  lastRequestCacheCreationTokens: number | null;
  lastRequestCacheReadTokens: number | null;
  costUsd: number | null;
  sessionDurationSeconds: number | null;
  snapshotAgeSeconds: number | null;
  quota: AgentQuotaSnapshot;
  subagents: AgentWorkMetric[];
  backgroundTasks: AgentWorkMetric[];
};

/** One temperature-reporting device. */
export type ThermalSensor = {
  label: string;
  /** Kernel driver or provider: coretemp, k10temp, jc42, nvme, acpi_thermal_zone, … */
  source: string;
  temperatureC: number | null;
  highC: number | null;
  criticalC: number | null;
};

export type ThermalGroup = {
  headlineC: number | null;
  /** Which sensor it came from, or how it was chosen ("hottest of 16 cores"). */
  headlineLabel: string | null;
  /** Set only when the reading is not a die temperature (an ACPI zone). */
  caveat: string | null;
  sensors: ThermalSensor[];
};

export type ThermalCategoryCode = "cpu" | "memory" | "disk";

/** A category this host genuinely cannot report, with the reason named. */
export type ThermalUnsupported = { category: ThermalCategoryCode; reason: string };

export type ThermalMetric = {
  cpu: ThermalGroup | null;
  memory: ThermalGroup | null;
  disk: ThermalGroup | null;
  unsupported: ThermalUnsupported[];
};

export type RemoteTelemetry = {
  sessionId: string;
  timestamp: string;
  hostname: string | null;
  cpu: CpuMetric | null;
  memory: MemoryMetric | null;
  /** `null` means not read yet; within it, an `unsupported` entry means the host
   * cannot report that category. The two must never render the same way. */
  thermal: ThermalMetric | null;
  disks: DiskMetric[];
  gpu: GpuMetric[];
  users: RemoteUserSession[];
  agents: AgentMetric[];
  errors: {
    cpu?: string;
    memory?: string;
    disk?: string;
    gpu?: string;
    users?: string;
    agents?: string;
    /** Set only when a temperature command failed — never merely because a host
     * has no sensors. */
    thermal?: string;
  };
};

export type TelemetryDisplayMode = "gpu-only" | "system-only" | "gpu-system";

export type TelemetrySettings = {
  telemetryIntervalSecs: 1 | 2 | 5 | 10;
  displayMode: TelemetryDisplayMode;
  diskIgnoreFsTypes: string[];
};
