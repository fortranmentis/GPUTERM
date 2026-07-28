import { invoke } from "@tauri-apps/api/core";
import { Bot, RefreshCw, Settings2 } from "lucide-react";
import { useState, type RefObject } from "react";
import type {
  AgentMetric,
  AgentQuotaHistoryPoint,
  AgentWorkMetric,
} from "../types/gpu";
import { formatBytes } from "../utils/formatBytes";
import { formatPercent } from "../utils/format";
import { DetailUsageBar, ResourceDetailPopover } from "./ResourceDetailPopover";

type AgentUsagePopoverProps = {
  sessionId: string;
  agents: AgentMetric[];
  error?: string | null;
  anchorRef: RefObject<HTMLElement | null>;
  onClose: () => void;
  onPopOut?: () => void;
};

export function AgentDetailContent({
  sessionId,
  agents,
  error,
}: {
  sessionId: string;
  agents: AgentMetric[];
  error?: string | null;
}) {
  const [busyProvider, setBusyProvider] = useState<string | null>(null);
  const [actionMessage, setActionMessage] = useState<Record<string, string>>({});

  const refreshQuota = async (agent: AgentMetric) => {
    const key = `${agent.provider}:${agent.rootPid}`;
    setBusyProvider(agent.provider);
    try {
      await invoke("refresh_agent_quota", {
        sessionId,
        provider: agent.provider,
      });
      setActionMessage((current) => ({
        ...current,
        [key]: "Refresh requested. The next telemetry update will contain the new quota.",
      }));
    } catch (error) {
      setActionMessage((current) => ({ ...current, [key]: String(error) }));
    } finally {
      setBusyProvider(null);
    }
  };

  const setupClaude = async (agent: AgentMetric) => {
    const key = `${agent.provider}:${agent.rootPid}`;
    setBusyProvider(agent.provider);
    try {
      const result = await invoke<{ status: string; message: string }>(
        "configure_claude_quota_monitor",
        { sessionId },
      );
      setActionMessage((current) => ({ ...current, [key]: result.message }));
    } catch (error) {
      setActionMessage((current) => ({ ...current, [key]: String(error) }));
    } finally {
      setBusyProvider(null);
    }
  };

  if (agents.length === 0) {
    return (
      <div className="resource-unavailable">
        <strong>No AI agents detected</strong>
        <span>{error ?? "Start agy, codex, or claude on this host to monitor it."}</span>
      </div>
    );
  }

  return (
    <div className="agent-detail-list">
      {agents.map((agent) => (
        <article className="agent-detail-card" key={`${agent.provider}:${agent.rootPid}`}>
          <header>
            <div>
              <span className={`agent-provider-tag ${agent.provider}`}>
                {agent.displayName}
              </span>
              <strong>{agent.model ?? "Model unavailable"}</strong>
            </div>
            <span className={`agent-status ${normalizeStatus(agent.status)}`}>
              {agent.status}
            </span>
          </header>

          <AgentUsageDetails
            agent={agent}
            busy={busyProvider === agent.provider}
            actionMessage={actionMessage[`${agent.provider}:${agent.rootPid}`]}
            onRefresh={() => void refreshQuota(agent)}
            onSetupClaude={() => void setupClaude(agent)}
          />

          <div className="agent-resource-grid">
            <AgentField label="CPU" value={formatPercent(agent.cpuPercent, 1)} />
            <AgentField label="Memory" value={formatBytes(agent.memoryBytes)} />
            <AgentField label="Processes" value={String(agent.processCount)} />
            <AgentField label="PID" value={String(agent.rootPid)} />
            <AgentField label="Running" value={formatDuration(agent.elapsedSeconds)} />
            <AgentField label="User" value={agent.user ?? "n/a"} />
          </div>

          <div className="agent-session-lines">
            <span title={agent.sessionId ?? undefined}>
              Session <strong>{shorten(agent.sessionId) ?? "n/a"}</strong>
            </span>
            <span title={agent.cwd ?? undefined}>
              Workspace <strong>{agent.cwd ?? "n/a"}</strong>
            </span>
          </div>
        </article>
      ))}
    </div>
  );
}

