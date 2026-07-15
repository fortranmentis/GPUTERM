export type GpuMetric = {
  index: number;
  name: string;
  uuid: string;
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

export type RemoteTelemetry = {
  timestamp: string;
  hostname: string | null;
  cpu: CpuMetric | null;
  memory: MemoryMetric | null;
  disks: DiskMetric[];
  gpu: GpuMetric[];
  users: RemoteUserSession[];
  errors: {
    cpu?: string;
    memory?: string;
    disk?: string;
    gpu?: string;
    users?: string;
  };
};

export type TelemetryDisplayMode = "gpu-only" | "system-only" | "gpu-system";

export type TelemetrySettings = {
  telemetryIntervalSecs: 1 | 2 | 5 | 10;
  displayMode: TelemetryDisplayMode;
  diskIgnoreFsTypes: string[];
};
