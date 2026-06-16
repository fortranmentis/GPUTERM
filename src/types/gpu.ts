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

export type GpuMetricsPayload = {
  sessionId: string;
  status: "available" | "unavailable";
  metrics: GpuMetric[];
  message?: string | null;
  updatedAt: number;
};
