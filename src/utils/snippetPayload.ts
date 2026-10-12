import type { Snippet } from "../types/snippet";

const PASTE_START = "\u001b[200~";
const PASTE_END = "\u001b[201~";

/**
 * The bytes to type for a snippet. Line breaks become Return (CR), as a
 * keyboard sends them. A multi-line command is wrapped in bracketed-paste
 * markers when the remote program enabled them, so a shell or editor takes it
 * as one paste instead of running each line as it arrives.
 */
export function snippetPayload(
  command: string,
  appendReturn: boolean,
  bracketedPaste: boolean,
): string {
  let body = command.replace(/\r\n/g, "\n").replace(/\n+$/, "");
  const multiline = body.includes("\n");
  body = body.replace(/\n/g, "\r");
  if (multiline && bracketedPaste) {
    body = PASTE_START + body + PASTE_END;
  }
  return appendReturn ? `${body}\r` : body;
}

/** Whether a snippet is offered for the given saved profile. */
export function snippetAppliesTo(
  snippet: Snippet,
  profileId: string | null,
): boolean {
  if (!snippet.profileIds || snippet.profileIds.length === 0) {
    return true;
  }
  return profileId != null && snippet.profileIds.includes(profileId);
}
