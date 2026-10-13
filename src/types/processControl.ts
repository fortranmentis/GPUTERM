/** A row the user asked to end, as the table showed it. */
export type KillTarget = {
  pid: number;
  user: string | null;
  command: string | null;
};

/** Mirrors `ProcessIdentity` in src-tauri/src/ssh/process_control.rs. */
export type ProcessIdentity = {
  pid: number;
  alive: boolean;
  user: string | null;
  command: string | null;
  startMarker: string | null;
};

export type SignalStatus =
  | "sent"
  | "gone"
  | "identity_changed"
  | "permission_denied"
  | "needs_force"
  | "failed";

export type SignalOutcome = {
  status: SignalStatus;
  message: string | null;
};
