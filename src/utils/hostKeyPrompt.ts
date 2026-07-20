import { invoke } from "@tauri-apps/api/core";
import { confirm } from "@tauri-apps/plugin-dialog";

const UNKNOWN_HOST_KEY_PREFIX = "UNKNOWN_HOST_KEY:";
// A jump-host connection can raise one unknown-key prompt per hop
// (bastion + target), so allow a few prompt-and-retry rounds.
const MAX_HOST_KEY_PROMPTS = 3;

/**
 * Runs an SSH action; when the backend reports an unknown host key, shows the
 * fingerprint to the user and retries after they choose to trust it.
 * Sentinel format: `UNKNOWN_HOST_KEY:{fingerprint}|{keyType}|{host}:{port}`.
 */
export async function withHostKeyPrompt<T>(
  action: () => Promise<T>,
): Promise<T> {
  for (let attempt = 0; ; attempt += 1) {
    try {
      return await action();
    } catch (error) {
      const text = String(error);
      const prefixIndex = text.indexOf(UNKNOWN_HOST_KEY_PREFIX);
      if (prefixIndex < 0 || attempt >= MAX_HOST_KEY_PROMPTS) {
        throw error;
      }
      const payload = text
        .slice(prefixIndex + UNKNOWN_HOST_KEY_PREFIX.length)
        .trim();
      const firstSeparator = payload.indexOf("|");
      const secondSeparator = payload.indexOf("|", firstSeparator + 1);
      if (firstSeparator <= 0 || secondSeparator <= firstSeparator) {
        throw error;
      }
      const fingerprint = payload.slice(0, firstSeparator);
      const keyType = payload.slice(firstSeparator + 1, secondSeparator);
      const hostKey = payload.slice(secondSeparator + 1);
      const portIndex = hostKey.lastIndexOf(":");
      if (portIndex <= 0) {
        throw error;
      }
      const host = hostKey.slice(0, portIndex);
      const port = Number(hostKey.slice(portIndex + 1));
      const trusted = await confirm(
        `First connection to ${hostKey}.\n\nSHA256 (${keyType}) host key fingerprint:\n${fingerprint}\n\nTrust this host?`,
        { title: "Unknown host key", kind: "warning" },
      );
      if (!trusted) {
        throw new Error(`Connection canceled: ${hostKey} was not trusted`);
      }
      await invoke("trust_host_key", { host, port, keyType, fingerprint });
    }
  }
}
