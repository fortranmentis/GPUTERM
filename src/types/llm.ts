/**
 * Mirrors `src-tauri/src/llm`. Rust `Option<T>` is `T | null` here, and a null
 * is never rendered as `0` — the UI distinguishes "unknown" from "zero" from
 * "not supported by this version".
 *
 * There is deliberately no API key field: the secret lives in the encrypted
 * vault and never crosses the IPC boundary.
 */

export type LlmRuntimeType = "ollama" | "vllm";

export type LlmInstance = {
  id: string;
  name: string;
  runtimeType: LlmRuntimeType;
  baseUrl: string;
  enabled: boolean;
  requestTimeoutMs: number;
  pollIntervalSecs: number;
  createdAt: number;
  updatedAt: number;
  /**
   * Saved SSH profile to tunnel the poll through. When set, `baseUrl` is
   * resolved on that host's network, so `127.0.0.1` is the runtime's own
   * loopback rather than this machine's.
   */
  sshProfileId?: string | null;
};

export type LlmRuntimeErrorCode =
  | "timeout"
  | "connection_refused"
  | "dns_error"
  | "authentication_error"
  | "http_client_error"
  | "http_server_error"
  | "invalid_response"
  | "parse_error"
  | "engine_dead"
  | "ssh_tunnel_error"
  | "ssh_host_untrusted"
  | "unknown_error";

export type LlmStatusKind = "online" | "degraded" | "offline" | "error";

export type LlmRuntimeStatus = {
  instanceId: string;
  runtimeType: LlmRuntimeType;
  status: LlmStatusKind;
  responseTimeMs: number | null;
  checkedAt: number;
  errorCode?: LlmRuntimeErrorCode;
  errorMessage?: string;
};

export type LlmRuntimeModel = {
  id: string;
  name: string;
  /** `running` and `installed` for Ollama, `served` for vLLM. */
  status: "running" | "installed" | "served";
  parameterSize: string | null;
  quantization: string | null;
  modelSizeBytes: number | null;
  vramSizeBytes: number | null;
  vramResidentPercent: number | null;
  /** An estimate of what is not resident in VRAM. Not a RAM measurement. */
  nonVramBytes: number | null;
  /** The configured maximum context, not what a conversation is using. */
  contextLength: number | null;
  expiresAt: number | null;
  expiresInSeconds: number | null;
  metadata?: Record<string, string>;
};

export type LlmRuntimeMetrics = {
  requestsRunning: number | null;
  requestsWaiting: number | null;
  requestsSwapped: number | null;
  kvCacheUsageRatio: number | null;
  kvCacheRemainingRatio: number | null;
  prefixCacheHitRatio: number | null;
  promptTokensPerSecond: number | null;
  generationTokensPerSecond: number | null;
  requestsPerSecond: number | null;
  preemptionsTotal: number | null;
  preemptionsDelta: number | null;
  ttftP50Seconds: number | null;
  ttftP95Seconds: number | null;
  e2eLatencyP95Seconds: number | null;
  queueTimeP95Seconds: number | null;
  collectedAt: number;
  /** Metric names this server does not expose, so the UI can say so. */
  unsupported?: string[];
};

export type LlmHistoryPoint = {
  capturedAt: number;
  responseTimeMs: number | null;
  requestsRunning: number | null;
  requestsWaiting: number | null;
  kvCacheUsageRatio: number | null;
  promptTokensPerSecond: number | null;
  generationTokensPerSecond: number | null;
  requestsPerSecond: number | null;
  preemptionsDelta: number | null;
};

export type LlmEvent = {
  at: number;
  kind: "status_changed" | "counters_reset" | "error";
  code: string;
  detail?: string;
};

export type LlmErrorInfo = {
  at: number;
  code: LlmRuntimeErrorCode;
  message: string;
};

export type LlmSeverity =
  | "unknown"
  | "normal"
  | "warning"
  | "congested"
  | "critical";

export type LlmInstanceTelemetry = {
  instance: LlmInstance;
  /** Whether a key is stored, never the key itself. */
  hasApiKey: boolean;
  status: LlmRuntimeStatus | null;
  severity: LlmSeverity;
  severityReasons: string[];
  models: LlmRuntimeModel[];
  runningModelCount: number;
  metrics: LlmRuntimeMetrics | null;
  history: LlmHistoryPoint[];
  events: LlmEvent[];
  lastSuccessAt: number | null;
  lastError: LlmErrorInfo | null;
  consecutiveFailures: number;
  /** Name of the SSH profile the poll is tunneled through, when there is one. */
  sshProfileName?: string | null;
};

export type LlmSummary = {
  registered: number;
  enabled: number;
  normal: number;
  warning: number;
  error: number;
  unknown: number;
  models: number;
  vllmRequestsRunning: number | null;
  vllmRequestsWaiting: number | null;
};

export type LlmTelemetry = {
  generatedAt: number;
  summary: LlmSummary;
  instances: LlmInstanceTelemetry[];
};
