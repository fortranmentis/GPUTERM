import { FormEvent, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { confirm } from "@tauri-apps/plugin-dialog";
import { KeyRound, LoaderCircle, ShieldCheck, Trash2 } from "lucide-react";
import type { CredentialVaultStatus } from "../types/credentials";

const MIN_MASTER_PASSWORD_CHARS = 8;

export function CredentialVaultGate() {
  const [status, setStatus] = useState<CredentialVaultStatus | null>(null);
  const [password, setPassword] = useState("");
  const [confirmation, setConfirmation] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const loadStatus = async () => {
    setError(null);
    try {
      setStatus(
        await invoke<CredentialVaultStatus>("get_credential_vault_status"),
      );
    } catch (statusError) {
      setError(String(statusError));
    }
  };

  useEffect(() => {
    void loadStatus();
  }, []);

  if (status?.unlocked) {
    return null;
  }

  const creating = status?.exists === false;

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setError(null);
    if (creating) {
      if ([...password].length < MIN_MASTER_PASSWORD_CHARS) {
        setError(
          `Master password must be at least ${MIN_MASTER_PASSWORD_CHARS} characters`,
        );
        return;
      }
      if (password !== confirmation) {
        setError("Master password confirmation does not match");
        return;
      }
    }
    if (!password) {
      setError("Enter the GpuTerm master password");
      return;
    }

    const masterPassword = password;
    setPassword("");
    setConfirmation("");
    setBusy(true);
    try {
      const nextStatus = await invoke<CredentialVaultStatus>(
        creating ? "initialize_credential_vault" : "unlock_credential_vault",
        { masterPassword },
      );
      setStatus(nextStatus);
    } catch (submitError) {
      setError(String(submitError));
    } finally {
      setBusy(false);
    }
  };

  const resetVault = async () => {
    const accepted = await confirm(
      "Resetting the vault permanently deletes every SSH password saved by GpuTerm. Session profiles are kept. Continue?",
      { title: "Reset credential vault", kind: "warning" },
    );
    if (!accepted) return;

    setBusy(true);
    setError(null);
    try {
      setStatus(
        await invoke<CredentialVaultStatus>("reset_credential_vault"),
      );
      setPassword("");
      setConfirmation("");
    } catch (resetError) {
      setError(String(resetError));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="credential-vault-backdrop">
      <form
        className="credential-vault-dialog"
        role="dialog"
        aria-modal="true"
        aria-label={creating ? "Create credential vault" : "Unlock credential vault"}
        onSubmit={submit}
      >
        <div className="credential-vault-icon" aria-hidden="true">
          {creating ? <ShieldCheck size={24} /> : <KeyRound size={24} />}
        </div>
        <div className="credential-vault-heading">
          <h2>{creating ? "Create local credential vault" : "Unlock credential vault"}</h2>
          <p>
            {creating
              ? "Choose a GpuTerm master password. Existing macOS Keychain passwords are not imported; enter each SSH password again when connecting."
              : "Enter your GpuTerm master password to decrypt saved SSH credentials for this app session."}
          </p>
        </div>

        {status == null && !error ? (
          <div className="credential-vault-loading" role="status">
            <LoaderCircle className="spin" size={18} />
            Checking local vault…
          </div>
        ) : (
          <>
            <label>
              <span>Master password</span>
              <input
                autoFocus
                type="password"
                autoComplete={creating ? "new-password" : "current-password"}
                value={password}
                disabled={busy}
                onChange={(event) => setPassword(event.target.value)}
              />
            </label>
            {creating && (
              <label>
                <span>Confirm master password</span>
                <input
                  type="password"
                  autoComplete="new-password"
                  value={confirmation}
                  disabled={busy}
                  onChange={(event) => setConfirmation(event.target.value)}
                />
              </label>
            )}
            {creating && (
              <small className="credential-vault-warning">
                The master password cannot be recovered. Losing it requires resetting
                the vault and re-entering every SSH password.
              </small>
            )}
          </>
        )}

        {error && <div className="credential-vault-error" role="alert">{error}</div>}

        <div className="credential-vault-actions">
          {status == null ? (
            error ? (
              <button className="secondary-button" type="button" onClick={loadStatus}>
                Retry
              </button>
            ) : null
          ) : (
            <>
              {!creating && (
                <button
                  className="secondary-button danger"
                  type="button"
                  disabled={busy}
                  onClick={() => void resetVault()}
                >
                  <Trash2 size={15} />
                  Reset vault
                </button>
              )}
              <button className="primary-button" type="submit" disabled={busy}>
                {busy ? <LoaderCircle className="spin" size={15} /> : <KeyRound size={15} />}
                {creating ? "Create vault" : "Unlock"}
              </button>
            </>
          )}
        </div>
      </form>
    </div>
  );
}
