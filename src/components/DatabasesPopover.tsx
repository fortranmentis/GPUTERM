import { invoke } from "@tauri-apps/api/core";
import { confirm } from "@tauri-apps/plugin-dialog";
import { Database, LoaderCircle, Pencil, Plus, RefreshCw, Trash2 } from "lucide-react";
import { useState, type FormEvent, type RefObject } from "react";
import { ResourceDetailPopover } from "./ResourceDetailPopover";
import { useSessionStore } from "../stores/sessionStore";
import type {
  DbInstance,
  DbInstanceInput,
  DbKind,
  DbReading,
  DbStatus,
} from "../types/databases";
import type { SessionProfile } from "../types/session";
import {
  connectionsLabel,
  defaultPort,
  formatRate,
  kindTitle,
  needsAttention,
  rateLabel,
} from "../utils/dbFormat";
import { formatBytes } from "../utils/formatBytes";

type DatabasesPopoverProps = {
  instances: DbInstance[];
  readings: Record<string, DbReading>;
  profiles: SessionProfile[];
  connectedProfileIds: string[];
  onInstancesChange: (instances: DbInstance[], changedId?: string) => void;
  onRefresh: () => Promise<void> | void;
  anchorRef: RefObject<HTMLElement | null>;
  onClose: () => void;
};

/** Which form is open: a new database, or the one with this id. */
type Editing = { instance: DbInstance | null; key: number } | null;

/** Every saved database with its latest reading, and adding or editing one. */
export function DatabasesPopover({
  instances,
  readings,
  profiles,
  connectedProfileIds,
  onInstancesChange,
  onRefresh,
  anchorRef,
  onClose,
}: DatabasesPopoverProps) {
  const setMessage = useSessionStore((state) => state.setMessage);
  const [editing, setEditing] = useState<Editing>(null);
  const servers = profiles.filter((profile) => !profile.isLocal);

  const remove = async (instance: DbInstance) => {
    const accepted = await confirm(
      "GpuTerm stops reading it and forgets its saved password. The database itself is not touched.",
      { title: `Remove ${instance.name}?`, kind: "warning", okLabel: "Remove" },
    );
    if (!accepted) return;
    try {
      onInstancesChange(await invoke<DbInstance[]>("delete_db_instance", { id: instance.id }));
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
    }
  };

  return (
    <ResourceDetailPopover
      anchorRef={anchorRef}
      ariaLabel="Databases"
      title="Databases"
      icon={<Database size={16} />}
      className="databases-popover"
      onClose={onClose}
      headerActions={
        <button
          className="icon-button ghost"
          type="button"
          aria-label="Refresh databases"
          title="Refresh databases"
          onClick={() => void onRefresh()}
        >
          <RefreshCw size={14} />
        </button>
      }
    >
      <div className="databases-detail">
        <p className="containers-muted">
          PostgreSQL and MySQL/MariaDB, read with psql or mysql on a connected server. The
          queries only read statistics; the password goes to the client through a private
          file that is removed straight away.
        </p>
        {editing ? (
          <DatabaseForm
            key={editing.key}
            instance={editing.instance}
            servers={servers}
            onSaved={(next, id) => {
              onInstancesChange(next, id);
              setEditing(null);
            }}
            onCancel={() => setEditing(null)}
          />
        ) : (
          <div className="databases-add">
            <button
              className="secondary-button compact"
              type="button"
              disabled={servers.length === 0}
              onClick={() => setEditing({ instance: null, key: Date.now() })}
            >
              <Plus size={14} />
              Add database
            </button>
            {servers.length === 0 && (
              <small className="containers-muted">
                Save the server the database runs on as a session first.
              </small>
            )}
          </div>
        )}
        {instances.length === 0 && !editing && (
          <div className="empty-list compact">No database added</div>
        )}
        {instances.map((instance) => (
          <DatabaseRow
            key={instance.id}
            instance={instance}
            reading={readings[instance.id]}
            server={profiles.find((profile) => profile.id === instance.profileId)}
            connected={connectedProfileIds.includes(instance.profileId)}
            onEdit={() => setEditing({ instance, key: Date.now() })}
            onRemove={() => void remove(instance)}
          />
        ))}
      </div>
    </ResourceDetailPopover>
  );
}

