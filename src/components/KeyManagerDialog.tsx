import { useEffect, useState, type FormEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { confirm, save } from "@tauri-apps/plugin-dialog";
import {
  ChevronDown,
  ChevronRight,
  Copy,
  Download,
  FileKey,
  KeyRound,
  LoaderCircle,
  PenLine,
  Plus,
  Trash2,
  X,
} from "lucide-react";
import { useKeyStore } from "../stores/keyStore";
import { useSessionStore } from "../stores/sessionStore";
import { passphraseProblem } from "./ManagedKeyPanel";
import type { KeyKind, StoredKey } from "../types/keys";
import type { SessionProfile } from "../types/session";

/** Generator choices, in menu order. */
export const KEY_KIND_OPTIONS: { kind: KeyKind; title: string }[] = [
  { kind: "ed25519", title: "Ed25519 (recommended)" },
  { kind: "rsa-4096", title: "RSA 4096" },
  { kind: "rsa-3072", title: "RSA 3072" },
  { kind: "ecdsa-p256", title: "ECDSA P-256" },
  { kind: "ecdsa-p384", title: "ECDSA P-384" },
  { kind: "ecdsa-p521", title: "ECDSA P-521" },
];

/** Short type name for lists: `Ed25519`, `RSA 4096`, `ECDSA P-256`. */
export function kindTitle(kind: string) {
  if (kind === "ed25519") return "Ed25519";
  return KEY_KIND_OPTIONS.find((option) => option.kind === kind)?.title ?? kind;
}

/** Which saved servers sign in with the key. */
export function keyUsage(key: StoredKey, profiles: SessionProfile[]) {
  const names = profiles
    .filter((profile) => key.profileIds.includes(profile.id))
    .map((profile) => profile.name);
  if (names.length === 0) return "Not used by a server yet";
  if (names.length === 1) return `Used by ${names[0]}`;
  return `Used by ${names.length} servers`;
}

/** `id_ed25519_lab-laptop`: the ssh-keygen default name plus the key's name. */
export function keyFileName(key: Pick<StoredKey, "kind" | "name">) {
  const stem = key.kind.startsWith("rsa")
    ? "id_rsa"
    : key.kind.startsWith("ecdsa")
      ? "id_ecdsa"
      : key.kind === "ed25519"
        ? "id_ed25519"
        : "id_key";
  const safe = key.name.replace(/[^A-Za-z0-9._-]+/g, "-").replace(/^-+|-+$/g, "");
  return safe ? `${stem}_${safe}` : stem;
}

/** Why the export cannot go ahead yet, or null when it can. */
export function exportProblem(passphrase: string, repeat: string, unencrypted: boolean) {
  return unencrypted ? null : passphraseProblem(passphrase, repeat);
}

function formatCreated(epochSeconds: number) {
  return new Date(epochSeconds * 1000).toLocaleString();
}

type Notice = { kind: "success" | "error"; text: string } | null;

function GenerateForm({
  onDone,
  onCancel,
}: {
  onDone: (notice: Notice) => void;
  onCancel: () => void;
}) {
  const setKeys = useKeyStore((state) => state.setKeys);
  const [name, setName] = useState("");
  const [kind, setKind] = useState<KeyKind>("ed25519");
  const [comment, setComment] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!name.trim() || busy) return;
    setBusy(true);
    setError(null);
    try {
      const keys = await invoke<StoredKey[]>("generate_key", {
        name: name.trim(),
        kind,
        comment: comment.trim() || null,
      });
      setKeys(keys);
      onDone({ kind: "success", text: `Key pair ${name.trim()} generated` });
    } catch (generateError) {
      setError(String(generateError));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      className="key-generate-form"
      aria-label="Generate key pair"
      onSubmit={(event) => void submit(event)}
    >
      <label>
        <span>Name</span>
        <input
          autoFocus
          value={name}
          placeholder="Lab laptop"
          onChange={(event) => setName(event.target.value)}
        />
      </label>
      <label>
        <span>Type</span>
        <select value={kind} onChange={(event) => setKind(event.target.value as KeyKind)}>
          {KEY_KIND_OPTIONS.map((option) => (
            <option key={option.kind} value={option.kind}>
              {option.title}
            </option>
          ))}
        </select>
      </label>
      <label>
        <span>Comment (optional)</span>
        <input
          value={comment}
          placeholder="you@laptop"
          onChange={(event) => setComment(event.target.value)}
        />
      </label>
      {kind.startsWith("rsa") && (
        <small className="key-manager-hint">RSA keys can take several seconds to generate.</small>
      )}
      {error && (
        <small className="managed-key-problem" role="alert">
          {error}
        </small>
      )}
      <div className="app-modal-actions key-manager-actions">
        <button className="secondary-button compact" type="button" onClick={onCancel}>
          Cancel
        </button>
        <button
          className="primary-button compact"
          type="submit"
          disabled={busy || !name.trim()}
        >
          {busy && <LoaderCircle className="spin" size={14} />}
          {busy ? "Generating…" : "Generate"}
        </button>
      </div>
    </form>
  );
}

