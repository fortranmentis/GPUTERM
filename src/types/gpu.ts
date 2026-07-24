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
  usedPercent: number | null;
  windowMinutes: number | null;
  resetsAt: number | null;
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
  costUsd: number | null;
  sessionDurationSeconds: number | null;
  rateLimits: AgentRateLimitMetric[];
  subagents: AgentWorkMetric[];
  backgroundTasks: AgentWorkMetric[];
};

export type RemoteTelemetry = {
  sessionId: string;
  timestamp: string;
  hostname: string | null;
  cpu: CpuMetric | null;
  memory: MemoryMetric | null;
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
  };
};

export type TelemetryDisplayMode = "gpu-only" | "system-only" | "gpu-system";

export type TelemetrySettings = {
  telemetryIntervalSecs: 1 | 2 | 5 | 10;
  displayMode: TelemetryDisplayMode;
  diskIgnoreFsTypes: string[];
};