function DatabaseRow({
  instance,
  reading,
  server,
  connected,
  onEdit,
  onRemove,
}: {
  instance: DbInstance;
  reading: DbReading | undefined;
  server: SessionProfile | undefined;
  connected: boolean;
  onEdit: () => void;
  onRemove: () => void;
}) {
  const serverName = server?.name ?? "missing server";
  const attention = needsAttention(reading?.status);
  return (
    <div
      className={`database-row${attention ? " attention" : ""}`}
      data-testid={`database-${instance.id}`}
    >
      <div className="database-row-head">
        <div className="database-row-title">
          <strong>{instance.name}</strong>
          <span className="containers-muted">
            {kindTitle(instance.kind)} · {instance.user}@{instance.host || "socket"}:
            {instance.port} via {serverName}
          </span>
        </div>
        <button
          className="icon-button ghost"
          type="button"
          aria-label={`Edit ${instance.name}`}
          title="Edit"
          onClick={onEdit}
        >
          <Pencil size={13} />
        </button>
        <button
          className="icon-button ghost"
          type="button"
          aria-label={`Remove ${instance.name}`}
          title="Remove"
          onClick={onRemove}
        >
          <Trash2 size={13} />
        </button>
      </div>
      {reading?.status ? (
        <StatusGrid kind={instance.kind} status={reading.status} rate={reading.rate} />
      ) : reading?.error ? (
        <p className="containers-warn">{reading.error}</p>
      ) : !connected ? (
        <p className="containers-muted">Connect to {serverName} to read it.</p>
      ) : (
        <p className="containers-muted" role="status">
          <LoaderCircle className="spin" size={13} /> Reading…
        </p>
      )}
    </div>
  );
}

function StatusGrid({
  kind,
  status,
  rate,
}: {
  kind: DbKind;
  status: DbStatus;
  rate: number | null;
}) {
  const count = (value: number | null) => (value == null ? null : String(value));
  const rows: [string, string | null][] = [
    ["Version", status.version],
    ["Connections", connectionsLabel(status)],
    ["Active", count(status.active)],
    ["Idle in transaction", count(status.idleInTransaction)],
    ["Waiting on locks", count(status.waitingOnLocks)],
    ["Longest query", status.longestQuerySecs == null ? null : `${status.longestQuerySecs}s`],
    ["Throughput", rate == null ? null : `${formatRate(rate)} ${rateLabel(kind)}`],
    ["Slow queries", count(status.slowQueries)],
    ["Cache hit", status.cacheHitPercent == null ? null : `${status.cacheHitPercent.toFixed(1)}%`],
    ["Deadlocks", count(status.deadlocks)],
    ["Size", status.sizeBytes == null ? null : formatBytes(status.sizeBytes)],
    [
      "Replication lag",
      status.isReplica
        ? status.replicationLagSecs == null
          ? "unknown"
          : `${status.replicationLagSecs}s`
        : null,
    ],
  ];
  return (
    <dl className="database-status-grid">
      {rows
        .filter((row): row is [string, string] => row[1] != null)
        .map(([label, value]) => (
          <div key={label}>
            <dt>{label}</dt>
            <dd>{value}</dd>
          </div>
        ))}
    </dl>
  );
}