function ExportPrivateDialog({
  storedKey,
  onClose,
}: {
  storedKey: StoredKey;
  onClose: (exportedTo?: string) => void;
}) {
  const [passphrase, setPassphrase] = useState("");
  const [repeat, setRepeat] = useState("");
  const [unencrypted, setUnencrypted] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const problem = exportProblem(passphrase, repeat, unencrypted);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (problem) return;
    setError(null);
    const path = await save({ defaultPath: keyFileName(storedKey) });
    if (!path) return;
    setBusy(true);
    try {
      await invoke("export_private_key", {
        keyId: storedKey.id,
        path,
        passphrase: unencrypted ? null : passphrase,
      });
      onClose(path);
    } catch (exportError) {
      setError(String(exportError));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="app-modal-backdrop key-export-backdrop" role="presentation">
      <form
        className="app-modal managed-key-dialog"
        role="dialog"
        aria-modal="true"
        aria-label={`Export ${storedKey.name}`}
        onSubmit={(event) => void submit(event)}
      >
        <div className="app-modal-header key-manager-title">
          <Download size={18} />
          <strong>Export {storedKey.name}</strong>
        </div>
        <p className="app-modal-text">
          The private key is saved in OpenSSH format, as ssh-keygen writes it, with its public key
          next to it as <code>.pub</code>. Anyone with the file can sign in wherever this key is
          installed.
        </p>
        {!unencrypted && (
          <>
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
          </>
        )}
        <label className="managed-key-check">
          <input
            type="checkbox"
            checked={unencrypted}
            onChange={(event) => setUnencrypted(event.target.checked)}
          />
          <span>Save without a passphrase</span>
        </label>
        {unencrypted && (
          <small className="managed-key-problem">
            The file will hold the key in plain text. Keep it somewhere only you can read.
          </small>
        )}
        {error && (
          <small className="managed-key-problem" role="alert">
            {error}
          </small>
        )}
        <div className="app-modal-actions key-manager-actions">
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

function KeyRow({
  storedKey,
  profiles,
  onNotice,
  onExport,
}: {
  storedKey: StoredKey;
  profiles: SessionProfile[];
  onNotice: (notice: Notice) => void;
  onExport: (key: StoredKey) => void;
}) {
  const setKeys = useKeyStore((state) => state.setKeys);
  const [open, setOpen] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [newName, setNewName] = useState(storedKey.name);
  const [busy, setBusy] = useState(false);
  const usage = keyUsage(storedKey, profiles);

  const run = async (work: () => Promise<void>) => {
    setBusy(true);
    try {
      await work();
    } catch (error) {
      onNotice({ kind: "error", text: String(error) });
    } finally {
      setBusy(false);
    }
  };

  const copyPublicKey = () =>
    run(async () => {
      await navigator.clipboard.writeText(storedKey.publicKey);
      onNotice({ kind: "success", text: "Public key copied" });
    });

  const savePublicKey = () =>
    run(async () => {
      const path = await save({ defaultPath: `${keyFileName(storedKey)}.pub` });
      if (!path) return;
      await invoke("save_public_key", { keyId: storedKey.id, path });
      onNotice({ kind: "success", text: `Public key saved to ${path}` });
    });

  const rename = (event: FormEvent) => {
    event.preventDefault();
    if (!newName.trim()) return;
    void run(async () => {
      setKeys(
        await invoke<StoredKey[]>("rename_key", { keyId: storedKey.id, name: newName.trim() }),
      );
      setRenaming(false);
    });
  };

  const remove = () =>
    run(async () => {
      const confirmed = await confirm(
        `The private key is erased from this computer. Servers keep it in authorized_keys until you remove it there. ${usage}.`,
        { title: `Delete ${storedKey.name}?`, kind: "warning", okLabel: "Delete" },
      );
      if (!confirmed) return;
      setKeys(await invoke<StoredKey[]>("delete_key", { keyId: storedKey.id }));
      onNotice({ kind: "success", text: `${storedKey.name} deleted` });
    });

  return (
    <li className="key-row">
      <button
        className="key-row-summary"
        type="button"
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
      >
        {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        <KeyRound size={15} />
        <span className="key-row-text">
          <strong>{storedKey.name}</strong>
          <small>
            {kindTitle(storedKey.kind)} · {usage}
          </small>
        </span>
      </button>
      {open && (
        <div className="key-row-details">
          <dl className="managed-key-facts">
            <dt>Fingerprint</dt>
            <dd title={storedKey.fingerprint}>{storedKey.fingerprint}</dd>
            <dt>Created</dt>
            <dd>{formatCreated(storedKey.createdAt)}</dd>
          </dl>
          <code className="key-row-public" aria-label="Public key">
            {storedKey.publicKey}
          </code>
          {renaming && (
            <form className="key-rename-row" onSubmit={rename}>
              <input
                autoFocus
                aria-label={`New name for ${storedKey.name}`}
                value={newName}
                onChange={(event) => setNewName(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Escape") {
                    event.stopPropagation();
                    setRenaming(false);
                  }
                }}
              />
              <button
                className="secondary-button compact"
                type="submit"
                disabled={busy || !newName.trim()}
              >
                Save name
              </button>
              <button
                className="secondary-button compact"
                type="button"
                onClick={() => setRenaming(false)}
              >
                Cancel
              </button>
            </form>
          )}
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
              disabled={busy}
              onClick={() => void savePublicKey()}
            >
              <Download size={14} />
              Save .pub…
            </button>
            <button
              className="secondary-button compact"
              type="button"
              disabled={busy}
              onClick={() => onExport(storedKey)}
            >
              <FileKey size={14} />
              Export private…
            </button>
            <button
              className="secondary-button compact"
              type="button"
              disabled={busy}
              onClick={() => {
                setNewName(storedKey.name);
                setRenaming(true);
              }}
            >
              <PenLine size={14} />
              Rename
            </button>
            <button
              className="secondary-button compact danger"
              type="button"
              disabled={busy}
              onClick={() => void remove()}
            >
              <Trash2 size={14} />
              Delete
            </button>
          </div>
        </div>
      )}
    </li>
  );
}

/**
 * The key manager: generate key pairs on this computer, share or save their
 * public halves, export private keys, rename and delete them.
 */
export function KeyManagerDialog({ onClose }: { onClose: () => void }) {
  const keys = useKeyStore((state) => state.keys);
  const refresh = useKeyStore((state) => state.refresh);
  const profiles = useSessionStore((state) => state.sessions);
  const [generating, setGenerating] = useState(false);
  const [exporting, setExporting] = useState<StoredKey | null>(null);
  const [notice, setNotice] = useState<Notice>(null);

  useEffect(() => {
    refresh().catch((error) => setNotice({ kind: "error", text: String(error) }));
  }, [refresh]);

  useEffect(() => {
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !exporting) {
        onClose();
      }
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [exporting, onClose]);

  return (
    <div className="app-modal-backdrop" role="presentation">
      <div
        className="app-modal key-manager-dialog"
        role="dialog"
        aria-modal="true"
        aria-label="SSH keys"
      >
        <div className="app-modal-header key-manager-title">
          <KeyRound size={18} />
          <strong>SSH keys</strong>
          <button
            className="icon-button ghost key-manager-close"
            type="button"
            aria-label="Close SSH keys"
            title="Close"
            onClick={onClose}
          >
            <X size={15} />
          </button>
        </div>
        <p className="app-modal-text">
          Key pairs made on this computer. Private keys stay encrypted in GpuTerm&apos;s vault.
        </p>
        {notice && (
          <small
            className={notice.kind === "error" ? "managed-key-problem" : "key-manager-success"}
            role={notice.kind === "error" ? "alert" : "status"}
          >
            {notice.text}
          </small>
        )}
        {generating ? (
          <GenerateForm
            onCancel={() => setGenerating(false)}
            onDone={(result) => {
              setGenerating(false);
              setNotice(result);
            }}
          />
        ) : (
          <button
            className="secondary-button compact key-generate-button"
            type="button"
            onClick={() => {
              setNotice(null);
              setGenerating(true);
            }}
          >
            <Plus size={14} />
            Generate key pair
          </button>
        )}
        {keys.length === 0 ? (
          <small className="port-forwards-empty">No keys yet.</small>
        ) : (
          <ul className="key-list" aria-label="SSH keys">
            {keys.map((key) => (
              <KeyRow
                key={key.id}
                storedKey={key}
                profiles={profiles}
                onNotice={setNotice}
                onExport={setExporting}
              />
            ))}
          </ul>
        )}
      </div>
      {exporting && (
        <ExportPrivateDialog
          storedKey={exporting}
          onClose={(path) => {
            setExporting(null);
            if (path) {
              setNotice({ kind: "success", text: `Private key exported to ${path}` });
            }
          }}
        />
      )}
    </div>
  );
}
