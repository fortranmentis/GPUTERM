import type { SessionConnectRequest, SessionProfile } from "../types/session";

/**
 * A connect request that reopens a saved profile with the credentials the
 * backend still holds, so a reconnect never re-prompts for a password that
 * already worked. `interactive` lets a reconnect the user clicked show the
 * server's sign-in questions; an automatic one must not pop up a dialog.
 */
export function reconnectRequest(
  profile: SessionProfile,
  cols = 80,
  rows = 24,
  { interactive = false }: { interactive?: boolean } = {},
): SessionConnectRequest {
  return {
    id: profile.id,
    name: profile.name,
    host: profile.host,
    port: profile.port,
    username: profile.username,
    isLocal: profile.isLocal ?? false,
    password: null,
    privateKeyPath: profile.isLocal ? null : profile.privateKeyPath ?? null,
    proxyJumpId: profile.isLocal ? null : profile.proxyJumpId ?? null,
    proxyJumpPassword: null,
    reuseStoredCredentials: true,
    allowInteractiveAuth: interactive,
    cols,
    rows,
  };
}
