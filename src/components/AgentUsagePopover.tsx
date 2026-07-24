import { Bot } from "lucide-react";
import type { RefObject } from "react";
import type { AgentMetric, AgentWorkMetric } from "../types/gpu";
import { formatBytes } from "../utils/formatBytes";
import { formatPercent } from "../utils/format";
import { DetailUsageBar, ResourceDetailPopover } from "./ResourceDetailPopover";

type AgentUsagePopoverProps = {
  agents: AgentMetric[];
  error?: string | null;
  anchorRef: RefObject<HTMLElement | null>;
  onClose: () => void;
  onPopOut?: () => void;
};

export function AgentDetailContent({
  agents,
  error,
}: {
  agents: AgentMetric[];
  error?: string | null;
}) {
  if (agents.length === 0) {
    return (
      <div className="resource-unavailable">
        <strong>No coding agents detected</strong>
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

          <AgentUsageDetails agent={agent} />

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

function AgentUsageDetails({ agent }: { agent: AgentMetric }) {
  return (
    <div className="agent-provider-details">
      <ContextRemaining agent={agent} />
      <RateLimits provider={agent.provider} limits={agent.rateLimits} />
      <TokenSummary agent={agent} />

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

function RateLimits({
  provider,
  limits,
}: {
  provider: AgentMetric["provider"];
  limits: AgentMetric["rateLimits"];
}) {
  if (limits.length === 0) {
    return (
      <section className="agent-rate-limits unavailable" aria-label="Usage limits">
        <span>Usage limits</span>
        <small>No quota snapshot reported</small>
      </section>
    );
  }

  const grouped = groupLimits(limits);
  return (
    <section className="agent-rate-limits" aria-label="Usage limits">
      {grouped.map(({ group, items }) => (
        <section className="agent-rate-limit-group" key={group ?? "default"}>
          {group && <h4>{formatGroupLabel(group)}</h4>}
          <div className="agent-rate-limit-grid">
            {sortLimits(provider, items).map((limit) => {
              const remainingPercent =
                limit.usedPercent == null
                  ? null
                  : Math.max(0, Math.min(100, 100 - limit.usedPercent));
              return (
                <RemainingGauge
                  key={`${limit.group ?? ""}:${limit.label}:${limit.windowMinutes ?? ""}`}
                  label={formatRateLimitLabel(limit.label, limit.windowMinutes)}
                  remainingPercent={remainingPercent}
                  detail={
                    limit.resetsAt == null
                      ? limit.windowMinutes == null
                        ? "Reset time unavailable"
                        : `${formatWindow(limit.windowMinutes)} window`
                      : `Resets ${formatResetCountdown(limit.resetsAt)}`
                  }
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

function RemainingGauge({
  label,
  remainingPercent,
  detail,
  title,
}: {
  label: string;
  remainingPercent: number | null;
  detail: string;
  title?: string;
}) {
  const value = remainingPercent == null ? null : Math.max(0, Math.min(100, remainingPercent));
  const level = remainingLevel(value);
  return (
    <div className={`agent-remaining-gauge ${level}`} title={title}>
      <div>
        <span>{label}</span>
        <strong>{value == null ? "n/a" : `${formatGaugePercent(value)} remaining`}</strong>
      </div>
      <DetailUsageBar
        value={value}
        level={level}
        ariaLabel={`${label}: ${value == null ? "unavailable" : `${formatGaugePercent(value)} remaining`}`}
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
  agents,
  error,
  anchorRef,
  onClose,
  onPopOut,
}: AgentUsagePopoverProps) {
  return (
    <ResourceDetailPopover
      anchorRef={anchorRef}
      ariaLabel="Coding agents"
      title="Coding agents"
      icon={<Bot size={16} />}
      onClose={onClose}
      onPopOut={onPopOut}
      className="agent-detail-popover"
    >
      <AgentDetailContent agents={agents} error={error} />
    </ResourceDetailPopover>
  );
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

function shorten(value: string | null) {
  if (!value || value.length <= 18) return value;
  return `${value.slice(0, 8)}…${value.slice(-6)}`;
}

function formatReset(value: number) {
  const milliseconds = value > 10_000_000_000 ? value : value * 1000;
  return new Date(milliseconds).toLocaleString([], {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function formatResetCountdown(value: number) {
  const milliseconds = value > 10_000_000_000 ? value : value * 1000;
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
    maximumFractionDigits: 1,
  }).format(value)}%`;
}

function remainingLevel(value: number | null) {
  if (value == null) return "unknown" as const;
  if (value <= 10) return "critical" as const;
  if (value <= 25) return "warning" as const;
  return "normal" as const;
}

function groupLimits(limits: AgentMetric["rateLimits"]) {
  const groups = new Map<string | null, AgentMetric["rateLimits"]>();
  for (const limit of limits) {
    const current = groups.get(limit.group) ?? [];
    current.push(limit);
    groups.set(limit.group, current);
  }
  return Array.from(groups, ([group, items]) => ({ group, items }));
}

function sortLimits(
  provider: AgentMetric["provider"],
  limits: AgentMetric["rateLimits"],
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