function AgentUsageDetails({
  agent,
  busy,
  actionMessage,
  onRefresh,
  onSetupClaude,
}: {
  agent: AgentMetric;
  busy: boolean;
  actionMessage?: string;
  onRefresh: () => void;
  onSetupClaude: () => void;
}) {
  return (
    <div className="agent-provider-details">
      <QuotaDetails
        provider={agent.provider}
        quota={agent.quota}
        busy={busy}
        onRefresh={onRefresh}
        onSetupClaude={onSetupClaude}
      />
      {actionMessage && <div className="agent-quota-action-message">{actionMessage}</div>}

      {agent.provider === "agy" && <AgyUsageHistory history={agent.quota.history} />}

      <details className="agent-context-details">
        <summary>Context details</summary>
        <ContextRemaining agent={agent} />
        <TokenSummary agent={agent} />
      </details>

      {agent.provider === "agy" && (
        <>
          <WorkList title="Subagents" items={agent.subagents} />
          <WorkList title="Background tasks" items={agent.backgroundTasks} />
        </>
      )}

      {agent.provider === "claude" && (
        <div className="agent-resource-grid compact">
          <AgentField
            label="Session cost"
            value={agent.costUsd == null ? "n/a" : `$${agent.costUsd.toFixed(4)}`}
          />
          <AgentField
            label="Session time"
            value={formatDuration(agent.sessionDurationSeconds)}
          />
        </div>
      )}
    </div>
  );
}

function ContextRemaining({ agent }: { agent: AgentMetric }) {
  const remainingPercent =
    agent.contextRemainingPercent ??
    (agent.contextUsedPercent == null
      ? null
      : Math.max(0, Math.min(100, 100 - agent.contextUsedPercent)));
  let detail = "Context data unavailable";
  if (agent.contextRemainingTokens != null && agent.contextWindowTokens != null) {
    detail = `${formatTokens(agent.contextRemainingTokens)} of ${formatTokens(
      agent.contextWindowTokens,
    )} tokens left`;
  } else if (agent.contextRemainingTokens != null) {
    detail = `${formatTokens(agent.contextRemainingTokens)} tokens left`;
  } else if (agent.contextWindowTokens != null && agent.contextUsedTokens != null) {
    detail = `${formatTokens(
      Math.max(0, agent.contextWindowTokens - agent.contextUsedTokens),
    )} of ${formatTokens(agent.contextWindowTokens)} tokens left`;
  }

  return (
    <section className="agent-context-remaining" aria-label="Context remaining">
      <RemainingGauge
        label="Context remaining"
        remainingPercent={remainingPercent}
        detail={detail}
      />
    </section>
  );
}

function TokenSummary({ agent }: { agent: AgentMetric }) {
  // Providers that report a real cumulative count get a session total. Claude
  // reports per-request usage only, so its card shows the latest request rather
  // than a total the monitor cannot measure.
  if (agent.totalTokens != null || agent.inputTokens != null || agent.outputTokens != null) {
    return (
      <section className="agent-token-summary">
        <span>Session tokens</span>
        <div className="agent-resource-grid compact tokens">
          <AgentField label="Input" value={formatTokens(agent.inputTokens)} />
          <AgentField label="Output" value={formatTokens(agent.outputTokens)} />
          <AgentField label="Total" value={formatTokens(agent.totalTokens)} />
        </div>
      </section>
    );
  }

  const hasLastRequest =
    agent.lastRequestInputTokens != null ||
    agent.lastRequestOutputTokens != null ||
    agent.lastRequestCacheReadTokens != null ||
    agent.lastRequestCacheCreationTokens != null;
  if (!hasLastRequest) return null;

  return (
    <section className="agent-token-summary">
      <span>Last request</span>
      <div className="agent-resource-grid compact tokens">
        <AgentField label="Input" value={formatTokens(agent.lastRequestInputTokens)} />
        <AgentField label="Cache read" value={formatTokens(agent.lastRequestCacheReadTokens)} />
        <AgentField
          label="Cache write"
          value={formatTokens(agent.lastRequestCacheCreationTokens)}
        />
        <AgentField label="Output" value={formatTokens(agent.lastRequestOutputTokens)} />
      </div>
    </section>
  );
}

