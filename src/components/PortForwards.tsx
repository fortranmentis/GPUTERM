import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  AlertTriangle,
  ArrowLeftRight,
  ChevronDown,
  ChevronRight,
  LoaderCircle,
  Pencil,
  Play,
  Plus,
  Square,
  X,
} from "lucide-react";
import { useSessionStore } from "../stores/sessionStore";
import type {
  ForwardKind,
  ForwardSpec,
  ForwardStatus,
  SessionProfile,
} from "../types/session";
import { formatFileSize } from "../utils/formatBytes";

type PortForwardsProps = {
  profile: SessionProfile;
  connected: boolean;
};

type Draft = {
  id: string;
  kind: ForwardKind;
  bindHost: string;
  bindPort: string;
  destHost: string;
  destPort: string;
  enabled: boolean;
  /** Start it now without saving it on the profile. */
  temporary: boolean;
};

const POLL_INTERVAL_MS = 2000;

const KIND_LABELS: Record<ForwardKind, string> = {
  local: "Local (-L)",
  remote: "Remote (-R)",
  dynamic: "SOCKS5 (-D)",
};

const blankDraft = (temporary: boolean): Draft => ({
  id: "",
  kind: "local",
  bindHost: "127.0.0.1",
  bindPort: "",
  destHost: "localhost",
  destPort: "",
  enabled: true,
  temporary,
});

/** `-L 8888:localhost:8888` style summary of a rule. */
export function describeForward(spec: ForwardSpec): string {
  const bind = `${spec.bindHost}:${spec.bindPort}`;
  if (spec.kind === "dynamic") {
    return `SOCKS5 on ${bind}`;
  }
  const dest = `${spec.destHost ?? "?"}:${spec.destPort ?? "?"}`;
  return spec.kind === "local"
    ? `${bind} → ${dest} (via server)`
    : `server ${bind} → ${dest} (here)`;
}

function draftToSpec(draft: Draft): ForwardSpec {
  return {
    id: draft.id,
    kind: draft.kind,
    bindHost: draft.bindHost.trim() || "127.0.0.1",
    bindPort: Number(draft.bindPort),
    destHost: draft.kind === "dynamic" ? null : draft.destHost.trim(),
    destPort: draft.kind === "dynamic" ? null : Number(draft.destPort),
    enabled: draft.enabled,
  };
}

/**
 * Port forward rules of one profile: saved ones (started on connect) and,
 * while connected, the live forwards with their status and traffic.
 */
