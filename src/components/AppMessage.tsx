import { useEffect } from "react";
import { AlertCircle, AlertTriangle, CheckCircle2, Info, X } from "lucide-react";
import { useSessionStore } from "../stores/sessionStore";

const TOAST_DISMISS_MS = 3_000;
/** Alerts and toasts with a button stay up longer: they may arrive while the
 * user looks elsewhere. */
const WARNING_DISMISS_MS = 8_000;

/**
 * Renders the app-wide message: errors as a centered modal that must be
 * acknowledged, info/success/warning as a top-center toast that dismisses
 * itself.
 */
export function AppMessageOverlay() {
  const message = useSessionStore((state) => state.message);
  const setMessage = useSessionStore((state) => state.setMessage);

  useEffect(() => {
    if (!message || message.kind === "error") {
      return;
    }
    const timer = window.setTimeout(
      () => setMessage(null),
      message.kind === "warning" || message.action ? WARNING_DISMISS_MS : TOAST_DISMISS_MS,
    );
    return () => window.clearTimeout(timer);
  }, [message, setMessage]);

  useEffect(() => {
    if (message?.kind !== "error") {
      return;
    }
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setMessage(null);
      }
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [message, setMessage]);

  if (!message) {
    return null;
  }

  if (message.kind === "error") {
    return (
      <div
        className="app-modal-backdrop"
        onClick={() => setMessage(null)}
        role="presentation"
      >
        <div
          className="app-modal error"
          role="alertdialog"
          aria-label="Error"
          onClick={(event) => event.stopPropagation()}
        >
          <div className="app-modal-header">
            <AlertCircle size={18} />
            <strong>Error</strong>
          </div>
          <p className="app-modal-text">{message.text}</p>
          <div className="app-modal-actions">
            <button
              className="primary-button"
              type="button"
              autoFocus
              onClick={() => setMessage(null)}
            >
              OK
            </button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div
      className={`app-message app-toast ${message.kind}${message.action ? " has-action" : ""}`}
      role="status"
    >
      {message.kind === "success" ? (
        <CheckCircle2 size={16} />
      ) : message.kind === "warning" ? (
        <AlertTriangle size={16} />
      ) : (
        <Info size={16} />
      )}
      <span>{message.text}</span>
      {message.action && (
        <button
          className="secondary-button app-toast-action"
          type="button"
          onClick={() => {
            message.action?.run();
            setMessage(null);
          }}
        >
          {message.action.label}
        </button>
      )}
      <button
        className="icon-button ghost"
        type="button"
        aria-label="Dismiss message"
        title="Dismiss"
        onClick={() => setMessage(null)}
      >
        <X size={16} />
      </button>
    </div>
  );
}