function QuotaDetails({
  provider,
  quota,
  busy,
  onRefresh,
  onSetupClaude,
}: {
  provider: AgentMetric["provider"];
  quota: AgentMetric["quota"];
  busy: boolean;
  onRefresh: () => void;
  onSetupClaude: () => void;
}) {
  const limits = quota.limits;
  if (limits.length === 0) {
    return (
      <section className="agent-rate-limits unavailable" aria-label="Usage limits">
        <QuotaSectionHeader
          quota={quota}
          busy={busy}
          onRefresh={onRefresh}
          onSetupClaude={provider === "claude" ? onSetupClaude : undefined}
        />
        <small>{quota.message ?? "No quota snapshot reported"}</small>
        {provider === "agy" && (
          <small className="agent-quota-command">
            Open the active AGY terminal and run <code>/usage</code>.
          </small>
        )}
      </section>
    );
  }

  const grouped = groupLimits(limits);
  return (
    <section className="agent-rate-limits" aria-label="Usage limits">
      <QuotaSectionHeader
        quota={quota}
        busy={busy}
        onRefresh={onRefresh}
        onSetupClaude={provider === "claude" ? onSetupClaude : undefined}
      />
      {grouped.map(({ group, items }) => (
        <section className="agent-rate-limit-group" key={group ?? "default"}>
          {group && (
            <div className="agent-rate-limit-group-heading">
              <h4>{formatGroupLabel(group)}</h4>
              {uniqueModelNames(items).length > 0 && (
                <small>{uniqueModelNames(items).join(" · ")}</small>
              )}
            </div>
          )}
          <div className="agent-rate-limit-grid">
            {sortLimits(provider, items).map((limit) => {
              const remainingPercent =
                limit.stale ? null : limit.remainingPercent;
              return (
                <RemainingGauge
                  key={`${limit.group ?? ""}:${limit.label}:${limit.windowMinutes ?? ""}`}
                  label={formatRateLimitLabel(limit.label, limit.windowMinutes)}
                  remainingPercent={remainingPercent}
                  unavailableText={limit.stale ? "window reset" : undefined}
                  detail={formatLimitDetail(limit, quota.snapshotAgeSeconds)}
                  title={limit.resetsAt == null ? undefined : formatReset(limit.resetsAt)}
                />
              );
            })}
          </div>
        </section>
      ))}
    </section>
  );
}

function QuotaSectionHeader({
  quota,
  busy,
  onRefresh,
  onSetupClaude,
}: {
  quota: AgentMetric["quota"];
  busy: boolean;
  onRefresh: () => void;
  onSetupClaude?: () => void;
}) {
  return (
    <header className="agent-quota-header">
      <div>
        <strong>Usage remaining</strong>
        <small>
          {formatQuotaSource(quota.source)}
          {quota.snapshotAgeSeconds != null
            ? ` · ${formatAge(quota.snapshotAgeSeconds)} ago`
            : ""}
        </small>
      </div>
      <div className="agent-quota-actions">
        {onSetupClaude && quota.status === "setup-required" && (
          <button type="button" disabled={busy} onClick={onSetupClaude}>
            <Settings2 size={13} />
            Set up
          </button>
        )}
        <button type="button" disabled={busy} onClick={onRefresh}>
          <RefreshCw size={13} className={busy ? "spin" : undefined} />
          Refresh
        </button>
      </div>
    </header>
  );
}

function formatLimitDetail(
  limit: AgentMetric["quota"]["limits"][number],
  snapshotAgeSeconds: number | null,
) {
  if (limit.stale) return "Window rolled over — awaiting a new report";
  const base =
    limit.resetsAt == null
      ? limit.windowMinutes == null
        ? "Reset time unavailable"
        : `${formatWindow(limit.windowMinutes)} window`
      : `Resets ${formatResetCountdown(limit.resetsAt)}`;
  // A status line only refreshes while its session is doing work, so an aging
  // snapshot is worth saying out loud rather than presenting as live.
  if (snapshotAgeSeconds != null && snapshotAgeSeconds > 120) {
    return `${base} · as of ${formatAge(snapshotAgeSeconds)} ago`;
  }
  return base;
}

