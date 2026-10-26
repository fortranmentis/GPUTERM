import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { FileKey, LoaderCircle, Upload } from "lucide-react";
import { useKeyStore } from "../stores/keyStore";
import { useSessionStore } from "../stores/sessionStore";
import { withHostKeyPrompt } from "../utils/hostKeyPrompt";
import { kindTitle } from "./KeyManagerDialog";
import type { StoredKey } from "../types/keys";
import type { SessionProfile } from "../types/session";

const MANAGE_KEYS = "__manage__";

/**
 * In a saved profile: which key manager key it signs in with, and putting
 * that key on the server.
 */
export function ProfileKeyRow({
  profile,
  onManageKeys,
}: {
  profile: SessionProfile;
  onManageKeys: () => void;
}) {
  const keys = useKeyStore((state) => state.keys);
  const loaded = useKeyStore((state) => state.loaded);
  const refresh = useKeyStore((state) => state.refresh);
  const setKeys = useKeyStore((state) => state.setKeys);
  const setMessage = useSessionStore((state) => state.setMessage);
  const [busy, setBusy] = useState<null | "assign" | "install">(null);
  const assigned = keys.find((key) => key.profileIds.includes(profile.id)) ?? null;

  useEffect(() => {
    if (!loaded) {
      refresh().catch((error) => setMessage({ kind: "error", text: String(error) }));
    }
  }, [loaded, refresh, setMessage]);

  const assign = async (keyId: string | null) => {
    setBusy("assign");
    try {
      setKeys(await invoke<StoredKey[]>("assign_key", { profileId: profile.id, keyId }));
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
    } finally {
      setBusy(null);
    }
  };

  const install = async (key: StoredKey) => {
    setBusy("install");
    try {
      const updated = await withHostKeyPrompt(() =>
        invoke<StoredKey[]>("install_key", { profileId: profile.id, keyId: key.id }),
      );
      setKeys(updated);
      setMessage({
        kind: "success",
        text: `${key.name} installed on ${profile.host}; GpuTerm signs in with it from now on`,
      });
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
    } finally {
      setBusy(null);
    }
  };

  return (
    <section className="port-forwards profile-key-row">
      <label className="profile-key-select">
        <span>
          <FileKey size={14} />
          Sign in with key
        </span>
        <select
          value={assigned?.id ?? ""}
          disabled={busy != null}
          onChange={(event) => {
            const value = event.target.value;
            if (value === MANAGE_KEYS) {
              onManageKeys();
              return;
            }
            void assign(value || null);
          }}
        >
          <option value="">None</option>
          {keys.map((key) => (
            <option key={key.id} value={key.id}>
              {key.name} ({kindTitle(key.kind)})
            </option>
          ))}
          <option value={MANAGE_KEYS}>Manage keys…</option>
        </select>
      </label>
      {assigned && (
        <div className="profile-key-install">
          <button
            className="secondary-button compact"
            type="button"
            disabled={busy != null}
            onClick={() => void install(assigned)}
          >
            {busy === "install" ? <LoaderCircle className="spin" size={14} /> : <Upload size={14} />}
            {busy === "install" ? "Installing…" : "Install on server"}
          </button>
          <small>Adds the public key to authorized_keys and checks it signs in.</small>
        </div>
      )}
    </section>
  );
}
