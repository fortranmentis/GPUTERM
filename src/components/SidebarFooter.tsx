import { RefreshCw } from "lucide-react";
import { useUpdateStore } from "../stores/updateStore";

/** The app version and the update check, at the bottom of the sidebar. */
export function SidebarFooter() {
  const currentVersion = useUpdateStore((state) => state.currentVersion);
  const phase = useUpdateStore((state) => state.phase);
  const update = useUpdateStore((state) => state.update);
  const checkForUpdates = useUpdateStore((state) => state.checkForUpdates);
  const openDialog = useUpdateStore((state) => state.openDialog);
  const pending = update != null && (phase === "available" || phase === "ready");

  return (
    <div className="sidebar-footer">
      <span className="sidebar-version">{currentVersion ? `v${currentVersion}` : "GpuTerm"}</span>
      {pending ? (
        <button
          className="sidebar-update-button available"
          type="button"
          title={`GpuTerm ${update.version} is available`}
          onClick={openDialog}
        >
          <span className="update-dot" aria-hidden="true" />
          Update to {update.version}
        </button>
      ) : (
        <button
          className="sidebar-update-button"
          type="button"
          disabled={phase === "checking" || phase === "downloading"}
          onClick={() => void checkForUpdates(true)}
        >
          <RefreshCw size={12} className={phase === "checking" ? "spin" : undefined} />
          Check for updates
        </button>
      )}
    </div>
  );
}
