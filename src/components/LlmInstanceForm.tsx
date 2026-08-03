import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState, type FormEvent } from "react";
import type {
  LlmInstance,
  LlmRuntimeStatus,
  LlmRuntimeType,
} from "../types/llm";
import type { SessionProfile } from "../types/session";
import { withHostKeyPrompt } from "../utils/hostKeyPrompt";

const DEFAULT_TIMEOUT_MS = 3000;
const DEFAULT_POLL_SECS = 5;

const RUNTIME_HINTS: Record<LlmRuntimeType, string> = {
  ollama: "http://192.168.0.20:11434",
  vllm: "http://192.168.0.21:8000",
};

type InstanceForm = {
  id: string;
  name: string;
  runtimeType: LlmRuntimeType;
  baseUrl: string;
  /** Empty means poll directly from this machine. */
  sshProfileId: string;
  requestTimeoutMs: string;
  pollIntervalSecs: string;
  apiKey: string;
  /** True once the key field is touched, so an untouched edit keeps the stored
   * key instead of clearing it. */
  apiKeyDirty: boolean;
};

const blankForm: InstanceForm = {
  id: "",
  name: "",
  runtimeType: "ollama",
  baseUrl: "",
  sshProfileId: "",
  requestTimeoutMs: String(DEFAULT_TIMEOUT_MS),
  pollIntervalSecs: String(DEFAULT_POLL_SECS),
  apiKey: "",
  apiKeyDirty: false,
};

function tunnelHostLabel(profiles: SessionProfile[], profileId: string) {
  const profile = profiles.find((item) => item.id === profileId);
  return profile ? profile.host : "the selected host";
}

/**
 * A loopback address polled directly means the runtime is on *this* machine.
 * That is legitimate, but it is also the easy mistake to make after being told
 * to enter 127.0.0.1 for a tunnel, and the only symptom is a bare "connection
 * refused" that says nothing about the cause. Warned, not blocked.
 */
