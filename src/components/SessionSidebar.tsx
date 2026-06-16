import { FormEvent, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  KeyRound,
  PlugZap,
  Save,
  Server,
  Trash2,
  Unplug,
} from "lucide-react";
import { useSessionStore } from "../stores/sessionStore";
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
};

const blankForm: SessionForm = {
  id: "",
  name: "",
  host: "",
  port: "22",
  username: "",
  password: "",
  privateKeyPath: "",
};

export function SessionSidebar() {
  const sessions = useSessionStore((state) => state.sessions);
  const activeSessionId = useSessionStore((state) => state.activeSessionId);
  const connected = useSessionStore((state) => state.connected);
  const setSessions = useSessionStore((state) => state.setSessions);
  const setActiveSession = useSessionStore((state) => state.setActiveSession);
  const setConnected = useSessionStore((state) => state.setConnected);
  const setMessage = useSessionStore((state) => state.setMessage);
  const setRemoteTelemetry = useSessionStore((state) => state.setRemoteTelemetry);
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
      const info = await invoke<TerminalSessionInfo>("connect_terminal", {
        request: toRequest(),
      });
      setActiveSession(info.sessionId);
      setConnected(true);
      setRemoteTelemetry(null);
      updateForm({
        id: info.profile.id,
        name: info.profile.name,
        host: info.profile.host,
        port: String(info.profile.port),
        username: info.profile.username,
        privateKeyPath: info.profile.privateKeyPath ?? "",
        password: "",
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
      const result = await invoke<string>("test_ssh_connection", {
        request: toRequest(),
      });
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
      if (form.id === activeSessionId) {
        await invoke("disconnect_terminal", { sessionId: form.id });
        setConnected(false);
        setActiveSession(null);
        setRemoteTelemetry(null);
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
      await invoke("disconnect_terminal", { sessionId: activeSessionId });
      setConnected(false);
      setActiveSession(null);
      setRemoteTelemetry(null);
      setMessage({ kind: "info", text: "Disconnected" });
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
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
    });
  };

  return (
    <aside className="session-sidebar">
      <div className="brand-row">
        <div className="brand-mark">GT</div>
        <div>
          <h1>GpuTerm</h1>
          <p>{connected && activeProfile ? activeProfile.host : "SSH/SFTP"}</p>
        </div>
      </div>

      <form className="session-form" onSubmit={connect}>
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
            } ${session.id === activeSessionId ? "active" : ""}`}
            type="button"
            onClick={() => selectSession(session)}
          >
            <Server size={16} />
            <span>
              <strong>{session.name}</strong>
              <small>
                {session.username}@{session.host}:{session.port}
              </small>
            </span>
            {session.privateKeyPath && <KeyRound size={14} />}
          </button>
        ))}
      </div>

      <button
        className="disconnect-button"
        type="button"
        disabled={!connected || busy}
        onClick={disconnect}
      >
        <Unplug size={16} />
        Disconnect
      </button>
    </aside>
  );
}