function formatQuotaSource(source: AgentMetric["quota"]["source"]) {
  switch (source) {
    case "codex-app-server":
      return "Live Codex account";
    case "codex-session-log":
      return "Codex session-log fallback";
    case "claude-statusline":
      return "Claude status line";
    case "agy-usage-tui":
      return "Experimental AGY /usage";
    default:
      return "Provider data unavailable";
  }
}

function RemainingGauge({
  label,
  remainingPercent,
  detail,
  title,
  unavailableText,
}: {
  label: string;
  remainingPercent: number | null;
  detail: string;
  title?: string;
  unavailableText?: string;
}) {
  const value = remainingPercent == null ? null : Math.max(0, Math.min(100, remainingPercent));
  const level = remainingLevel(value);
  const reading =
    value == null ? (unavailableText ?? "n/a") : `${formatGaugePercent(value)} remaining`;
  return (
    <div className={`agent-remaining-gauge ${level}`} title={title}>
      <div>
        <span>{label}</span>
        <strong>{reading}</strong>
      </div>
      <DetailUsageBar
        value={value}
        level={level}
        ariaLabel={`${label}: ${value == null ? (unavailableText ?? "unavailable") : `${formatGaugePercent(value)} remaining`}`}
      />
      <small>{detail}</small>
    </div>
  );
}

function WorkList({ title, items }: { title: string; items: AgentWorkMetric[] }) {
  return (
    <section className="agent-work-list">
      <strong>{title}</strong>
      {items.length === 0 ? (
        <span>None reported</span>
      ) : (
        items.map((item, index) => (
          <div key={`${item.name}:${index}`}>
            <span title={item.name}>{item.name}</span>
            <small>{item.role ?? item.status ?? "running"}</small>
          </div>
        ))
      )}
    </section>
  );
}

function AgentField({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  );
}

export function AgentUsagePopover({
  sessionId,
  agents,
  error,
  anchorRef,
  onClose,
  onPopOut,
}: AgentUsagePopoverProps) {
  return (
    <ResourceDetailPopover
      anchorRef={anchorRef}
      ariaLabel="AI DASH"
      title="AI DASH"
      icon={<Bot size={16} />}
      onClose={onClose}
      onPopOut={onPopOut}
      className="agent-detail-popover"
    >
      <AgentDetailContent sessionId={sessionId} agents={agents} error={error} />
    </ResourceDetailPopover>
  );
}

const AGY_HISTORY_GROUPS = [
  {
    key: "gemini",
    label: "Gemini",
    matches: (group: string | null) => group?.toLowerCase().includes("gemini") === true,
  },
  {
    key: "claude-gpt",
    label: "Claude/GPT",
    matches: (group: string | null) => {
      const normalized = group?.toLowerCase() ?? "";
      return normalized.includes("claude") || normalized.includes("gpt");
    },
  },
] as const;

function AgyUsageHistory({ history }: { history: AgentQuotaHistoryPoint[] }) {
  const ordered = [...history].sort((left, right) => left.capturedAt - right.capturedAt);
  return (
    <section className="agy-usage-history" aria-label="AGY usage history">
      <header>
        <strong>Usage history</strong>
        <small>Last 24h · 5-minute samples · memory only</small>
      </header>
      <div className="agy-history-chart-list">
        <AgyHistoryChart
          label="5-hour remaining"
          windowMinutes={5 * 60}
          history={ordered}
        />
        <AgyHistoryChart
          label="Weekly remaining"
          windowMinutes={7 * 24 * 60}
          history={ordered}
        />
      </div>
    </section>
  );
}

