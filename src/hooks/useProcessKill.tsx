import { useCallback, useState } from "react";
import { KillProcessDialog } from "../components/KillProcessDialog";
import type { KillTarget } from "../types/processControl";

/**
 * Owns the end-process dialog for a view that shows process tables. The
 * dialog lives here, not inside a popover, so it survives the popover's
 * refreshes. `request` is undefined when there is no session to act on,
 * which hides the table's end-process buttons.
 */
export function useProcessKill(sessionId: string | null, onDone: () => void) {
  const [target, setTarget] = useState<KillTarget | null>(null);
  const close = useCallback(() => setTarget(null), []);
  const dialog =
    sessionId && target ? (
      <KillProcessDialog
        key={target.pid}
        sessionId={sessionId}
        target={target}
        onClose={close}
        onDone={onDone}
      />
    ) : null;
  return { request: sessionId ? setTarget : undefined, dialog };
}
