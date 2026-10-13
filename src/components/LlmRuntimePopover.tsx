import { invoke } from "@tauri-apps/api/core";
import { Boxes, Plus, RefreshCw } from "lucide-react";
import { useMemo, useState, type RefObject } from "react";
import { isLoopbackAddress, LlmInstanceForm } from "./LlmInstanceForm";
import { DetailUsageBar, ResourceDetailPopover } from "./ResourceDetailPopover";
import {
  CHART_RANGES,
  ChartRangeToggle,
  HistoryChart,
  type ChartRange,
} from "./HistoryChart";
import type {
  LlmEvent,
  LlmHistoryPoint,
  LlmInstance,
  LlmInstanceTelemetry,
  LlmRuntimeErrorCode,
  LlmRuntimeMetrics,
  LlmRuntimeModel,
  LlmSeverity,
  LlmTelemetry,
  LlmRuntimeType,
} from "../types/llm";
import { useSessionStore } from "../stores/sessionStore";
import { formatBytes } from "../utils/formatBytes";
import { isServingEngine, runtimeLabel } from "../utils/llmRuntimes";
import { formatNumber, formatPercent } from "../utils/format";

// The longest range must not exceed the backend's `HISTORY_WINDOW_SECONDS`
// (llm/monitor.rs), or its left edge stays empty.
export { CHART_RANGES, type ChartRange } from "./HistoryChart";

/** Nothing here is ever rendered as `0`: a null reading stays visibly absent. */
const UNKNOWN = "—";

const ERROR_LABELS: Record<LlmRuntimeErrorCode, string> = {
  timeout: "Request timed out",
  connection_refused: "Connection refused",
  dns_error: "Host name could not be resolved",
  authentication_error: "Rejected the API key",
  http_client_error: "Request rejected",
  http_server_error: "Server error",
  invalid_response: "Unexpected response",
  parse_error: "Response could not be read",
  engine_dead: "Engine not ready (HTTP 503)",
  ssh_tunnel_error: "SSH tunnel failed",
  ssh_host_untrusted: "SSH host key not trusted",
  unknown_error: "Could not be reached",
};

const SEVERITY_LABELS: Record<LlmSeverity, string> = {
  unknown: "Not polled yet",
  normal: "Normal",
  warning: "Attention",
  congested: "Congested",
  critical: "Critical",
};

const REASON_LABELS: Record<string, string> = {
  poll_failed: "A poll failed",
  repeated_failures: "Three or more consecutive failures",
  parse_degraded: "Part of the response could not be read",
  slow_response: "Response took a second or longer",
  api_error: "The API returned an error",
  engine_dead: "The engine reported that it is not ready",
  authentication_error: "The API key was rejected",
  kv_cache_high: "KV cache at 70% or above",
  kv_cache_congested: "KV cache at 85% or above",
  kv_cache_critical: "KV cache at 95% or above",
  requests_waiting: "Requests are queued",
  waiting_sustained: "The queue is not draining",
  preemption_increase: "New preemptions since the last poll",
  ssh_unreachable: "The SSH tunnel could not be established",
};

const EVENT_LABELS: Record<string, string> = {
  counters_reset: "Counters restarted — the server was probably restarted",
  online: "Came online",
  degraded: "Degraded",
  offline: "Went offline",
  error: "Reported an error",
};

function severityClass(severity: LlmSeverity) {
  return severity === "critical"
    ? "critical"
    : severity === "congested" || severity === "warning"
      ? "warning"
      : severity === "normal"
        ? "normal"
        : "unknown";
}

function formatRatioPercent(value: number | null | undefined) {
  return value == null ? UNKNOWN : formatPercent(value * 100, 1);
}

function formatMs(value: number | null | undefined) {
  return value == null ? UNKNOWN : `${Math.round(value)} ms`;
}

function formatSeconds(value: number | null | undefined) {
  if (value == null) {
    return UNKNOWN;
  }
  return value < 1 ? `${Math.round(value * 1000)} ms` : `${formatNumber(value, 2)} s`;
}

function formatCount(value: number | null | undefined) {
  return value == null ? UNKNOWN : formatNumber(value, 0);
}