function AgyHistoryChart({
  label,
  windowMinutes,
  history,
}: {
  label: string;
  windowMinutes: number;
  history: AgentQuotaHistoryPoint[];
}) {
  const width = 320;
  const height = 72;
  const left = 24;
  const right = 6;
  const top = 7;
  const bottom = 14;
  const plotWidth = width - left - right;
  const plotHeight = height - top - bottom;
  const now = Math.max(
    Math.floor(Date.now() / 1000),
    history.at(-1)?.capturedAt ?? 0,
  );
  const start = now - 24 * 60 * 60;
  const x = (capturedAt: number) =>
    left + Math.max(0, Math.min(1, (capturedAt - start) / (24 * 60 * 60))) * plotWidth;
  const y = (remaining: number) =>
    top + (1 - Math.max(0, Math.min(100, remaining)) / 100) * plotHeight;
  const series = AGY_HISTORY_GROUPS.map((group) => {
    const segments = historySegments(history, windowMinutes, group.matches);
    const values = segments.flat();
    const latest = values.at(-1)?.remainingPercent ?? null;
    return { ...group, segments, latest };
  });
  const successfulPoints = new Set(
    series.flatMap((item) =>
      item.segments.flat().map((point) => point.capturedAt),
    ),
  ).size;

  return (
    <section className="agy-history-chart">
      <div className="agy-history-chart-heading">
        <strong>{label}</strong>
        <div className="agy-history-legend">
          {series.map((item) => (
            <span className={item.key} key={item.key}>
              {item.label} {item.latest == null ? "n/a" : `${formatGaugePercent(item.latest)}`}
            </span>
          ))}
        </div>
      </div>
      <svg
        viewBox={`0 0 ${width} ${height}`}
        role="img"
        aria-label={`AGY ${label} trend over the last 24 hours`}
      >
        <title>{`AGY ${label} trend over the last 24 hours`}</title>
        {[0, 50, 100].map((value) => (
          <g key={value}>
            <line
              className="agy-history-grid-line"
              x1={left}
              x2={width - right}
              y1={y(value)}
              y2={y(value)}
            />
            <text x={left - 4} y={y(value) + 3} textAnchor="end">
              {value}
            </text>
          </g>
        ))}
        <text x={left} y={height - 2}>−24h</text>
        <text x={width - right} y={height - 2} textAnchor="end">now</text>
        {series.flatMap((item) =>
          item.segments.map((segment, index) => {
            const path = segment
              .map(
                (point, pointIndex) =>
                  `${pointIndex === 0 ? "M" : "L"} ${x(point.capturedAt).toFixed(2)} ${y(
                    point.remainingPercent,
                  ).toFixed(2)}`,
              )
              .join(" ");
            const last = segment.at(-1);
            return (
              <g className={`agy-history-series ${item.key}`} key={`${item.key}:${index}`}>
                <path d={path} />
                {last && (
                  <circle
                    cx={x(last.capturedAt)}
                    cy={y(last.remainingPercent)}
                    r={1.8}
                  />
                )}
              </g>
            );
          }),
        )}
      </svg>
      {successfulPoints < 2 && (
        <small className="agy-history-pending">
          Trend appears after the next successful 5-minute sample.
        </small>
      )}
    </section>
  );
}

type AgyHistoryValue = {
  capturedAt: number;
  remainingPercent: number;
};

function historySegments(
  history: AgentQuotaHistoryPoint[],
  windowMinutes: number,
  matchesGroup: (group: string | null) => boolean,
) {
  const segments: AgyHistoryValue[][] = [];
  let current: AgyHistoryValue[] = [];
  const flush = () => {
    if (current.length > 0) segments.push(current);
    current = [];
  };
  for (const point of history) {
    const limit =
      point.status === "available"
        ? point.limits.find(
            (candidate) =>
              candidate.windowMinutes === windowMinutes &&
              matchesGroup(candidate.group) &&
              candidate.remainingPercent != null,
          )
        : undefined;
    if (limit?.remainingPercent == null) {
      flush();
      continue;
    }
    current.push({
      capturedAt: point.capturedAt,
      remainingPercent: limit.remainingPercent,
    });
  }
  flush();
  return segments;
}

function normalizeStatus(status: string) {
  const normalized = status.toLowerCase();
  if (["active", "working", "thinking", "tool_use", "running"].includes(normalized)) {
    return "active";
  }
  return "idle";
}

function formatTokens(value: number | null) {
  return value == null ? "n/a" : Intl.NumberFormat("en", { notation: "compact" }).format(value);
}