export function PortForwards({ profile, connected }: PortForwardsProps) {
  const setSessions = useSessionStore((state) => state.setSessions);
  const [open, setOpen] = useState(false);
  const [statuses, setStatuses] = useState<ForwardStatus[]>([]);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const saved = profile.forwards ?? [];

  // Bumped by every status request and by a profile switch. A reply applies
  // only if no newer request was made since, so a slow poll for the previous
  // profile (or one overtaken by a start/stop) cannot overwrite fresher state.
  const generationRef = useRef(0);

  const requestStatuses = useCallback(
    async (command: string, args: Record<string, unknown>) => {
      const generation = ++generationRef.current;
      const next = await invoke<ForwardStatus[]>(command, args);
      if (generation === generationRef.current) {
        setStatuses(next);
      }
    },
    [],
  );

  const refresh = useCallback(async () => {
    try {
      await requestStatuses("list_forwards", { profileId: profile.id });
    } catch {
      // Status is best effort; the next poll retries.
    }
  }, [profile.id, requestStatuses]);

  useEffect(() => {
    generationRef.current += 1;
    setDraft(null);
    setError(null);
    setStatuses([]);
  }, [profile.id]);

  useEffect(() => {
    if (!open || !connected) {
      return;
    }
    void refresh();
    const timer = window.setInterval(() => void refresh(), POLL_INTERVAL_MS);
    return () => window.clearInterval(timer);
  }, [connected, open, refresh]);

  const run = async (action: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await action();
    } catch (actionError) {
      setError(String(actionError));
    } finally {
      setBusy(false);
    }
  };

  const saveRules = async (forwards: ForwardSpec[]) => {
    const profiles = await invoke<SessionProfile[]>("save_profile_forwards", {
      profileId: profile.id,
      forwards,

      });
    setSessions(profiles);
  };

  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (!draft) {
      return;
    }
    void run(async () => {
      const spec = await invoke<ForwardSpec>("validate_forward", {
        spec: draftToSpec(draft),
      });
      if (draft.temporary) {
        await requestStatuses("start_forward", {
          profileId: profile.id,
          spec,
        });
      } else {
        const exists = saved.some((rule) => rule.id === spec.id);
        await saveRules(
          exists
            ? saved.map((rule) => (rule.id === spec.id ? spec : rule))
            : [...saved, spec],
        );
        if (connected && spec.enabled) {
          await requestStatuses("start_forward", {
            profileId: profile.id,
            spec,
          });
        }
      }
      setDraft(null);
    });
  };

  const removeSaved = (rule: ForwardSpec) =>
    void run(async () => {
      await saveRules(saved.filter((item) => item.id !== rule.id));
      if (connected) {
        await requestStatuses("stop_forward", {
          profileId: profile.id,
          forwardId: rule.id,
        });
      }
    });

  const startRule = (spec: ForwardSpec) =>
    void run(async () => {
      await requestStatuses("start_forward", {
        profileId: profile.id,
        spec,
      });
    });

  const stopRule = (forwardId: string) =>
    void run(async () => {
      await requestStatuses("stop_forward", {
        profileId: profile.id,
        forwardId,
      });
    });

  const statusFor = (id: string) =>
    statuses.find((status) => status.spec.id === id);
  const temporary = statuses.filter((status) => status.temporary);
  const runningCount = statuses.filter(
    (status) => status.state === "listening" || status.state === "starting",
  ).length;

  return (
    <section className="port-forwards">
      <button
        className="port-forwards-toggle"
        type="button"
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
      >
        {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        <ArrowLeftRight size={14} />
        <span>Port forwards</span>
        <small>
          {saved.length} saved
          {connected && runningCount > 0 ? ` · ${runningCount} active` : ""}
        </small>
      </button>
      {open && (
        <div className="port-forwards-body">
          {error && (
            <div className="port-forward-error" role="alert">
              {error}
            </div>
          )}
          {saved.length === 0 && temporary.length === 0 && !draft && (
            <small className="port-forwards-empty">
              No forwards. Saved rules start each time this session connects.
            </small>
          )}
          {saved.map((rule) => (
            <ForwardRow
              key={rule.id}
              spec={rule}
              status={connected ? statusFor(rule.id) : undefined}
              connected={connected}
              busy={busy}
              onStart={() => startRule(rule)}
              onStop={() => stopRule(rule.id)}
              onEdit={() =>
                setDraft({
                  id: rule.id,
                  kind: rule.kind,
                  bindHost: rule.bindHost,
                  bindPort: String(rule.bindPort),
                  destHost: rule.destHost ?? "",
                  destPort: rule.destPort == null ? "" : String(rule.destPort),
                  enabled: rule.enabled,
                  temporary: false,
                })
              }
              onRemove={() => removeSaved(rule)}
            />
          ))}
          {temporary.map((status) => (
            <ForwardRow
              key={status.spec.id}
              spec={status.spec}
              status={status}
              connected={connected}
              busy={busy}
              onStop={() => stopRule(status.spec.id)}
            />
          ))}
          {draft ? (
            <form className="port-forward-form" onSubmit={submit}>
              <label>
                <span>Type</span>
                <select
                  value={draft.kind}
                  onChange={(event) =>
                    setDraft({ ...draft, kind: event.target.value as ForwardKind })
                  }
                >
                  {(Object.keys(KIND_LABELS) as ForwardKind[]).map((kind) => (
                    <option key={kind} value={kind}>
                      {KIND_LABELS[kind]}
                    </option>
                  ))}
                </select>
              </label>
              <div className="field-row">
                <label>
                  <span>{draft.kind === "remote" ? "Server listen address" : "Listen address"}</span>
                  <input
                    value={draft.bindHost}
                    onChange={(event) => setDraft({ ...draft, bindHost: event.target.value })}
                  />
                </label>
                <label>
                  <span>Port</span>
                  <input
                    value={draft.bindPort}
                    inputMode="numeric"
                    placeholder="8888"
                    onChange={(event) => setDraft({ ...draft, bindPort: event.target.value })}
                  />
                </label>
              </div>
              {draft.kind !== "dynamic" && (
                <div className="field-row">
                  <label>
                    <span>
                      {draft.kind === "local" ? "Destination (from server)" : "Destination (from here)"}
                    </span>
                    <input
                      value={draft.destHost}
                      onChange={(event) => setDraft({ ...draft, destHost: event.target.value })}
                    />
                  </label>
                  <label>
                    <span>Port</span>
                    <input
                      value={draft.destPort}
                      inputMode="numeric"
                      placeholder="8888"
                      onChange={(event) => setDraft({ ...draft, destPort: event.target.value })}
                    />
                  </label>
                </div>
              )}
              {!draft.temporary && (
                <label className="port-forward-check">
                  <input
                    type="checkbox"
                    checked={draft.enabled}
                    onChange={(event) => setDraft({ ...draft, enabled: event.target.checked })}
                  />
                  <span>Start when this session connects</span>
                </label>
              )}
              <div className="button-row">
                <button className="primary-button" type="submit" disabled={busy}>
                  {busy && <LoaderCircle className="spin" size={15} />}
                  {draft.temporary ? "Start" : "Save"}
                </button>
                <button
                  className="secondary-button"
                  type="button"
                  onClick={() => setDraft(null)}
                >
                  Cancel
                </button>
              </div>
            </form>
          ) : (
            <div className="button-row">
              <button
                className="secondary-button compact"
                type="button"
                disabled={busy}
                onClick={() => setDraft(blankDraft(false))}
              >
                <Plus size={14} />
                Add rule
              </button>
              {connected && (
                <button
                  className="secondary-button compact"
                  type="button"
                  disabled={busy}
                  title="Start a forward for this connection only"
                  onClick={() => setDraft(blankDraft(true))}
                >
                  <Play size={14} />
                  Temporary
                </button>
              )}
            </div>
          )}
        </div>
      )}
    </section>
  );
}