export function isLoopbackAddress(baseUrl: string) {
  const authority = baseUrl
    .replace(/^https?:\/\//i, "")
    .split(/[/?#]/)[0]
    .toLowerCase();
  const host = authority.startsWith("[")
    ? authority.slice(1, authority.indexOf("]"))
    : authority.split(":")[0];
  return host === "localhost" || host === "::1" || /^127\./.test(host);
}

/** Backend failures arrive as strings; local validation throws an `Error`. */
function describeError(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function toForm(instance: LlmInstance): InstanceForm {
  return {
    id: instance.id,
    name: instance.name,
    runtimeType: instance.runtimeType,
    baseUrl: instance.baseUrl,
    sshProfileId: instance.sshProfileId ?? "",
    requestTimeoutMs: String(instance.requestTimeoutMs),
    pollIntervalSecs: String(instance.pollIntervalSecs),
    apiKey: "",
    apiKeyDirty: false,
  };
}

export function LlmInstanceForm({
  editing,
  hasApiKey,
  onSaved,
  onCancel,
}: {
  /** The instance being edited, or null to add a new one. */
  editing: LlmInstance | null;
  hasApiKey: boolean;
  onSaved: (instances: LlmInstance[]) => void;
  onCancel: () => void;
}) {
  const [form, setForm] = useState<InstanceForm>(
    editing ? toForm(editing) : blankForm,
  );
  const [busy, setBusy] = useState(false);
  const [profiles, setProfiles] = useState<SessionProfile[]>([]);
  const [message, setMessage] = useState<{
    kind: "error" | "success";
    text: string;
  } | null>(null);

  useEffect(() => {
    setForm(editing ? toForm(editing) : blankForm);
    setMessage(null);
  }, [editing]);

  useEffect(() => {
    invoke<SessionProfile[]>("load_sessions")
      // A local terminal has no SSH transport to tunnel through.
      .then((saved) => setProfiles(saved.filter((profile) => !profile.isLocal)))
      .catch(() => undefined);
  }, []);

  const updateForm = (patch: Partial<InstanceForm>) => {
    setForm((current) => ({ ...current, ...patch }));
  };

  const validate = () => {
    const url = form.baseUrl.trim();
    if (!url) {
      throw new Error("Address is required");
    }
    if (!/^https?:\/\//i.test(url)) {
      throw new Error("Address must start with http:// or https://");
    }
    // Mirrors the backend so the user learns before a round trip: the request
    // would reach 127.0.0.1 and the certificate is checked against that.
    if (form.sshProfileId && /^https:/i.test(url)) {
      throw new Error(
        "An https address cannot be reached through an SSH tunnel: the certificate would be checked against 127.0.0.1. Use http, or poll it directly.",
      );
    }
  };

  const toInstance = (): LlmInstance => ({
    id: form.id || crypto.randomUUID(),
    name: form.name.trim() || form.baseUrl.trim(),
    runtimeType: form.runtimeType,
    baseUrl: form.baseUrl.trim(),
    sshProfileId: form.sshProfileId || null,
    enabled: editing?.enabled ?? true,
    requestTimeoutMs: Number(form.requestTimeoutMs) || DEFAULT_TIMEOUT_MS,
    pollIntervalSecs: Number(form.pollIntervalSecs) || DEFAULT_POLL_SECS,
    createdAt: editing?.createdAt ?? 0,
    updatedAt: 0,
  });

  // `undefined` leaves a stored key alone; an empty string clears it.
  const apiKeyArgument = () => (form.apiKeyDirty ? form.apiKey : undefined);

  const save = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setMessage(null);
    try {
      validate();
      const instances = await invoke<LlmInstance[]>("save_llm_instance", {
        instance: toInstance(),
        apiKey: apiKeyArgument(),
      });
      // The secret is dropped from component state as soon as it is stored.
      setForm((current) => ({ ...current, apiKey: "", apiKeyDirty: false }));
      onSaved(instances);
    } catch (error) {
      setMessage({ kind: "error", text: describeError(error) });
    } finally {
      setBusy(false);
    }
  };

  const test = async () => {
    setBusy(true);
    setMessage(null);
    try {
      validate();
      // For a tunneled instance this is also where an untrusted SSH host key is
      // resolved: the background poller cannot show a fingerprint prompt, so
      // this is the only path that can bootstrap trust.
      const status = await withHostKeyPrompt(() =>
        invoke<LlmRuntimeStatus>("test_llm_instance", {
          instance: toInstance(),
          apiKey: apiKeyArgument(),
        }),
      );
      setMessage(
        status.status === "online"
          ? {
              kind: "success",
              text: `Reachable in ${status.responseTimeMs ?? "?"} ms`,
            }
          : {
              kind: "error",
              text: `${status.status}: ${status.errorMessage ?? "No detail reported"}`,
            },
      );
    } catch (error) {
      setMessage({ kind: "error", text: describeError(error) });
    } finally {
      setBusy(false);
    }
  };

  return (
    <form className="llm-instance-form" onSubmit={(event) => void save(event)}>
      <h4>{editing ? "Edit instance" : "Add instance"}</h4>

      <label>
        <span>Runtime</span>
        <select
          value={form.runtimeType}
          disabled={busy}
          onChange={(event) =>
            updateForm({ runtimeType: event.target.value as LlmRuntimeType })
          }
        >
          <option value="ollama">Ollama</option>
          <option value="vllm">vLLM</option>
        </select>
      </label>

      <label>
        <span>Name</span>
        <input
          value={form.name}
          disabled={busy}
          placeholder="GPU server Ollama"
          onChange={(event) => updateForm({ name: event.target.value })}
        />
      </label>

      <label>
        <span>Reach through</span>
        <select
          value={form.sshProfileId}
          disabled={busy}
          onChange={(event) => updateForm({ sshProfileId: event.target.value })}
        >
          <option value="">Direct (this machine)</option>
          {profiles.map((profile) => (
            <option value={profile.id} key={profile.id}>
              {profile.name} ({profile.username}@{profile.host})
            </option>
          ))}
        </select>
      </label>

      <label>
        <span>Address</span>
        <input
          value={form.baseUrl}
          disabled={busy}
          placeholder={
            form.sshProfileId
              ? "http://127.0.0.1:11434"
              : RUNTIME_HINTS[form.runtimeType]
          }
          onChange={(event) => updateForm({ baseUrl: event.target.value })}
        />
      </label>

      {!form.sshProfileId && isLoopbackAddress(form.baseUrl) && (
        <small className="llm-form-warning">
          This address is <strong>this machine&apos;s</strong> loopback, because
          Reach through is set to Direct. If the runtime is on another host, pick
          that host&apos;s SSH profile above — otherwise the poll will keep
          failing with &ldquo;connection refused&rdquo;.
        </small>
      )}

      {form.sshProfileId && (
        <small className="llm-form-hint">
          The address is resolved on {tunnelHostLabel(profiles, form.sshProfileId)}{" "}
          over an SSH tunnel. Use 127.0.0.1 to reach a runtime bound to that
          machine&apos;s own loopback — nothing needs to be exposed on its
          network.
        </small>
      )}

      <label>
        <span>API key</span>
        <input
          type="password"
          value={form.apiKey}
          disabled={busy}
          autoComplete="off"
          placeholder={
            hasApiKey ? "Stored — leave blank to keep" : "Optional"
          }
          onChange={(event) =>
            updateForm({ apiKey: event.target.value, apiKeyDirty: true })
          }
        />
      </label>

      <div className="llm-form-row">
        <label>
          <span>Poll (s)</span>
          <input
            type="number"
            min={1}
            max={300}
            value={form.pollIntervalSecs}
            disabled={busy}
            onChange={(event) =>
              updateForm({ pollIntervalSecs: event.target.value })
            }
          />
        </label>
        <label>
          <span>Timeout (ms)</span>
          <input
            type="number"
            min={500}
            max={60000}
            step={500}
            value={form.requestTimeoutMs}
            disabled={busy}
            onChange={(event) =>
              updateForm({ requestTimeoutMs: event.target.value })
            }
          />
        </label>
      </div>

      {hasApiKey && !form.apiKeyDirty && (
        <div className="llm-form-hint-row">
          <small className="llm-form-hint">
            An API key is stored for this instance. It is never shown here; type
            a new one to replace it.
          </small>
          <button
            type="button"
            disabled={busy}
            onClick={() => updateForm({ apiKey: "", apiKeyDirty: true })}
          >
            Remove stored key
          </button>
        </div>
      )}
      {hasApiKey && form.apiKeyDirty && form.apiKey === "" && (
        <small className="llm-form-hint">
          The stored key will be removed when you save.
        </small>
      )}

      <div className="llm-form-actions">
        <button type="submit" className="primary" disabled={busy}>
          {editing ? "Save" : "Add"}
        </button>
        <button type="button" disabled={busy} onClick={() => void test()}>
          Test connection
        </button>
        <button type="button" disabled={busy} onClick={onCancel}>
          Cancel
        </button>
      </div>

      {message && (
        <p className={`llm-form-message ${message.kind}`} role="status">
          {message.text}
        </p>
      )}
    </form>
  );
}
