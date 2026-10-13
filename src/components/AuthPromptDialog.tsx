import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { KeyRound } from "lucide-react";
import { useEffect, useState, type FormEvent } from "react";

/** Mirrors `AuthPromptPayload` in src-tauri/src/ssh/kbdint.rs. */
export type AuthPromptRequest = {
  requestId: string;
  sessionId: string;
  host: string;
  port: number;
  username: string;
  isJump: boolean;
  instructions: string;
  prompts: Array<{ text: string; echo: boolean }>;
};

/**
 * Shows the server's sign-in questions (a one-time code, or a password asked
 * through keyboard-interactive) while a connection the user started waits.
 * Requests are answered one at a time: a jump host may ask before the target.
 */
export function AuthPromptDialog() {
  const [queue, setQueue] = useState<AuthPromptRequest[]>([]);
  const [answers, setAnswers] = useState<string[]>([]);
  const current = queue[0] ?? null;

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    listen<AuthPromptRequest>("ssh-auth-prompt", (event) => {
      setQueue((pending) => [...pending, event.payload]);
    }).then((next) => {
      if (disposed) {
        next();
      } else {
        unlisten = next;
      }
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    setAnswers(current ? current.prompts.map(() => "") : []);
  }, [current]);

  const finish = (responses: string[] | null) => {
    if (!current) return;
    void invoke("answer_auth_prompt", { requestId: current.requestId, responses }).catch(
      () => undefined,
    );
    setQueue((pending) => pending.slice(1));
  };

  useEffect(() => {
    if (!current) return;
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        finish(null);
      }
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  });

  if (!current) {
    return null;
  }

  const submit = (event: FormEvent) => {
    event.preventDefault();
    finish(answers);
  };

  return (
    <div className="app-modal-backdrop" role="presentation">
      <form
        className="app-modal auth-prompt-dialog"
        role="dialog"
        aria-label="Sign-in required"
        onSubmit={submit}
      >
        <div className="app-modal-header">
          <KeyRound size={18} />
          <strong>
            {current.username}@{current.host}
            {current.port !== 22 ? `:${current.port}` : ""}
          </strong>
          {current.isJump && <span className="auth-prompt-badge">Jump host</span>}
        </div>
        {current.instructions.trim() && (
          <p className="app-modal-text auth-prompt-instructions">{current.instructions.trim()}</p>
        )}
        {current.prompts.map((prompt, index) => (
          <label className="auth-prompt-field" key={index}>
            <span>{prompt.text.trim() || "Response"}</span>
            <input
              type={prompt.echo ? "text" : "password"}
              autoComplete={prompt.echo ? "off" : "one-time-code"}
              autoFocus={index === 0}
              value={answers[index] ?? ""}
              onChange={(event) =>
                setAnswers((current) =>
                  current.map((value, at) => (at === index ? event.target.value : value)),
                )
              }
            />
          </label>
        ))}
        <small className="session-credential-hint">
          The server asked for this. GpuTerm saves a password you enter here once the
          sign-in succeeds; codes are never saved.
        </small>
        <div className="app-modal-actions">
          <button className="secondary-button" type="button" onClick={() => finish(null)}>
            Cancel
          </button>
          <button className="primary-button" type="submit">
            Sign in
          </button>
        </div>
      </form>
    </div>
  );
}
