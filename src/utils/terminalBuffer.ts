/**
 * Holding buffer for terminal output that arrives before its session is
 * attached to the xterm instance (e.g. the MOTD emitted while the connect
 * invoke is still resolving, or output wiped by the reset on reconnect).
 */

const MAX_ENTRY_CHARS = 256 * 1024;
const MAX_ENTRIES = 8;

export function appendPendingOutput(
  buffers: Map<string, string>,
  sessionId: string,
  data: string,
) {
  let next = (buffers.get(sessionId) ?? "") + data;
  if (next.length > MAX_ENTRY_CHARS) {
    // Keep the tail — that is what a terminal would be showing.
    next = next.slice(next.length - MAX_ENTRY_CHARS);
  }
  // Re-insert so Map iteration order tracks recency for the entry cap.
  buffers.delete(sessionId);
  buffers.set(sessionId, next);
  while (buffers.size > MAX_ENTRIES) {
    const oldest = buffers.keys().next().value;
    if (oldest === undefined) {
      break;
    }
    buffers.delete(oldest);
  }
}

export function takePendingOutput(
  buffers: Map<string, string>,
  sessionId: string,
): string | null {
  const pending = buffers.get(sessionId) ?? null;
  buffers.delete(sessionId);
  return pending;
}

export function clearPendingOutput(buffers: Map<string, string>, sessionId: string) {
  buffers.delete(sessionId);
}