function DatabaseForm({
  instance,
  servers,
  onSaved,
  onCancel,
}: {
  instance: DbInstance | null;
  servers: SessionProfile[];
  onSaved: (instances: DbInstance[], id?: string) => void;
  onCancel: () => void;
}) {
  const [name, setName] = useState(instance?.name ?? "");
  const [kind, setKind] = useState<DbKind>(instance?.kind ?? "postgres");
  const [profileId, setProfileId] = useState(instance?.profileId ?? "");
  const [host, setHost] = useState(instance?.host ?? "127.0.0.1");
  const [port, setPort] = useState(instance ? String(instance.port) : "");
  const [user, setUser] = useState(instance?.user ?? "");
  const [database, setDatabase] = useState(instance?.database ?? "");
  const [password, setPassword] = useState("");
  const [removePassword, setRemovePassword] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setError(null);
    const trimmedPort = port.trim();
    const portNumber = trimmedPort === "" ? undefined : Number(trimmedPort);
    if (
      portNumber !== undefined &&
      (!Number.isInteger(portNumber) || portNumber < 1 || portNumber > 65535)
    ) {
      setError("Enter a port between 1 and 65535");
      return;
    }
    const input: DbInstanceInput = {
      id: instance?.id,
      name,
      kind,
      profileId,
      host,
      port: portNumber,
      user,
      database: kind === "postgres" ? database : "",
      password: removePassword ? "" : password === "" ? undefined : password,
    };
    setBusy(true);
    try {
      const next = await invoke<DbInstance[]>("save_db_instance", { input });
      setPassword("");
      onSaved(next, instance?.id);
    } catch (saveError) {
      setError(String(saveError));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      className="database-form"
      aria-label={instance ? `Edit ${instance.name}` : "Add database"}
      onSubmit={submit}
    >
      <label>
        <span>Name</span>
        <input value={name} placeholder="Main Postgres" onChange={(e) => setName(e.target.value)} />
      </label>
      <label>
        <span>Type</span>
        <select value={kind} onChange={(e) => setKind(e.target.value as DbKind)}>
          <option value="postgres">PostgreSQL</option>
          <option value="mysql">MySQL / MariaDB</option>
        </select>
      </label>
      <label>
        <span>Server</span>
        <select value={profileId} onChange={(e) => setProfileId(e.target.value)}>
          <option value="">Choose a saved server</option>
          {servers.map((server) => (
            <option key={server.id} value={server.id}>
              {server.name} ({server.username}@{server.host})
            </option>
          ))}
        </select>
      </label>
      <div className="database-form-row">
        <label>
          <span>Host (from the server)</span>
          <input
            value={host}
            placeholder="Empty: local socket"
            onChange={(e) => setHost(e.target.value)}
          />
        </label>
        <label className="database-port">
          <span>Port</span>
          <input
            value={port}
            inputMode="numeric"
            placeholder={String(defaultPort(kind))}
            onChange={(e) => setPort(e.target.value)}
          />
        </label>
      </div>
      <div className="database-form-row">
        <label>
          <span>User</span>
          <input
            value={user}
            placeholder={kind === "postgres" ? "postgres" : "root"}
            onChange={(e) => setUser(e.target.value)}
          />
        </label>
        {kind === "postgres" && (
          <label>
            <span>Database</span>
            <input
              value={database}
              placeholder="Default"
              onChange={(e) => setDatabase(e.target.value)}
            />
          </label>
        )}
      </div>
      <label>
        <span>Password</span>
        <input
          type="password"
          autoComplete="new-password"
          value={password}
          disabled={removePassword}
          placeholder={instance?.hasPassword ? "Saved; type to replace" : "None"}
          onChange={(e) => setPassword(e.target.value)}
        />
      </label>
      {instance?.hasPassword && (
        <label className="database-checkbox">
          <input
            type="checkbox"
            checked={removePassword}
            onChange={(e) => setRemovePassword(e.target.checked)}
          />
          <span>Remove the saved password</span>
        </label>
      )}
      <small className="containers-muted">
        Use a read-only monitoring user where you can. The password is kept in the encrypted
        credential vault, which must be unlocked to save it or read the database.
      </small>
      {error && <div className="containers-error" role="alert">{error}</div>}
      <div className="database-form-actions">
        <button className="secondary-button compact" type="button" onClick={onCancel}>
          Cancel
        </button>
        <button
          className="primary-button compact"
          type="submit"
          disabled={busy || profileId === "" || name.trim() === ""}
        >
          {busy ? "Saving…" : "Save"}
        </button>
      </div>
    </form>
  );
}
