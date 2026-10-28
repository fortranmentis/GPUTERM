import { useEffect } from "react";
import { Download, ExternalLink, LoaderCircle, RefreshCw } from "lucide-react";
import { useUpdateStore } from "../stores/updateStore";

/** First automatic check after launch, then every six hours. */
const FIRST_CHECK_MS = 5_000;
const CHECK_INTERVAL_MS = 6 * 60 * 60 * 1000;

/** Runs the automatic update checks; mount once, in the main window. */
export function useUpdateChecks() {
  const init = useUpdateStore((state) => state.init);
  const checkForUpdates = useUpdateStore((state) => state.checkForUpdates);
  useEffect(() => {
    void init();
    const first = window.setTimeout(() => void checkForUpdates(false), FIRST_CHECK_MS);
    const interval = window.setInterval(() => void checkForUpdates(false), CHECK_INTERVAL_MS);
    return () => {
      window.clearTimeout(first);
      window.clearInterval(interval);
    };
  }, [init, checkForUpdates]);
}

export function UpdateDialog() {
  const {
    currentVersion,
    installMode,
    phase,
    update,
    progress,
    error,
    dialogOpen,
    install,
    restart,
    skipVersion,
    openDownloadPage,
    closeDialog,
  } = useUpdateStore();

  useEffect(() => {
    if (!dialogOpen || phase === "downloading") return;
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") closeDialog();
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [dialogOpen, phase, closeDialog]);

  if (!dialogOpen) {
    return null;
  }

  const title =
    phase === "checking"
      ? "Checking for updates…"
      : phase === "upToDate"
        ? "GpuTerm is up to date"
        : update
          ? `GpuTerm ${update.version} is available`
          : "Update";

  return (
    <div className="app-modal-backdrop update-backdrop" role="presentation">
      <div className="app-modal update-dialog" role="dialog" aria-label="Software update">
        <div className="app-modal-header">
          {phase === "checking" ? <LoaderCircle className="spin" size={18} /> : <Download size={18} />}
          <strong>{title}</strong>
        </div>
        <p className="app-modal-text">
          {phase === "upToDate"
            ? `You have the latest version${currentVersion ? ` (${currentVersion})` : ""}.`
            : currentVersion && update
              ? `You have ${currentVersion}.`
              : null}
        </p>
        {update?.body && phase !== "upToDate" && (
          <pre className="update-notes" aria-label="Release notes">
            {update.body.trim()}
          </pre>
        )}
        {installMode === "manual" && update && phase === "available" && (
          <p className="app-modal-text update-hint">
            This copy was installed from a package, so download the new version from the release
            page.
          </p>
        )}
        {phase === "downloading" && (
          <div className="update-progress" role="progressbar" aria-valuenow={progress ?? undefined}
            aria-valuemin={0} aria-valuemax={100} aria-label="Download progress">
            <span style={{ width: `${progress ?? 100}%` }} className={progress == null ? "indeterminate" : undefined} />
          </div>
        )}
        {phase === "ready" && (
          <p className="app-modal-text">The update is installed. Restart GpuTerm to use it.</p>
        )}
        {error && (
          <p className="update-error" role="alert">
            {error}
          </p>
        )}
        <div className="app-modal-actions">
          {phase === "available" && update && (
            <>
              <button className="secondary-button" type="button" onClick={skipVersion}>
                Skip this version
              </button>
              <button className="secondary-button" type="button" onClick={closeDialog}>
                Later
              </button>
              {installMode === "manual" ? (
                <button className="primary-button" type="button" onClick={() => void openDownloadPage()}>
                  <ExternalLink size={14} />
                  Open download page
                </button>
              ) : (
                <button className="primary-button" type="button" autoFocus onClick={() => void install()}>
                  <Download size={14} />
                  Update
                </button>
              )}
            </>
          )}
          {phase === "downloading" && (
            <button className="primary-button" type="button" disabled>
              <LoaderCircle className="spin" size={14} />
              Installing…
            </button>
          )}
          {phase === "ready" && (
            <>
              <button className="secondary-button" type="button" onClick={closeDialog}>
                Later
              </button>
              <button className="primary-button" type="button" autoFocus onClick={() => void restart()}>
                <RefreshCw size={14} />
                Restart now
              </button>
            </>
          )}
          {phase === "error" && (
            <>
              <button className="secondary-button" type="button" onClick={() => void openDownloadPage()}>
                <ExternalLink size={14} />
                Open download page
              </button>
              <button className="primary-button" type="button" onClick={closeDialog}>
                Close
              </button>
            </>
          )}
          {(phase === "checking" || phase === "upToDate" || phase === "idle") && (
            <button className="primary-button" type="button" autoFocus onClick={closeDialog}>
              Close
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
