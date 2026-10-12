import { invoke } from "@tauri-apps/api/core";
import { confirm } from "@tauri-apps/plugin-dialog";

const UNKNOWN_HOST_KEY_PREFIX = "UNKNOWN_HOST_KEY:";
const HOST_KEY_CHANGED_PREFIX = "HOST_KEY_CHANGED:";
// A jump-host connection can raise one host-key prompt per hop
// (bastion + target), so allow a few prompt-and-retry rounds.
const MAX_HOST_KEY_PROMPTS = 3;

export type HostKeyChallenge = {
  kind: "unknown" | "changed";
  /** Previously trusted fingerprint; only set for a changed key. */
  expected?: string;
  fingerprint: string;
  keyType: string;
  hostKey: string;
  host: string;
  port: number;
};

/**
 * Parses a host-key sentinel out of a backend error.
 * Unknown: `UNKNOWN_HOST_KEY:{fingerprint}|{keyType}|{host}:{port}`.
 * Changed: `HOST_KEY_CHANGED:{expected}|{fingerprint}|{keyType}|{host}:{port}`.
 * Fingerprints are hex and key types contain no `|`, so the leading `|`
 * delimiters are unambiguous; the port is split at the last `:` because IPv6
 * hosts contain colons.
 */
export function parseHostKeyChallenge(text: string): HostKeyChallenge | null {
  const changedIndex = text.indexOf(HOST_KEY_CHANGED_PREFIX);
  const unknownIndex = text.indexOf(UNKNOWN_HOST_KEY_PREFIX);
  let kind: HostKeyChallenge["kind"];
  let fields: string[];
  if (changedIndex >= 0) {
    kind = "changed";
    fields = splitFields(
      text.slice(changedIndex + HOST_KEY_CHANGED_PREFIX.length).trim(),
      4,
    );
  } else if (unknownIndex >= 0) {
    kind = "unknown";
    fields = splitFields(
      text.slice(unknownIndex + UNKNOWN_HOST_KEY_PREFIX.length).trim(),
      3,
    );
  } else {
    return null;
  }
  if (fields.some((field) => !field)) {
    return null;
  }
  const expected = kind === "changed" ? fields[0] : undefined;
  const [fingerprint, keyType, hostKey] =
    kind === "changed" ? fields.slice(1) : fields;
  const portIndex = hostKey.lastIndexOf(":");
  if (portIndex <= 0) {
    return null;
  }
  const port = Number(hostKey.slice(portIndex + 1));
  if (!Number.isInteger(port) || port <= 0) {
    return null;
  }
  return {
    kind,
    expected,
    fingerprint,
    keyType,
    hostKey,
    host: hostKey.slice(0, portIndex),
    port,
  };
}

/** Splits on the first `count - 1` pipes; the last field keeps the rest. */
function splitFields(payload: string, count: number): string[] {
  const fields: string[] = [];
  let rest = payload;
  for (let index = 0; index < count - 1; index += 1) {
    const separator = rest.indexOf("|");
    if (separator < 0) {
      return [""];
    }
    fields.push(rest.slice(0, separator));
    rest = rest.slice(separator + 1);
  }
  fields.push(rest);
  return fields;
}

function promptText(challenge: HostKeyChallenge): string {
  if (challenge.kind === "changed") {
    return (
      `The host key for ${challenge.hostKey} has CHANGED.\n\n` +
      `This can mean the server was reinstalled, or that someone is intercepting the connection.\n\n` +
      `Previously trusted SHA256 (${challenge.keyType}):\n${challenge.expected}\n\n` +
      `Now presented:\n${challenge.fingerprint}\n\n` +
      `Replace the trusted key and connect?`
    );
  }
  return `First connection to ${challenge.hostKey}.\n\nSHA256 (${challenge.keyType}) host key fingerprint:\n${challenge.fingerprint}\n\nTrust this host?`;
}

/**
 * Runs an SSH action; when the backend reports an unknown or changed host key,
 * shows the fingerprint(s) to the user and retries after they choose to trust
 * it. Trusting a changed key overwrites the fingerprint for that key type.
 */
export async function withHostKeyPrompt<T>(
  action: () => Promise<T>,
): Promise<T> {
  for (let attempt = 0; ; attempt += 1) {
    try {
      return await action();
    } catch (error) {
      const challenge = parseHostKeyChallenge(String(error));
      if (!challenge || attempt >= MAX_HOST_KEY_PROMPTS) {
        throw error;
      }
      const trusted = await confirm(promptText(challenge), {
        title:
          challenge.kind === "changed" ? "Host key changed" : "Unknown host key",
        kind: challenge.kind === "changed" ? "error" : "warning",
        okLabel: challenge.kind === "changed" ? "Replace key" : "Trust",
      });
      if (!trusted) {
        throw new Error(
          challenge.kind === "changed"
            ? `Connection canceled: the host key for ${challenge.hostKey} changed and was not replaced`
            : `Connection canceled: ${challenge.hostKey} was not trusted`,
        );
      }
      await invoke("trust_host_key", {
        host: challenge.host,
        port: challenge.port,
        keyType: challenge.keyType,
        fingerprint: challenge.fingerprint,
      });
    }
  }
}
