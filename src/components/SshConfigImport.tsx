import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AlertTriangle, LoaderCircle } from "lucide-react";
import type { SshConfigImportResult, SshConfigPreview } from "../types/session";

type SshConfigImportProps = {
  onClose: () => void;
  onImported: (result: SshConfigImportResult) => void;
};

/**
 * Previews the hosts in `~/.ssh/config` (or pasted text) and imports the
 * chosen ones as session profiles. Hosts matching a saved profile update it in
 * place, so its saved password stays attached.
 */
export function SshConfigImport({ onClose, onImported }: SshConfigImportProps) {
  const [preview, setPreview] = useState<SshConfigPreview | null>(null);
  const [loading, setLoading] = useState(true);
  const [importing, setImporting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // null: read ~/.ssh/config. A string: the pasted config.
  const [pastedText, setPastedText] = useState<string | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [defaultUser, setDefaultUser] = useState("");

  const load = async (text: string | null) => {
    setLoading(true);
    setError(null);
    try {
      const next = await invoke<SshConfigPreview>("preview_ssh_config", { text });
      setPreview(next);
      // New hosts start selected; ones already saved are opt-in updates.
      setSelected(
        new Set(
          next.hosts
            .filter((host) => !host.conflictProfileId)
            .map((host) => host.alias),
        ),
      );
    } catch (loadError) {
      setPreview(null);
      setError(String(loadError));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void load(null);
  }, []);

  const needsDefaultUser = useMemo(
    () =>
      preview?.hosts.some((host) => selected.has(host.alias) && !host.user) ?? false,
    [preview, selected],
  );

  const toggle = (alias: string) => {
    setSelected((current) => {
      const next = new Set(current);
      if (next.has(alias)) {
        next.delete(alias);
      } else {
        next.add(alias);
      }
      return next;
    });
  };

  const runImport = async () => {
    setImporting(true);
    setError(null);
    try {
      const result = await invoke<SshConfigImportResult>("import_ssh_config", {
        request: {
          text: pastedText,
          aliases: [...selected],
          defaultUser: defaultUser.trim() || null,
        },
      });
      onImported(result);
    } catch (importError) {
      setError(String(importError));
    } finally {
      setImporting(false);
    }
  };

  return (
    <div className="session-form ssh-config-import">
      <small className="form-binding-hint">
        Import from {preview?.sourcePath ?? (pastedText == null ? "~/.ssh/config" : "pasted config")}
      </small>

      {loading ? (
        <div className="ssh-config-status">
          <LoaderCircle className="spin" size={14} />
          Reading config…
        </div>
      ) : (
        <>
          {error && (
            <div className="ssh-config-status error" role="alert">
              {error}
            </div>
          )}
          {preview?.includedFiles && preview.includedFiles.length > 0 && (
            <div className="ssh-config-status">
              Also read: {preview.includedFiles.join(", ")}
            </div>
          )}
          {preview?.warnings.map((warning) => (
            <div className="ssh-config-status warning" key={warning}>
              <AlertTriangle size={13} />
              {warning}
            </div>
          ))}
          {preview && preview.hosts.length === 0 && (
            <div className="ssh-config-status">No concrete Host entries were found.</div>
          )}
          {preview && preview.hosts.length > 0 && (
            <div className="ssh-config-hosts">
              {preview.hosts.map((host) => (
                <label className="ssh-config-host" key={host.alias}>
                  <input
                    type="checkbox"
                    checked={selected.has(host.alias)}
                    onChange={() => toggle(host.alias)}
                  />
                  <span>
                    <strong>
                      {host.alias}
                      {host.conflictProfileId && <i>updates saved</i>}
                    </strong>
                    <small>
                      {host.user ? `${host.user}@` : ""}
                      {host.host}:{host.port}
                      {host.proxyJump ? ` · via ${host.proxyJump}` : ""}
                    </small>
                    {host.warnings.map((warning) => (
                      <small className="ssh-config-host-warning" key={warning}>
                        {warning}
                      </small>
                    ))}
                  </span>
                </label>
              ))}
            </div>
          )}
          {needsDefaultUser && (
            <label>
              <span>User for hosts without one</span>
              <input
                value={defaultUser}
                onChange={(event) => setDefaultUser(event.target.value)}
                placeholder="ubuntu"
              />
            </label>
          )}
          {pastedText != null && (
            <label>
              <span>Paste an ssh_config</span>
              <textarea
                className="ssh-config-text"
                rows={6}
                spellCheck={false}
                value={pastedText}
                onChange={(event) => setPastedText(event.target.value)}
                onBlur={() => void load(pastedText)}
              />
            </label>
          )}
        </>
      )}

      <div className="button-row">
        <button
          className="primary-button"
          type="button"
          disabled={
            loading ||
            importing ||
            selected.size === 0 ||
            (needsDefaultUser && !defaultUser.trim())
          }
          onClick={() => void runImport()}
        >
          {importing && <LoaderCircle className="spin" size={15} />}
          Import {selected.size > 0 ? selected.size : ""}
        </button>
        {pastedText == null && (
          <button
            className="secondary-button"
            type="button"
            disabled={loading || importing}
            onClick={() => setPastedText("")}
          >
            Paste text
          </button>
        )}
        <button className="secondary-button" type="button" onClick={onClose}>
          Cancel
        </button>
      </div>
    </div>
  );
}

export function describeImport(result: SshConfigImportResult): string {
  const parts = [];
  if (result.created > 0) {
    parts.push(`${result.created} added`);
  }
  if (result.updated > 0) {
    parts.push(`${result.updated} updated`);
  }
  const summary = parts.length > 0 ? `Imported: ${parts.join(", ")}` : "Nothing imported";
  return result.warnings.length > 0
    ? `${summary}\n${result.warnings.join("\n")}`
    : summary;
}