function formatRate(value: number | null | undefined, unit: string) {
  return value == null ? UNKNOWN : `${formatNumber(value, 1)} ${unit}`;
}

function formatClockTime(epochSeconds: number | null | undefined) {
  if (!epochSeconds) {
    return UNKNOWN;
  }
  return new Date(epochSeconds * 1000).toLocaleTimeString();
}

function formatRemaining(seconds: number | null | undefined) {
  if (seconds == null) {
    return UNKNOWN;
  }
  if (seconds === 0) {
    return "expired";
  }
  if (seconds < 60) {
    return `${seconds}s`;
  }
  const minutes = Math.floor(seconds / 60);
  return minutes < 60 ? `${minutes}m` : `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
}

export function LlmRuntimeDetailContent({
  telemetry,
  instances,
  onInstancesChange,
}: {
  telemetry: LlmTelemetry | null;
  instances: LlmInstance[];
  onInstancesChange: (instances: LlmInstance[]) => void;
}) {
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [formState, setFormState] = useState<"closed" | "new" | "edit">(
    "closed",
  );
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  // Pinned when the edit form opens: `selected` falls back to the first row,
  // so if another window deletes the instance being edited the form must
  // close rather than silently switch to editing a different one.
  const [editingId, setEditingId] = useState<string | null>(null);

  const rows = useMemo(() => {
    const byId = new Map(
      (telemetry?.instances ?? []).map((entry) => [entry.instance.id, entry]),
    );
    // The registered list is authoritative, so a just-added instance appears
    // before its first poll has produced any telemetry.
    return instances.map((instance) => ({
      instance,
      telemetry: byId.get(instance.id) ?? null,
    }));
  }, [instances, telemetry]);

  const selected =
    rows.find((row) => row.instance.id === selectedId) ?? rows[0] ?? null;
  const editingRow =
    formState === "edit"
      ? (rows.find((row) => row.instance.id === editingId) ?? null)
      : null;
  if (formState === "edit" && !editingRow) {
    // Adjusting state during render (not in an effect) avoids one frame with
    // a form bound to nothing.
    setFormState("closed");
  }

  const runAction = async (action: () => Promise<LlmInstance[]>) => {
    setBusy(true);
    setActionError(null);
    try {
      onInstancesChange(await action());
    } catch (error) {
      setActionError(String(error));
    } finally {
      setBusy(false);
    }
  };

  if (instances.length === 0 && formState === "closed") {
    return (
      <div className="llm-detail">
        <div className="resource-unavailable">
          <strong>No LLM runtimes registered</strong>
          <span>
            Add an Ollama, vLLM, SGLang, TGI, llama.cpp, OpenAI-compatible or Triton server to monitor it. Nothing is polled until
            you do.
          </span>
        </div>
        <button
          type="button"
          className="primary"
          onClick={() => setFormState("new")}
        >
          <Plus size={14} /> Add instance
        </button>
      </div>
    );
  }

  return (
    <div className="llm-detail">
      {telemetry && <LlmSummaryStrip telemetry={telemetry} />}

      <div className="llm-instance-strip" role="tablist" aria-label="LLM instances">
        {rows.map((row) => {
          const active = selected?.instance.id === row.instance.id;
          const severity = row.telemetry?.severity ?? "unknown";
          return (
            <button
              type="button"
              role="tab"
              aria-selected={active}
              className={`llm-instance-tab ${active ? "selected" : ""} ${severityClass(severity)}`}
              key={row.instance.id}
              onClick={() => setSelectedId(row.instance.id)}
            >
              <span className={`llm-runtime-tag ${row.instance.runtimeType}`}>
                {runtimeLabel(row.instance.runtimeType)}
              </span>
              <strong>{row.instance.name}</strong>
              <small>
                {row.instance.enabled
                  ? (row.telemetry?.status?.status ?? "pending")
                  : "disabled"}
              </small>
            </button>
          );
        })}
        <button
          type="button"
          className="llm-instance-tab add"
          onClick={() => setFormState("new")}
        >
          <Plus size={14} /> Add
        </button>
      </div>

      {actionError && (
        <p className="llm-form-message error" role="alert">
          {actionError}
        </p>
      )}

      {selected && (
        <LlmInstanceDetail
          instance={selected.instance}
          telemetry={selected.telemetry}
          busy={busy}
          onEdit={() => {
            setEditingId(selected.instance.id);
            setFormState("edit");
          }}
          onToggleEnabled={() =>
            void runAction(() =>
              invoke<LlmInstance[]>("set_llm_instance_enabled", {
                id: selected.instance.id,
                enabled: !selected.instance.enabled,
              }),
            )
          }
          onDelete={() =>
            void runAction(() =>
              invoke<LlmInstance[]>("delete_llm_instance", {
                id: selected.instance.id,
              }),
            )
          }
        />
      )}

      {(formState === "new" || editingRow) && (
        <LlmInstanceForm
          editing={editingRow?.instance ?? null}
          hasApiKey={editingRow?.telemetry?.hasApiKey ?? false}
          onSaved={(saved) => {
            onInstancesChange(saved);
            setFormState("closed");
          }}
          onCancel={() => setFormState("closed")}
        />
      )}
    </div>
  );
}

function LlmSummaryStrip({ telemetry }: { telemetry: LlmTelemetry }) {
  const { summary } = telemetry;
  return (
    <div className="llm-summary-strip">
      <SummaryItem label="Registered" value={String(summary.registered)} />
      <SummaryItem label="Normal" value={String(summary.normal)} tone="normal" />
      <SummaryItem
        label="Attention"
        value={String(summary.warning)}
        tone="warning"
      />
      <SummaryItem label="Error" value={String(summary.error)} tone="critical" />
      <SummaryItem label="Models" value={String(summary.models)} />
      <SummaryItem
        label="Queue running"
        value={formatCount(summary.vllmRequestsRunning)}
      />
      <SummaryItem
        label="Queue waiting"
        value={formatCount(summary.vllmRequestsWaiting)}
      />
    </div>
  );
}

function SummaryItem({
  label,
  value,
  tone,
}: {
  label: string;
  value: string;
  tone?: string;
}) {
  return (
    <div className={`llm-summary-item ${tone ?? ""}`}>
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  );
}

function LlmInstanceDetail({
  instance,
  telemetry,
  busy,
  onEdit,
  onToggleEnabled,
  onDelete,
}: {
  instance: LlmInstance;
  telemetry: LlmInstanceTelemetry | null;
  busy: boolean;
  onEdit: () => void;
  onToggleEnabled: () => void;
  onDelete: () => void;
}) {
  const status = telemetry?.status ?? null;
  const severity = telemetry?.severity ?? "unknown";
  const metrics = telemetry?.metrics ?? null;

  return (
    <article className="llm-detail-card">
      <header>
        <div>
          <span className={`llm-runtime-tag ${instance.runtimeType}`}>
            {runtimeLabel(instance.runtimeType)}
          </span>
          <strong>{instance.name}</strong>
          <small title={instance.baseUrl}>
            {instance.baseUrl}
            {telemetry?.sshProfileName && (
              // A bare 127.0.0.1 is meaningless without naming the hop.
              <span className="llm-via-badge">via {telemetry.sshProfileName}</span>
            )}
          </small>
        </div>
        <span className={`llm-severity ${severityClass(severity)}`}>
          {SEVERITY_LABELS[severity]}
        </span>
      </header>

      <div className="llm-detail-actions">
        <button type="button" disabled={busy} onClick={onEdit}>
          Edit
        </button>
        <button type="button" disabled={busy} onClick={onToggleEnabled}>
          {instance.enabled ? "Disable" : "Enable"}
        </button>
        <button type="button" className="danger" disabled={busy} onClick={onDelete}>
          Delete
        </button>
      </div>

      {!instance.enabled && (
        <p className="llm-form-hint">
          Polling is off for this instance. Nothing is being requested from it.
        </p>
      )}

      <div className="resource-metric-grid llm-metric-grid">
        <Field label="Status" value={status?.status ?? "pending"} />
        <Field label="Response time" value={formatMs(status?.responseTimeMs)} />
        <Field
          label="Last successful poll"
          value={formatClockTime(telemetry?.lastSuccessAt)}
        />
        <Field
          label="Consecutive failures"
          value={String(telemetry?.consecutiveFailures ?? 0)}
        />
        <Field label="Poll interval" value={`${instance.pollIntervalSecs}s`} />
        <Field label="Timeout" value={`${instance.requestTimeoutMs} ms`} />
        <Field
          label="Reached through"
          title={
            telemetry?.sshProfileName
              ? "The address is resolved on the remote host, over an SSH tunnel."
              : undefined
          }
          value={telemetry?.sshProfileName ?? "Direct"}
        />
      </div>

      {telemetry?.severityReasons && telemetry.severityReasons.length > 0 && (
        <ul className="llm-reason-list">
          {telemetry.severityReasons.map((reason) => (
            <li key={reason}>{REASON_LABELS[reason] ?? reason}</li>
          ))}
        </ul>
      )}

      {!instance.sshProfileId &&
        isLoopbackAddress(instance.baseUrl) &&
        (status?.status === "offline" || status?.status === "error") && (
          <p className="llm-form-warning">
            This address is <strong>this machine&apos;s</strong> loopback and
            Reached through is <strong>Direct</strong>, so the poll never leaves
            this computer. If the runtime is on another host, press Edit and set
            Reach through to that host&apos;s SSH profile — the address then
            resolves on the remote side.
          </p>
        )}

      {telemetry?.lastError && (
        <p className="llm-form-message error">
          <strong>
            {ERROR_LABELS[telemetry.lastError.code] ?? telemetry.lastError.code}
          </strong>{" "}
          {telemetry.lastError.message} ({formatClockTime(telemetry.lastError.at)})
        </p>
      )}

      {isServingEngine(instance.runtimeType) && <VllmMetrics metrics={metrics} />}

      <LlmModelList
        models={telemetry?.models ?? []}
        runtimeType={instance.runtimeType}
      />

      <LlmCharts
        history={telemetry?.history ?? []}
        runtimeType={instance.runtimeType}
      />

      {metrics?.unsupported && metrics.unsupported.length > 0 && (
        <section className="llm-unsupported">
          <strong>Not exposed by this server</strong>
          <ul>
            {metrics.unsupported.map((name) => (
              <li key={name}>
                <code>{name}</code>
              </li>
            ))}
          </ul>
          <small>
            These are reported as unavailable rather than zero, because this
            build of the runtime does not publish them.
          </small>
        </section>
      )}

      <LlmEventList events={telemetry?.events ?? []} />
    </article>
  );
}

function VllmMetrics({ metrics }: { metrics: LlmRuntimeMetrics | null }) {
  if (!metrics) {
    return (
      <div className="resource-unavailable">
        <strong>No metrics collected yet</strong>
        <span>
          Serving metrics appear once <code>/metrics</code> has been read
          successfully.
        </span>
      </div>
    );
  }

  const kvPercent =
    metrics.kvCacheUsageRatio == null ? null : metrics.kvCacheUsageRatio * 100;

  return (
    <>
      <section className="llm-kv-cache">
        <div className="llm-kv-heading">
          <span>Server-wide KV cache</span>
          <strong>{formatRatioPercent(metrics.kvCacheUsageRatio)}</strong>
        </div>
        <DetailUsageBar
          value={kvPercent}
          level={
            kvPercent == null
              ? "unknown"
              : kvPercent >= 85
                ? "critical"
                : kvPercent >= 70
                  ? "warning"
                  : "normal"
          }
          ariaLabel="Server-wide KV cache usage"
        />
        <small>
          This is the whole server&apos;s KV cache, not the remaining context of
          any one conversation.
        </small>
      </section>

      <div className="resource-metric-grid llm-metric-grid">
        <Field label="Running requests" value={formatCount(metrics.requestsRunning)} />
        <Field label="Waiting requests" value={formatCount(metrics.requestsWaiting)} />
        <Field
          label="Swapped requests"
          value={
            metrics.requestsSwapped == null
              ? "not supported"
              : formatCount(metrics.requestsSwapped)
          }
        />
        <Field
          label="KV cache remaining"
          value={formatRatioPercent(metrics.kvCacheRemainingRatio)}
        />
        <Field
          label="Prefix cache hit rate"
          value={formatRatioPercent(metrics.prefixCacheHitRatio)}
        />
        <Field
          label="Prompt tokens"
          value={formatRate(metrics.promptTokensPerSecond, "tok/s")}
        />
        <Field
          label="Generated tokens"
          value={formatRate(metrics.generationTokensPerSecond, "tok/s")}
        />
        <Field
          label="Requests"
          value={formatRate(metrics.requestsPerSecond, "req/s")}
        />
        <Field label="Preemptions" value={formatCount(metrics.preemptionsTotal)} />
        <Field
          label="Preemptions since last poll"
          value={formatCount(metrics.preemptionsDelta)}
        />
        <Field label="TTFT P50" value={formatSeconds(metrics.ttftP50Seconds)} />
        <Field label="TTFT P95" value={formatSeconds(metrics.ttftP95Seconds)} />
        <Field
          label="End-to-end P95"
          value={formatSeconds(metrics.e2eLatencyP95Seconds)}
        />
        <Field
          label="Queue time P95"
          value={formatSeconds(metrics.queueTimeP95Seconds)}
        />
      </div>
    </>
  );
}

function LlmModelList({
  models,
  runtimeType,
}: {
  models: LlmRuntimeModel[];
  runtimeType: LlmRuntimeType;
}) {
  if (models.length === 0) {
    return (
      <section className="llm-model-list">
        <h4>Models</h4>
        <p className="llm-form-hint">
          {runtimeType === "ollama"
            ? "No models are loaded or installed."
            : "No served models reported."}
        </p>
      </section>
    );
  }

  return (
    <section className="llm-model-list">
      <h4>Models ({models.length})</h4>
      {models.map((model) => (
        <div className="llm-model-row" key={`${model.id}:${model.name}`}>
          <header>
            <strong title={model.name}>{model.name}</strong>
            <span className={`llm-model-status ${model.status}`}>
              {model.status}
            </span>
          </header>
          <div className="resource-metric-grid llm-metric-grid">
            <Field label="Parameters" value={model.parameterSize ?? UNKNOWN} />
            <Field label="Quantization" value={model.quantization ?? UNKNOWN} />
            <Field
              label="Model size"
              value={
                model.modelSizeBytes == null
                  ? UNKNOWN
                  : formatBytes(model.modelSizeBytes)
              }
            />
            <Field
              label="Resident in VRAM"
              value={
                model.vramSizeBytes == null
                  ? UNKNOWN
                  : formatBytes(model.vramSizeBytes)
              }
            />
            <Field
              label="VRAM residency"
              value={
                model.vramResidentPercent == null
                  ? UNKNOWN
                  : formatPercent(model.vramResidentPercent, 1)
              }
            />
            <Field
              label="Estimated non-VRAM residency"
              title="Model size minus the part resident in VRAM. This is an estimate of CPU offloading, not a measurement of system RAM usage."
              value={
                model.nonVramBytes == null
                  ? UNKNOWN
                  : formatBytes(model.nonVramBytes)
              }
            />
            <Field
              label="Configured max context"
              title="The context window the model is configured for, not the context a conversation is currently using."
              value={
                model.contextLength == null
                  ? UNKNOWN
                  : `${formatNumber(model.contextLength, 0)} tokens`
              }
            />
            <Field
              label="Unload at"
              value={formatClockTime(model.expiresAt)}
            />
            <Field
              label="Kept in memory for"
              value={formatRemaining(model.expiresInSeconds)}
            />
          </div>
        </div>
      ))}
    </section>
  );
}

function LlmEventList({ events }: { events: LlmEvent[] }) {
  if (events.length === 0) {
    return null;
  }
  const recent = [...events].reverse().slice(0, 8);
  return (
    <section className="llm-event-list">
      <h4>Recent changes</h4>
      <ul>
        {recent.map((event, index) => (
          <li key={`${event.at}:${event.kind}:${index}`}>
            <span>{formatClockTime(event.at)}</span>
            <strong>
              {EVENT_LABELS[event.code] ??
                ERROR_LABELS[event.code as LlmRuntimeErrorCode] ??
                event.code}
            </strong>
          </li>
        ))}
      </ul>
    </section>
  );
}

type ChartSeries = {
  key: string;
  label: string;
  value: (point: LlmHistoryPoint) => number | null;
};

function LlmCharts({
  history,
  runtimeType,
}: {
  history: LlmHistoryPoint[];
  runtimeType: LlmRuntimeType;
}) {
  const [range, setRange] = useState<ChartRange>(CHART_RANGES[0]);
  if (history.length === 0) {
    return null;
  }

  return (
    <section className="llm-charts">
      <ChartRangeToggle range={range} onChange={setRange} />
      <div className="history-chart-grid">
        <LlmHistoryChart
          label="Response time (ms)"
          history={history}
          range={range}
          series={[
            {
              key: "response",
              label: "Response",
              value: (point) => point.responseTimeMs,
            },
          ]}
        />
        {isServingEngine(runtimeType) && (
          <>
            <LlmHistoryChart
              label="KV cache (%)"
              history={history}
              range={range}
              max={100}
              series={[
                {
                  key: "kv",
                  label: "KV cache",
                  value: (point) =>
                    point.kvCacheUsageRatio == null
                      ? null
                      : point.kvCacheUsageRatio * 100,
                },
              ]}
            />
            <LlmHistoryChart
              label="Requests"
              history={history}
              range={range}
              series={[
                {
                  key: "running",
                  label: "Running",
                  value: (point) => point.requestsRunning,
                },
                {
                  key: "waiting",
                  label: "Waiting",
                  value: (point) => point.requestsWaiting,
                },
              ]}
            />
            <LlmHistoryChart
              label="Tokens/s"
              history={history}
              range={range}
              series={[
                {
                  key: "prompt",
                  label: "Prompt",
                  value: (point) => point.promptTokensPerSecond,
                },
                {
                  key: "generation",
                  label: "Generated",
                  value: (point) => point.generationTokensPerSecond,
                },
              ]}
            />
          </>
        )}
      </div>
    </section>
  );
}

/** Series drawn in amber; the rest use the default cyan. */
const AMBER_SERIES = new Set(["waiting", "generation"]);

export function LlmHistoryChart({
  label,
  history,
  series,
  max,
  range = CHART_RANGES[0],
}: {
  label: string;
  history: LlmHistoryPoint[];
  series: ChartSeries[];
  /** Fixed y-axis top; otherwise the axis follows the data. */
  max?: number;
  range?: ChartRange;
}) {
  return (
    <HistoryChart
      label={label}
      points={history}
      max={max}
      range={range}
      series={series.map((item) => ({
        ...item,
        tone: AMBER_SERIES.has(item.key) ? "amber" : "cyan",
      }))}
    />
  );
}

function Field({
  label,
  value,
  title,
}: {
  label: string;
  value: string;
  title?: string;
}) {
  return (
    <div title={title}>
      <span>{label}</span>
      <strong title={value}>{value}</strong>
    </div>
  );
}

export function LlmRuntimePopover({
  telemetry,
  instances,
  onInstancesChange,
  anchorRef,
  onClose,
  onPopOut,
}: {
  telemetry: LlmTelemetry | null;
  instances: LlmInstance[];
  onInstancesChange: (instances: LlmInstance[]) => void;
  anchorRef: RefObject<HTMLElement | null>;
  onClose: () => void;
  onPopOut?: () => void;
}) {
  const setMessage = useSessionStore((state) => state.setMessage);
  const refreshNow = () => {
    invoke("refresh_llm_telemetry").catch((error) =>
      setMessage({
        kind: "error",
        text: `Could not refresh LLM runtimes: ${String(error)}`,
      }),
    );
  };
  return (
    <ResourceDetailPopover
      anchorRef={anchorRef}
      ariaLabel="LLM runtime details"
      title="LLM runtimes"
      icon={<Boxes size={16} />}
      className="llm-detail-popover"
      headerActions={
        <button
          type="button"
          className="icon-button ghost"
          title="Poll every enabled instance now"
          aria-label="Refresh LLM runtimes"
          onClick={refreshNow}
        >
          <RefreshCw size={14} />
        </button>
      }
      onClose={onClose}
      onPopOut={onPopOut}
    >
      <LlmRuntimeDetailContent
        telemetry={telemetry}
        instances={instances}
        onInstancesChange={onInstancesChange}
      />
    </ResourceDetailPopover>
  );
}
