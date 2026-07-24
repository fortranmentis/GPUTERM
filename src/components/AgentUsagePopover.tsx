import { Bot } from "lucide-react";
import type { RefObject } from "react";
import type { AgentMetric, AgentWorkMetric } from "../types/gpu";
import { formatBytes } from "../utils/formatBytes";
import { formatPercent } from "../utils/format";
import { ResourceDetailPopover } from "./ResourceDetailPopover";

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

          <AgentUsageDetails agent={agent} />
        </article>
      ))}
    </div>
  );
}

function AgentUsageDetails({ agent }: { agent: AgentMetric }) {
  if (agent.provider === "agy") {
    return (
      <div className="agent-provider-details">
        <TokenSummary agent={agent} />
        <WorkList title="Subagents" items={agent.subagents} />
        <WorkList title="Background tasks" items={agent.backgroundTasks} />
      </div>
    );
  }

  if (agent.provider === "claude") {
    return (
      <div className="agent-provider-details">
        <TokenSummary agent={agent} />
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
      </div>
    );
  }

  return (
    <div className="agent-provider-details">
      <TokenSummary agent={agent} />
      {agent.rateLimits.length > 0 && (
        <div className="agent-rate-limits">
          {agent.rateLimits.map((limit) => (
            <div key={limit.label}>
              <span>{limit.label}</span>
              <strong>
                {limit.usedPercent == null ? "n/a" : `${limit.usedPercent.toFixed(0)}% used`}
              </strong>
              <small>
                {limit.windowMinutes == null ? "" : `${limit.windowMinutes} min window`}
                {limit.resetsAt == null ? "" : ` · resets ${formatReset(limit.resetsAt)}`}
              </small>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function TokenSummary({ agent }: { agent: AgentMetric }) {
  return (
    <div className="agent-resource-grid compact">
      <AgentField label="Input tokens" value={formatTokens(agent.inputTokens)} />
      <AgentField label="Output tokens" value={formatTokens(agent.outputTokens)} />
      <AgentField label="Total tokens" value={formatTokens(agent.totalTokens)} />
      <AgentField
        label="Context"
        value={
          agent.contextUsedPercent == null
            ? `${formatTokens(agent.contextUsedTokens)} / ${formatTokens(agent.contextWindowTokens)}`
            : `${agent.contextUsedPercent.toFixed(1)}%`
        }
      />
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
  return new Date(milliseconds).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
  });
}