function formatDuration(value: number | null) {
  if (value == null) return "n/a";
  const seconds = Math.max(0, Math.round(value));
  const days = Math.floor(seconds / 86_400);
  const hours = Math.floor((seconds % 86_400) / 3_600);
  const minutes = Math.floor((seconds % 3_600) / 60);
  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${minutes}m`;
  return `${minutes}m`;
}

function formatAge(seconds: number) {
  if (seconds < 60) return "less than 1m";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  const remainder = minutes % 60;
  return remainder > 0 ? `${hours}h ${remainder}m` : `${hours}h`;
}

function shorten(value: string | null) {
  if (!value || value.length <= 18) return value;
  return `${value.slice(0, 8)}…${value.slice(-6)}`;
}

function formatReset(value: number) {
  // Rust normalizes every provider reset timestamp to epoch seconds.
  const milliseconds = value * 1000;
  return new Date(milliseconds).toLocaleString([], {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function formatResetCountdown(value: number) {
  // Rust normalizes every provider reset timestamp to epoch seconds.
  const milliseconds = value * 1000;
  const remainingSeconds = Math.max(0, Math.round((milliseconds - Date.now()) / 1000));
  if (remainingSeconds === 0) return "now";
  const days = Math.floor(remainingSeconds / 86_400);
  const hours = Math.floor((remainingSeconds % 86_400) / 3_600);
  const minutes = Math.floor((remainingSeconds % 3_600) / 60);
  if (days > 0) return hours > 0 ? `in ${days}d ${hours}h` : `in ${days}d`;
  if (hours > 0) return minutes > 0 ? `in ${hours}h ${minutes}m` : `in ${hours}h`;
  return `in ${Math.max(1, minutes)}m`;
}

function formatLimitLabel(value: string) {
  return value
    .replace(/([a-z])([A-Z])/g, "$1 $2")
    .replaceAll("_", " ")
    .replace(/\b\w/g, (letter) => letter.toUpperCase());
}

function formatGroupLabel(value: string) {
  return value.includes("_") || /[a-z][A-Z]/.test(value) ? formatLimitLabel(value) : value;
}

function formatRateLimitLabel(label: string, windowMinutes: number | null) {
  if (windowMinutes === 5 * 60) return "5-hour limit";
  if (windowMinutes === 7 * 24 * 60) return "Weekly limit";
  return formatLimitLabel(label);
}

function formatWindow(minutes: number) {
  if (minutes % (7 * 24 * 60) === 0) {
    return pluralize(minutes / (7 * 24 * 60), "week");
  }
  if (minutes % (24 * 60) === 0) return pluralize(minutes / (24 * 60), "day");
  if (minutes % 60 === 0) return pluralize(minutes / 60, "hour");
  return `${minutes} min`;
}

function pluralize(value: number, unit: string) {
  return `${value} ${unit}${value === 1 ? "" : "s"}`;
}

function formatGaugePercent(value: number) {
  return `${Intl.NumberFormat("en", {
    minimumFractionDigits: value % 1 === 0 ? 0 : 1,
    maximumFractionDigits: 2,
  }).format(value)}%`;
}

function remainingLevel(value: number | null) {
  if (value == null) return "unknown" as const;
  if (value <= 10) return "critical" as const;
  if (value <= 25) return "warning" as const;
  return "normal" as const;
}

function groupLimits(limits: AgentMetric["quota"]["limits"]) {
  const groups = new Map<string | null, AgentMetric["quota"]["limits"]>();
  for (const limit of limits) {
    const current = groups.get(limit.group) ?? [];
    current.push(limit);
    groups.set(limit.group, current);
  }
  return Array.from(groups, ([group, items]) => ({ group, items }));
}

function uniqueModelNames(limits: AgentMetric["quota"]["limits"]) {
  return [...new Set(limits.flatMap((limit) => limit.modelNames ?? []))];
}

function sortLimits(
  provider: AgentMetric["provider"],
  limits: AgentMetric["quota"]["limits"],
) {
  const preferredWindows =
    provider === "agy" ? [7 * 24 * 60, 5 * 60] : [5 * 60, 7 * 24 * 60];
  return [...limits].sort((left, right) => {
    const leftIndex = preferredWindows.indexOf(left.windowMinutes ?? -1);
    const rightIndex = preferredWindows.indexOf(right.windowMinutes ?? -1);
    return (
      (leftIndex < 0 ? preferredWindows.length : leftIndex) -
      (rightIndex < 0 ? preferredWindows.length : rightIndex)
    );
  });
}
