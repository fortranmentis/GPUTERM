/** Mirrors `ThresholdRule` in src-tauri/src/alerts.rs. */
export type ThresholdRule = {
  enabled: boolean;
  threshold: number;
  /** The alert clears below `threshold - hysteresis`. */
  hysteresis: number;
  sustainSecs: number;
};

export type IdleGpuRule = {
  enabled: boolean;
  utilBelow: number;
  minVramPercent: number;
  sustainSecs: number;
};

export type AlertSettings = {
  enabled: boolean;
  osNotifications: boolean;
  notifyWhenFocused: boolean;
  cooldownSecs: number;
  gpuTemp: ThresholdRule;
  gpuVram: ThresholdRule;
  gpuIdle: IdleGpuRule;
  memory: ThresholdRule;
  disk: ThresholdRule;
  llmCritical: boolean;
};

export type AlertKind = "gpu-temp" | "gpu-vram" | "gpu-idle" | "memory" | "disk" | "llm-critical";

/** Mirrors `AlertEvent`, sent as `telemetry-alert`. */
export type AlertEvent = {
  id: string;
  /** A session id, or "llm". */
  scope: string;
  hostLabel: string;
  kind: AlertKind;
  subject: string;
  state: "fired" | "resolved";
  value: number | null;
  threshold: number | null;
  at: number;
  title: string;
  body: string;
  notify: boolean;
  osDelivered: boolean;
};
