import { useState, type FormEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { confirm, save } from "@tauri-apps/plugin-dialog";
import {
  ChevronDown,
  ChevronRight,
  Copy,
  Download,
  KeyRound,
  LoaderCircle,
  RefreshCw,
  Trash2,
} from "lucide-react";
import { useSessionStore } from "../stores/sessionStore";
import { withHostKeyPrompt } from "../utils/hostKeyPrompt";
import type { SessionProfile } from "../types/session";

/** Mirrors `MIN_EXPORT_PASSPHRASE_CHARS` in src-tauri/src/ssh/managed_key.rs. */
export const MIN_EXPORT_PASSPHRASE_CHARS = 8;

function formatDate(epochSeconds: number) {
  return new Date(epochSeconds * 1000).toLocaleDateString();
}

/** A file name for the export, from the profile name. */
export function exportFileName(profileName: string) {
  const safe = profileName.replace(/[^A-Za-z0-9._-]+/g, "-").replace(/^-+|-+$/g, "");
  return `id_ed25519_gputerm${safe ? `_${safe}` : ""}`;
}

/** Why the export passphrases cannot be used yet, or null when they can. */
export function passphraseProblem(passphrase: string, repeat: string) {
  if ([...passphrase].length < MIN_EXPORT_PASSPHRASE_CHARS) {
    return `Use at least ${MIN_EXPORT_PASSPHRASE_CHARS} characters`;
  }
  if (passphrase !== repeat) {
    return "The passphrases do not match";
  }
  return null;
}

function ExportDialog({
  profile,
  onClose,
}: {
  profile: SessionProfile;
  onClose: (exportedTo?: string) => void;
}) {
  const [passphrase, setPassphrase] = useState("");
  const [repeat, setRepeat] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const problem = passphraseProblem(passphrase, repeat);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (problem) {
      return;
    }
    setError(null);
    const path = await save({ defaultPath: exportFileName(profile.name) });
    if (!path) {
      return;
    }
    setBusy(true);
    try {
      await invoke("export_managed_key", { profileId: profile.id, path, passphrase });
      onClose(path);
    } catch (exportError) {
      setError(String(exportError));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="app-modal-backdrop" role="presentation">
      <form
        className="app-modal managed-key-dialog"
        role="dialog"
        aria-label="Export private key"
        onSubmit={(event) => void submit(event)}
      >
        <div className="app-modal-header">
          <Download size={18} />
          <strong>Export private key</strong>
        </div>
        <p className="app-modal-text">
          The key is saved in OpenSSH format, encrypted with this passphrase, with its public
          key next to it as <code>.pub</code>. Anyone with the file and the passphrase can sign
          in as {profile.username}@{profile.host}.
        </p>
        <label>
          <span>Passphrase</span>
          <input
            type="password"
            autoComplete="new-password"
            autoFocus
            value={passphrase}
            onChange={(event) => setPassphrase(event.target.value)}
          />
        </label>
        <label>
          <span>Repeat passphrase</span>
          <input
            type="password"
            autoComplete="new-password"
            value={repeat}
            onChange={(event) => setRepeat(event.target.value)}
          />
        </label>
        {(passphrase || repeat) && problem && (
          <small className="managed-key-problem">{problem}</small>
        )}
        {error && (
          <small className="managed-key-problem" role="alert">
            {error}
          </small>
        )}
        <div className="app-modal-actions">
          <button className="secondary-button" type="button" onClick={() => onClose()}>
            Cancel
          </button>
          <button className="primary-button" type="submit" disabled={problem != null || busy}>
            {busy ? "Exporting…" : "Choose file…"}
          </button>
        </div>
      </form>
    </div>
  );
}

function RemoveDialog({
  profile,
  onClose,
}: {
  profile: SessionProfile;
  onClose: (removeFromServer?: boolean) => void;
}) {
  const [fromServer, setFromServer] = useState(true);
  return (
    <div className="app-modal-backdrop" role="presentation">
      <div className="app-modal managed-key-dialog" role="alertdialog" aria-label="Remove SSH key">
        <div className="app-modal-header">
          <Trash2 size={18} />
          <strong>Remove the GpuTerm key?</strong>
        </div>
        <p className="app-modal-text">
          GpuTerm forgets the key. Signing in to {profile.host} then uses the password, key file
          or agent again. Export the key first if you still need it elsewhere.
        </p>
        <label className="managed-key-check">
          <input
            type="checkbox"
            checked={fromServer}
            onChange={(event) => setFromServer(event.target.checked)}
          />
          <span>Also remove it from the server&apos;s authorized_keys</span>
        </label>
        <div className="app-modal-actions">
          <button className="secondary-button" type="button" onClick={() => onClose()}>
            Cancel
          </button>
          <button
            className="primary-button danger"
            type="button"
            onClick={() => onClose(fromServer)}
          >
            Remove
          </button>
        </div>
      </div>
    </div>
  );
}

/**
 * The SSH key GpuTerm creates for a profile: set it up (generate, install on
 * the server, confirm it signs in), copy its public key, export the private
 * key, replace it, or remove it.
 */
export function ManagedKeyPanel({ profile }: { profile: SessionProfile }) {
  const setSessions = useSessionStore((state) => state.setSessions);
  const setMessage = useSessionStore((state) => state.setMessage);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState<null | "setup" | "remove">(null);
  const [dialog, setDialog] = useState<null | "export" | "remove">(null);
  const key = profile.managedKey ?? null;

  const setUp = async (replacing: boolean) => {
    if (
      replacing &&
      !(await confirm(
        "A new key is installed and checked first; the current key is then removed from the server.",
        { title: "Replace the SSH key?", kind: "warning" },
      ))
    ) {
      return;
    }
    setBusy("setup");
    try {
      const profiles = await withHostKeyPrompt(() =>
        invoke<SessionProfile[]>("setup_managed_key", { profileId: profile.id }),
      );
      setSessions(profiles);
      setMessage({
        kind: "success",
        text: replacing
          ? `New SSH key installed on ${profile.host}`
          : `SSH key installed on ${profile.host}; GpuTerm signs in with it from now on`,
      });
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
    } finally {
      setBusy(null);
    }
  };

  const remove = async (removeFromServer: boolean) => {
    setBusy("remove");
    try {
      const profiles = await withHostKeyPrompt(() =>
        invoke<SessionProfile[]>("remove_managed_key", {
          profileId: profile.id,
          removeFromServer,
        }),
      );
      setSessions(profiles);
      setMessage({ kind: "success", text: "SSH key removed" });
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
    } finally {
      setBusy(null);
    }
  };

  const copyPublicKey = async () => {
    if (!key) return;
    try {
      await navigator.clipboard.writeText(key.publicKey);
      setMessage({ kind: "success", text: "Public key copied" });
    } catch (error) {
      setMessage({ kind: "error", text: `Could not copy: ${String(error)}` });
    }
  };

  return (
    <section className="port-forwards managed-key">
      <button
        className="port-forwards-toggle"
        type="button"
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
      >
        {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        <KeyRound size={14} />
        <span>SSH key</span>
        <small>{key ? "GpuTerm key" : "none"}</small>
      </button>
      {open && (
        <div className="port-forwards-body">
          {!key ? (
            <>
              <small className="port-forwards-empty">
                GpuTerm can create a key for this server, add it to the server&apos;s
                authorized_keys, and sign in with it from then on. The private key stays
                encrypted in GpuTerm&apos;s vault.
              </small>
              <button
                className="secondary-button compact"
                type="button"
                disabled={busy != null}
                onClick={() => void setUp(false)}
              >
                {busy === "setup" ? <LoaderCircle className="spin" size={14} /> : <KeyRound size={14} />}
                {busy === "setup" ? "Setting up…" : "Set up key"}
              </button>
            </>
          ) : (
            <>
              <dl className="managed-key-facts">
                <dt>Fingerprint</dt>
                <dd title={key.fingerprint}>{key.fingerprint}</dd>
                <dt>Created</dt>
                <dd>{formatDate(key.createdAt)}</dd>
              </dl>
              <div className="managed-key-actions">
                <button
                  className="secondary-button compact"
                  type="button"
                  onClick={() => void copyPublicKey()}
                >
                  <Copy size={14} />
                  Copy public key
                </button>
                <button
                  className="secondary-button compact"
                  type="button"
                  onClick={() => setDialog("export")}
                >
                  <Download size={14} />
                  Export…
                </button>
                <button
                  className="secondary-button compact"
                  type="button"
                  disabled={busy != null}
                  onClick={() => void setUp(true)}
                >
                  {busy === "setup" ? <LoaderCircle className="spin" size={14} /> : <RefreshCw size={14} />}
                  Replace
                </button>
                <button
                  className="secondary-button compact danger"
                  type="button"
                  disabled={busy != null}
                  onClick={() => setDialog("remove")}
                >
                  {busy === "remove" ? <LoaderCircle className="spin" size={14} /> : <Trash2 size={14} />}
                  Remove
                </button>
              </div>
            </>
          )}
        </div>
      )}
      {dialog === "export" && (
        <ExportDialog
          profile={profile}
          onClose={(path) => {
            setDialog(null);
            if (path) {
              setMessage({ kind: "success", text: `Private key exported to ${path}` });
            }
          }}
        />
      )}
      {dialog === "remove" && (
        <RemoveDialog
          profile={profile}
          onClose={(removeFromServer) => {
            setDialog(null);
            if (removeFromServer !== undefined) {
              void remove(removeFromServer);
            }
          }}
        />
      )}
    </section>
  );
}
