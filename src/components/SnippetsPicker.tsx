import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { LoaderCircle, Pencil, Plus, ScrollText, X } from "lucide-react";
import type { Snippet } from "../types/snippet";
import { snippetAppliesTo } from "../utils/snippetPayload";

type SnippetsPickerProps = {
  /** Saved profile of the focused pane; scopes the list and new snippets. */
  profileId: string | null;
  profileName: string | null;
  disabled: boolean;
  onSend: (snippet: Snippet) => void;
};

type Draft = {
  id: string;
  name: string;
  command: string;
  appendReturn: boolean;
  onlyThisServer: boolean;
};

const blankDraft: Draft = {
  id: "",
  name: "",
  command: "",
  appendReturn: true,
  onlyThisServer: false,
};

/**
 * Toolbar popover of saved commands. Clicking one types it into the focused
 * pane; snippets can be global or limited to particular servers.
 */
export function SnippetsPicker({
  profileId,
  profileName,
  disabled,
  onSend,
}: SnippetsPickerProps) {
  const anchorRef = useRef<HTMLDivElement | null>(null);
  const [open, setOpen] = useState(false);
  const [snippets, setSnippets] = useState<Snippet[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [saving, setSaving] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setSnippets(await invoke<Snippet[]>("list_snippets"));
    } catch (loadError) {
      setError(String(loadError));
    } finally {
      setLoading(false);
    }
  }, []);

  const close = useCallback(() => {
    setOpen(false);
    setDraft(null);
    setError(null);
  }, []);

  useEffect(() => {
    if (!open) {
      return;
    }
    const handlePointerDown = (event: MouseEvent) => {
      if (!anchorRef.current?.contains(event.target as Node)) {
        close();
      }
    };
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        close();
      }
    };
    document.addEventListener("mousedown", handlePointerDown);
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("mousedown", handlePointerDown);
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [close, open]);

  const visible = (snippets ?? []).filter((snippet) =>
    snippetAppliesTo(snippet, profileId),
  );

  const save = async (event: FormEvent) => {
    event.preventDefault();
    if (!draft) {
      return;
    }
    setSaving(true);
    setError(null);
    try {
      const existing = snippets?.find((snippet) => snippet.id === draft.id);
      // Keep any other servers an existing snippet is scoped to; the checkbox
      // only adds or removes the focused one.
      const otherProfiles = (existing?.profileIds ?? []).filter(
        (id) => id !== profileId,
      );
      const profileIds =
        draft.onlyThisServer && profileId
          ? [...otherProfiles, profileId]
          : otherProfiles;
      const next = await invoke<Snippet[]>("save_snippet", {
        snippet: {
          id: draft.id,
          name: draft.name,
          command: draft.command,
          appendReturn: draft.appendReturn,
          profileIds: profileIds.length > 0 ? profileIds : undefined,
        },
      });
      setSnippets(next);
      setDraft(null);
    } catch (saveError) {
      setError(String(saveError));
    } finally {
      setSaving(false);
    }
  };

  const remove = async (snippet: Snippet) => {
    setError(null);
    try {
      setSnippets(await invoke<Snippet[]>("delete_snippet", { id: snippet.id }));
    } catch (deleteError) {
      setError(String(deleteError));
    }
  };

  return (
    <div className="terminal-tmux-picker-anchor" ref={anchorRef}>
      <button
        className="icon-button"
        type="button"
        disabled={disabled}
        aria-label="Snippets"
        title="Saved commands"
        aria-expanded={open}
        onClick={() => {
          if (open) {
            close();
          } else {
            setOpen(true);
            void load();
          }
        }}
      >
        <ScrollText size={16} />
      </button>
      {open && (
        <div
          className="terminal-tmux-picker terminal-snippets-picker"
          role="dialog"
          aria-label="Snippets"
        >
          <div className="terminal-tmux-picker-title">
            <span className="terminal-session-picker-heading">Snippets</span>
            {!draft && (
              <button
                className="icon-button ghost"
                type="button"
                aria-label="New snippet"
                title="New snippet"
                onClick={() => setDraft({ ...blankDraft })}
              >
                <Plus size={14} />
              </button>
            )}
          </div>
          {error && (
            <div className="terminal-tmux-status error" role="alert">
              {error}
            </div>
          )}
          {draft ? (
            <form onSubmit={(event) => void save(event)}>
              <label>
                <span>Name</span>
                <input
                  value={draft.name}
                  placeholder="Defaults to the first line"
                  onChange={(event) =>
                    setDraft({ ...draft, name: event.target.value })
                  }
                />
              </label>
              <label>
                <span>Command</span>
                <textarea
                  value={draft.command}
                  rows={4}
                  spellCheck={false}
                  autoFocus
                  placeholder="watch -n 1 nvidia-smi"
                  onChange={(event) =>
                    setDraft({ ...draft, command: event.target.value })
                  }
                />
              </label>
              <label className="terminal-snippet-check">
                <input
                  type="checkbox"
                  checked={draft.appendReturn}
                  onChange={(event) =>
                    setDraft({ ...draft, appendReturn: event.target.checked })
                  }
                />
                <span>Press Return after sending</span>
              </label>
              {profileId && (
                <label className="terminal-snippet-check">
                  <input
                    type="checkbox"
                    checked={draft.onlyThisServer}
                    onChange={(event) =>
                      setDraft({ ...draft, onlyThisServer: event.target.checked })
                    }
                  />
                  <span>Only for {profileName ?? "this server"}</span>
                </label>
              )}
              <div className="terminal-snippet-actions">
                <button
                  className="secondary-button"
                  type="button"
                  onClick={() => setDraft(null)}
                >
                  Cancel
                </button>
                <button
                  className="primary-button"
                  type="submit"
                  disabled={saving || draft.command.trim() === ""}
                >
                  {saving && <LoaderCircle className="spin" size={15} />}
                  Save
                </button>
              </div>
            </form>
          ) : loading && snippets == null ? (
            <div className="terminal-tmux-status">
              <LoaderCircle className="spin" size={14} />
              Loading snippets…
            </div>
          ) : visible.length === 0 ? (
            <div className="terminal-tmux-status">
              No snippets yet. Save commands you run often with +.
            </div>
          ) : (
            <div className="terminal-session-picker-list">
              {visible.map((snippet) => (
                <div className="terminal-snippet-row" key={snippet.id}>
                  <button
                    type="button"
                    title={snippet.command}
                    onClick={() => {
                      onSend(snippet);
                      close();
                    }}
                  >
                    <ScrollText size={15} />
                    <span>
                      <strong>{snippet.name}</strong>
                      <small>{snippet.command.split("\n")[0]}</small>
                    </span>
                    {snippet.profileIds && snippet.profileIds.length > 0 && (
                      <i title="Limited to specific servers">Scoped</i>
                    )}
                  </button>
                  <button
                    className="icon-button ghost"
                    type="button"
                    aria-label={`Edit snippet ${snippet.name}`}
                    title="Edit"
                    onClick={() =>
                      setDraft({
                        id: snippet.id,
                        name: snippet.name,
                        command: snippet.command,
                        appendReturn: snippet.appendReturn,
                        onlyThisServer:
                          profileId != null &&
                          (snippet.profileIds ?? []).includes(profileId),
                      })
                    }
                  >
                    <Pencil size={13} />
                  </button>
                  <button
                    className="icon-button ghost"
                    type="button"
                    aria-label={`Delete snippet ${snippet.name}`}
                    title="Delete"
                    onClick={() => void remove(snippet)}
                  >
                    <X size={13} />
                  </button>
                </div>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
