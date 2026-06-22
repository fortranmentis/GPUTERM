export type ProcessMetric = {
  pid: number;
  user: string | null;
  command: string | null;
  cpuPercent?: number | null;
  memoryPercent?: number | null;
  rssBytes?: number | null;
  vszBytes?: number | null;
  elapsedTime?: string | null;
};

export type CpuDetailMetric = {
  modelName: string | null;
  usagePercent: number | null;
  loadAvg1: number | null;
  loadAvg5: number | null;
  loadAvg15: number | null;
  totalCores: number | null;
  onlineCores: number | null;
  avgClockGhz: number | null;
  uptimeSeconds: number | null;
  logicalCoreUsagePercent: Array<number | null>;
  topProcesses: ProcessMetric[];
};

export type MemoryDetailMetric = {
  totalMiB: number | null;
  usedMiB: number | null;
  availableMiB: number | null;
  freeMiB: number | null;
  buffersMiB: number | null;
  cachedMiB: number | null;
  swapTotalMiB: number | null;
  swapUsedMiB: number | null;
  swapFreeMiB: number | null;
  usagePercent: number | null;
  topProcesses: ProcessMetric[];
};

export type GpuProcessMetric = {
  gpuIndex: number | null;
  gpuUuid: string | null;
  pid: number;
  user: string | null;
  processName: string | null;
  command: string | null;
  usedMemoryMiB: number | null;
};

export type GpuDetailMetric = {
  index: number;
  name: string;
  uuid: string;
  driverVersion: string | null;
  gpuUtilPercent: number | null;
  memoryUtilPercent: number | null;
  memoryTotalMiB: number | null;
  memoryUsedMiB: number | null;
  memoryFreeMiB: number | null;
  temperatureC: number | null;
  powerDrawW: number | null;
  powerLimitW: number | null;
  fanSpeedPercent: number | null;
  graphicsClockMHz: number | null;
  memoryClockMHz: number | null;
  pciBusId: string | null;
  persistenceMode: string | null;
  migMode: string | null;
  processes: GpuProcessMetric[];
};

export type ResourceDetails = {
  cpu: CpuDetailMetric | null;
  memory: MemoryDetailMetric | null;
  gpus: GpuDetailMetric[];
  errors: {
    cpu?: string;
    memory?: string;
    gpu?: string;
  };
};

export type ResourceDetailType = "cpu" | "memory" | "gpu";