function ForwardRow({
  spec,
  status,
  connected,
  busy,
  onStart,
  onStop,
  onEdit,
  onRemove,
}: {
  spec: ForwardSpec;
  status?: ForwardStatus;
  connected: boolean;
  busy: boolean;
  onStart?: () => void;
  onStop: () => void;
  onEdit?: () => void;
  onRemove?: () => void;
}) {
  const running = status?.state === "listening" || status?.state === "starting";
  const state = status?.state ?? (connected ? (spec.enabled ? "stopped" : "off") : "saved");
  return (
    <div className={`port-forward-row ${status?.state ?? ""}`}>
      <span className="port-forward-main">
        <strong>
          <i className={`port-forward-state ${state}`}>{state}</i>
          {spec.kind === "dynamic" ? "SOCKS5" : spec.kind === "local" ? "-L" : "-R"}{" "}
          {status?.boundPort ?? spec.bindPort}
          {status?.temporary && <em>temporary</em>}
        </strong>
        <small title={describeForward(spec)}>{describeForward(spec)}</small>
        {status && running && (
          <small>
            {status.activeConnections} open · {formatFileSize(status.bytesTransferred)}
          </small>
        )}
        {status?.exposed && (
          <small className="port-forward-warning">
            <AlertTriangle size={11} /> Reachable from other machines
          </small>
        )}
        {status?.message && (
          <small className="port-forward-warning" title={status.message}>
            {status.message}
          </small>
        )}
      </span>
      <span className="port-forward-actions">
        {connected &&
          (running ? (
            <button
              className="icon-button ghost"
              type="button"
              aria-label={`Stop forward ${spec.bindPort}`}
              title="Stop"
              disabled={busy}
              onClick={onStop}
            >
              <Square size={13} />
            </button>
          ) : (
            onStart && (
              <button
                className="icon-button ghost"
                type="button"
                aria-label={`Start forward ${spec.bindPort}`}
                title="Start"
                disabled={busy}
                onClick={onStart}
              >
                <Play size={13} />
              </button>
            )
          ))}
        {onEdit && (
          <button
            className="icon-button ghost"
            type="button"
            aria-label={`Edit forward ${spec.bindPort}`}
            title="Edit"
            disabled={busy}
            onClick={onEdit}
          >
            <Pencil size={13} />
          </button>
        )}
        {onRemove && (
          <button
            className="icon-button ghost"
            type="button"
            aria-label={`Delete forward ${spec.bindPort}`}
            title="Delete rule"
            disabled={busy}
            onClick={onRemove}
          >
            <X size={13} />
          </button>
        )}
      </span>
    </div>
  );
}
