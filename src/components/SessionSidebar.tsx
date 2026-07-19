import { FormEvent, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { confirm } from "@tauri-apps/plugin-dialog";
import {
  KeyRound,
  PlugZap,
  Plus,
  Save,
  Server,
  Trash2,
  Unplug,
} from "lucide-react";
import { selectIsActiveConnected, useSessionStore } from "../stores/sessionStore";
import { useDisconnectSession } from "../hooks/useDisconnectSession";
import gtLogo from "../assets/gt-logo.png";
import type {
  SessionConnectRequest,
  SessionProfile,
  TerminalSessionInfo,
} from "../types/session";

type SessionForm = {
  id: string;
  name: string;
  host: string;
  port: string;
  username: string;
  password: string;
  privateKeyPath: string;
  proxyJumpId: string;
  proxyJumpPassword: string;
};

const blankForm: SessionForm = {
  id: "",
  name: "",
  host: "",
  port: "22",
  username: "",
  password: "",
  privateKeyPath: "",
  proxyJumpId: "",
  proxyJumpPassword: "",
};

const UNKNOWN_HOST_KEY_PREFIX = "UNKNOWN_HOST_KEY:";
// A jump-host connection can raise one unknown-key prompt per hop
// (bastion + target), so allow a few prompt-and-retry rounds.
const MAX_HOST_KEY_PROMPTS = 3;

/**
 * Runs an SSH action; when the backend reports an unknown host key, shows the
 * fingerprint to the user and retries after they choose to trust it.
 * Sentinel format: `UNKNOWN_HOST_KEY:{fingerprint}|{keyType}|{host}:{port}`.
 */
async function withHostKeyPrompt<T>(action: () => Promise<T>): Promise<T> {
  for (let attempt = 0; ; attempt += 1) {
    try {
      return await action();
    } catch (error) {
      const text = String(error);
      const prefixIndex = text.indexOf(UNKNOWN_HOST_KEY_PREFIX);
      if (prefixIndex < 0 || attempt >= MAX_HOST_KEY_PROMPTS) {
        throw error;
      }
      const payload = text.slice(prefixIndex + UNKNOWN_HOST_KEY_PREFIX.length).trim();
      const firstSeparator = payload.indexOf("|");
      const secondSeparator = payload.indexOf("|", firstSeparator + 1);
      if (firstSeparator <= 0 || secondSeparator <= firstSeparator) {
        throw error;
      }
      const fingerprint = payload.slice(0, firstSeparator);
      const keyType = payload.slice(firstSeparator + 1, secondSeparator);
      const hostKey = payload.slice(secondSeparator + 1);
      const portIndex = hostKey.lastIndexOf(":");
      if (portIndex <= 0) {
        throw error;
      }
      const host = hostKey.slice(0, portIndex);
      const port = Number(hostKey.slice(portIndex + 1));
      const trusted = await confirm(
        `First connection to ${hostKey}.\n\nSHA256 (${keyType}) host key fingerprint:\n${fingerprint}\n\nTrust this host?`,
        { title: "Unknown host key", kind: "warning" },
      );
      if (!trusted) {
        throw new Error(`Connection canceled: ${hostKey} was not trusted`);
      }
      await invoke("trust_host_key", { host, port, keyType, fingerprint });
    }
  }
}

export function SessionSidebar() {
  const sessions = useSessionStore((state) => state.sessions);
  const activeSessionId = useSessionStore((state) => state.activeSessionId);
  const connectedSessionIds = useSessionStore((state) => state.connectedSessionIds);
  const isActiveConnected = useSessionStore(selectIsActiveConnected);
  const setSessions = useSessionStore((state) => state.setSessions);
  const setActiveSession = useSessionStore((state) => state.setActiveSession);
  const addConnectedSession = useSessionStore((state) => state.addConnectedSession);
  const setMessage = useSessionStore((state) => state.setMessage);
  const disconnectSession = useDisconnectSession();
  const [form, setForm] = useState<SessionForm>(blankForm);
  const [busy, setBusy] = useState(false);

  const activeProfile = useMemo(
    () => sessions.find((session) => session.id === activeSessionId) ?? null,
    [activeSessionId, sessions],
  );

  const updateForm = (patch: Partial<SessionForm>) => {
    setForm((current) => ({ ...current, ...patch }));
  };

  const loadSessions = async () => {
    const nextSessions = await invoke<SessionProfile[]>("load_sessions");
    setSessions(nextSessions);
  };

  const toRequest = (): SessionConnectRequest => ({
    id: form.id || null,
    name: form.name.trim() || `${form.username}@${form.host}`,
    host: form.host.trim(),
    port: Number(form.port) || 22,
    username: form.username.trim(),
    password: form.password || null,
    privateKeyPath: form.privateKeyPath || null,
    proxyJumpId: form.proxyJumpId || null,
    proxyJumpPassword: form.proxyJumpPassword || null,
    cols: 120,
    rows: 32,
  });

  const toProfile = (): SessionProfile => ({
    id: form.id || crypto.randomUUID(),
    name: form.name.trim() || `${form.username}@${form.host}`,
    host: form.host.trim(),
    port: Number(form.port) || 22,
    username: form.username.trim(),
    privateKeyPath: form.privateKeyPath || null,
    proxyJumpId: form.proxyJumpId || null,
  });

  const validate = () => {
    if (!form.host.trim()) {
      throw new Error("Host is required");
    }
    if (!form.username.trim()) {
      throw new Error("Username is required");
    }
  };

  const connect = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    try {
      validate();
      const info = await withHostKeyPrompt(() =>
        invoke<TerminalSessionInfo>("connect_terminal", {
          request: toRequest(),
        }),
      );
      setActiveSession(info.sessionId);
      addConnectedSession(info.sessionId);
      updateForm({
        id: info.profile.id,
        name: info.profile.name,
        host: info.profile.host,
        port: String(info.profile.port),
        username: info.profile.username,
        privateKeyPath: info.profile.privateKeyPath ?? "",
        proxyJumpId: info.profile.proxyJumpId ?? "",
        password: "",
        proxyJumpPassword: "",
      });
      await loadSessions();
      setMessage({
        kind: "success",
        text: `Connected to ${info.profile.username}@${info.profile.host}`,
      });
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
    } finally {
      setBusy(false);
    }
  };

  const testConnection = async () => {
    setBusy(true);
    try {
      validate();
      const result = await withHostKeyPrompt(() =>
        invoke<string>("test_ssh_connection", {
          request: toRequest(),
        }),
      );
      setMessage({ kind: "success", text: result });
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
    } finally {
      setBusy(false);
    }
  };

  const save = async () => {
    setBusy(true);
    try {
      validate();
      const profile = toProfile();
      const nextSessions = await invoke<SessionProfile[]>("save_session", {
        profile,
      });
      updateForm({
        id: profile.id,
        name: profile.name,
        host: profile.host,
        port: String(profile.port),
        username: profile.username,
        privateKeyPath: profile.privateKeyPath ?? "",
        proxyJumpId: profile.proxyJumpId ?? "",
      });
      setSessions(nextSessions);
      setMessage({ kind: "success", text: "Session saved" });
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    if (!form.id) {
      setForm(blankForm);
      return;
    }
    setBusy(true);
    try {
      if (connectedSessionIds.includes(form.id)) {
        await disconnectSession(form.id);
      }
      const nextSessions = await invoke<SessionProfile[]>("delete_session", {
        id: form.id,
      });
      setSessions(nextSessions);
      setForm(blankForm);
      setMessage({ kind: "success", text: "Session deleted" });
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
    } finally {
      setBusy(false);
    }
  };

  const disconnect = async () => {
    if (!activeSessionId) {
      return;
    }
    setBusy(true);
    try {
      await disconnectSession();
    } finally {
      setBusy(false);
    }
  };

  const selectSession = (session: SessionProfile) => {
    setForm({
      id: session.id,
      name: session.name,
      host: session.host,
      port: String(session.port),
      username: session.username,
      password: "",
      privateKeyPath: session.privateKeyPath ?? "",
      proxyJumpId: session.proxyJumpId ?? "",
      proxyJumpPassword: "",
    });
    // Clicking a live session switches the terminal/SFTP/telemetry view to it.
    if (connectedSessionIds.includes(session.id)) {
      setActiveSession(session.id);
    }
  };

  return (
    <aside className="session-sidebar">
      <div className="brand-row">
        <img className="brand-mark" src={gtLogo} alt="GpuTerm logo" />
        <div>
          <h1>GpuTerm</h1>
          <p>{isActiveConnected && activeProfile ? activeProfile.host : "SSH/SFTP"}</p>
        </div>
      </div>

      <form className="session-form" onSubmit={connect}>
        <small className="form-binding-hint">
          {form.id
            ? `Editing saved profile: ${form.name || form.host || form.id}`
            : "New profile"}
        </small>
        <label>
          <span>Name</span>
          <input
            value={form.name}
            onChange={(event) => updateForm({ name: event.target.value })}
            placeholder="lab-a100"
          />
        </label>
        <label>
          <span>Host</span>
          <input
            value={form.host}
            onChange={(event) => updateForm({ host: event.target.value })}
            placeholder="10.0.0.21"
            required
          />
        </label>
        <div className="field-row">
          <label>
            <span>Port</span>
            <input
              value={form.port}
              inputMode="numeric"
              onChange={(event) => updateForm({ port: event.target.value })}
              required
            />
          </label>
          <label>
            <span>User</span>
            <input
              value={form.username}
              onChange={(event) => updateForm({ username: event.target.value })}
              placeholder="ubuntu"
              required
            />
          </label>
        </div>
        <label>
          <span>Password / passphrase</span>
          <input
            value={form.password}
            type="password"
            autoComplete="current-password"
            onChange={(event) => updateForm({ password: event.target.value })}
          />
        </label>
        <label>
          <span>Private key path</span>
          <input
            value={form.privateKeyPath}
            onChange={(event) =>
              updateForm({ privateKeyPath: event.target.value })
            }
            placeholder="C:\\Users\\you\\.ssh\\id_ed25519"
          />
        </label>
        <label>
          <span>Jump host</span>
          <select
            value={form.proxyJumpId}
            onChange={(event) => updateForm({ proxyJumpId: event.target.value })}
          >
            <option value="">None (direct)</option>
            {sessions
              .filter((session) => session.id !== form.id)
              .map((session) => (
                <option value={session.id} key={session.id}>
                  {session.name} ({session.username}@{session.host})
                </option>
              ))}
          </select>
        </label>
        {form.proxyJumpId && (
          <label>
            <span>Jump host password</span>
            <input
              value={form.proxyJumpPassword}
              type="password"
              autoComplete="off"
              placeholder="Leave blank for key/agent auth"
              onChange={(event) =>
                updateForm({ proxyJumpPassword: event.target.value })
              }
            />
          </label>
        )}
        <div className="button-row">
          <button className="primary-button" disabled={busy} type="submit">
            <PlugZap size={16} />
            Connect
          </button>
          <button
            className="secondary-button"
            disabled={busy}
            type="button"
            onClick={testConnection}
          >
            <Server size={16} />
            Test
          </button>
        </div>
        <div className="button-row">
          <button
            className="secondary-button"
            disabled={busy}
            type="button"
            onClick={() => setForm(blankForm)}
          >
            <Plus size={16} />
            New
          </button>
          <button
            className="secondary-button"
            disabled={busy}
            type="button"
            onClick={save}
          >
            <Save size={16} />
            Save
          </button>
          <button
            className="secondary-button danger"
            disabled={busy}
            type="button"
            onClick={remove}
          >
            <Trash2 size={16} />
            Delete
          </button>
        </div>
      </form>

      <div className="sidebar-section-title">Sessions</div>
      <div className="session-list">
        {sessions.map((session) => (
          <button
            key={session.id}
            className={`session-item ${
              session.id === form.id ? "selected" : ""
            } ${session.id === activeSessionId ? "active" : ""} ${
              connectedSessionIds.includes(session.id) ? "connected" : ""
            }`}
            type="button"
            onClick={() => selectSession(session)}
          >
            {connectedSessionIds.includes(session.id) && (
              <span className="status-dot" aria-hidden="true" />
            )}
            <Server size={16} />
            <span>
              <strong>{session.name}</strong>
              <small>
                {session.username}@{session.host}:{session.port}
                {session.proxyJumpId && (
                  <>
                    {" "}
                    · via{" "}
                    {sessions.find((item) => item.id === session.proxyJumpId)?.name ??
                      "missing jump host"}
                  </>
                )}
              </small>
            </span>
            {session.privateKeyPath && <KeyRound size={14} />}
          </button>
        ))}
      </div>

      <button
        className="disconnect-button"
        type="button"
        disabled={!isActiveConnected || busy}
        onClick={disconnect}
      >
        <Unplug size={16} />
        Disconnect
      </button>
    </aside>
  );
}
