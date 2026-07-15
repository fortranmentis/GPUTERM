import { Users } from "lucide-react";
import type { RefObject } from "react";
import type { RemoteUserSession } from "../types/gpu";
import { ResourceDetailPopover } from "./ResourceDetailPopover";

type UsersPopoverProps = {
  users: RemoteUserSession[];
  error?: string | null;
  anchorRef: RefObject<HTMLElement | null>;
  onClose: () => void;
};

export function UsersPopover({ users, error, anchorRef, onClose }: UsersPopoverProps) {
  return (
    <ResourceDetailPopover
      anchorRef={anchorRef}
      ariaLabel="Logged-in users"
      title="Logged-in users"
      icon={<Users size={16} />}
      onClose={onClose}
    >
      {users.length === 0 ? (
        <div className="empty-list">{error ?? "No login sessions"}</div>
      ) : (
        <div className="disk-detail-table users-detail-table">
          <div className="disk-detail-row head">
            <span>User</span>
            <span>TTY</span>
            <span>Login time</span>
            <span>From</span>
          </div>
          {users.map((session, index) => (
            <div
              className="disk-detail-row"
              key={`${session.user}:${session.tty}:${index}`}
            >
              <span title={session.user}>{session.user}</span>
              <span>{session.tty}</span>
              <span>{session.loginTime}</span>
              <span title={session.from ?? undefined}>{session.from ?? "-"}</span>
            </div>
          ))}
        </div>
      )}
    </ResourceDetailPopover>
  );
}
