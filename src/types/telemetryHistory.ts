/** Mirrors `TelemetryHistoryPayload` in src-tauri/src/ssh/telemetry_history.rs. */
export type SystemHistoryPoint = {
  capturedAt: number;
  cpuPercent: number | null;
  memoryPercent: number | null;
  swapPercent: number | null;
  diskMaxPercent: number | null;
};

export type GpuHistoryPoint = {
  capturedAt: number;
  utilPercent: number | null;
  vramPercent: number | null;
  vramUsedMiB: number | null;
  temperatureC: number | null;
  powerW: number | null;
};

export type GpuHistorySeries = {
  key: string;
  index: number;
  name: string;
  memoryTotalMiB: number | null;
  powerLimitW: number | null;
  points: GpuHistoryPoint[];
};

export type TelemetryHistory = {
  sessionId: string;
  bucketSecs: number;
  windowSecs: number;
  system: SystemHistoryPoint[];
  gpus: GpuHistorySeries[];
};

/** Same key as `gpu_history_key` in the backend: the UUID, else the index. */
export function gpuHistoryKey(uuid: string | null | undefined, index: number) {
  return uuid && uuid.trim() ? uuid : `idx:${index}`;
}
