import { FormEvent, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  KeyRound,
  PlugZap,
  Plus,
  Save,
  Server,
  PanelLeftClose,
  Trash2,
  Unplug,
} from "lucide-react";
import { selectIsActiveConnected, useSessionStore } from "../stores/sessionStore";
import { useDisconnectSession } from "../hooks/useDisconnectSession";
import { withHostKeyPrompt } from "../utils/hostKeyPrompt";
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

export function SessionSidebar({ onClose }: { onClose?: () => void }) {
  const sessions = useSessionStore((state) => state.sessions);
  const activeSessionId = useSessionStore((state) => state.activeSessionId);
  const connectedSessionIds = useSessionStore((state) => state.connectedSessionIds);
  const isActiveConnected = useSessionStore(selectIsActiveConnected);
  const setSessions = useSessionStore((state) => state.setSessions);
  const showSession = useSessionStore((state) => state.showSession);
  const addConnectedSession = useSessionStore((state) => state.addConnectedSession);
  const setMessage = useSessionStore((state) => state.setMessage);
  const disconnectSession = useDisconnectSession();
  const [form, setForm] = useState<SessionForm>(blankForm);
  const [showNewForm, setShowNewForm] = useState(false);
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

  const connect = async (event?: FormEvent) => {
    event?.preventDefault();
    setBusy(true);
    try {
      validate();
      const info = await withHostKeyPrompt(() =>
        invoke<TerminalSessionInfo>("connect_terminal", {
          request: toRequest(),
        }),
      );
      addConnectedSession(info.sessionId, info.terminalId ?? info.sessionId);
      showSession(info.sessionId);
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
      setShowNewForm(false);
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
      setShowNewForm(false);
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
      setShowNewForm(false);
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
    setShowNewForm(false);
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
      showSession(session.id);
    }
  };

  const startNewProfile = () => {
    setForm(blankForm);
    setShowNewForm(true);
  };

  return (
    <aside className="session-sidebar">
      <div className="brand-row">
        <img className="brand-mark" src={gtLogo} alt="GpuTerm logo" />
        <div>
          <h1>GpuTerm</h1>
          <p>{isActiveConnected && activeProfile ? activeProfile.host : "SSH/SFTP"}</p>
        </div>
        {onClose && (
          <button
            className="icon-button ghost sidebar-close-button"
            type="button"
            aria-label="Close host selector"
            title="Close host selector"
            onClick={onClose}
          >
            <PanelLeftClose size={17} />
          </button>
        )}
      </div>

      <div className="sidebar-section-heading">
        <div className="sidebar-section-title">Sessions</div>
        <button
          className="secondary-button compact"
          disabled={busy}
          type="button"
          onClick={startNewProfile}
        >
          <Plus size={15} />
          New
        </button>
      </div>

      {showNewForm && (
        <form className="session-form new-session-form" onSubmit={connect}>
          <small className="form-binding-hint">New profile</small>
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
              {sessions.map((session) => (
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
              onClick={save}
            >
              <Save size={16} />
              Save
            </button>
            <button
              className="secondary-button"
              disabled={busy}
              type="button"
              onClick={() => {
                setShowNewForm(false);
                setForm(blankForm);
              }}
            >
              Cancel
            </button>
          </div>
        </form>
      )}

      {!showNewForm && (
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
      )}

      {!showNewForm && form.id && (
        <form className="selected-session-actions" onSubmit={connect}>
          <span title={form.name}>{form.name}</span>
          {!connectedSessionIds.includes(form.id) && (
            <>
              <label>
                <span>Password / key passphrase</span>
                <input
                  value={form.password}
                  type="password"
                  autoComplete="current-password"
                  placeholder="Leave blank for key/agent auth"
                  onChange={(event) =>
                    updateForm({ password: event.target.value })
                  }
                />
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
            </>
          )}
          <div className="button-row">
            <button
              className="primary-button"
              disabled={busy || connectedSessionIds.includes(form.id)}
              type="submit"
            >
              <PlugZap size={16} />
              Connect
            </button>
            <button
              className="secondary-button"
              disabled={busy || connectedSessionIds.includes(form.id)}
              type="button"
              onClick={testConnection}
            >
              <Server size={16} />
              Test
            </button>
            <button
              className="secondary-button danger"
              disabled={busy}
              type="button"
              aria-label={`Delete ${form.name}`}
              onClick={remove}
            >
              <Trash2 size={16} />
            </button>
          </div>
        </form>
      )}

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
