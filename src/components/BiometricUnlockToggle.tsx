import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Fingerprint } from "lucide-react";
import { useSessionStore } from "../stores/sessionStore";
import { biometricName, type BiometricStatus } from "../types/credentials";

/**
 * Turns Touch ID / Windows Hello unlock of the credential vault on or off.
 * Shown only on a computer that can use it. Turning it on stores the vault
 * key behind the platform check, so it needs the vault unlocked.
 */
export function BiometricUnlockToggle() {
  const setMessage = useSessionStore((state) => state.setMessage);
  const [status, setStatus] = useState<BiometricStatus | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let disposed = false;
    void (async () => {
      let next: BiometricStatus | null = null;
      try {
        next = (await invoke<BiometricStatus>("biometric_status")) ?? null;
      } catch {
        next = null;
      }
      if (!disposed) setStatus(next);
    })();
    return () => {
      disposed = true;
    };
  }, []);

  if (!status?.available) {
    return null;
  }

  const name = biometricName(status);
  const toggle = async (enable: boolean) => {
    setBusy(true);
    try {
      const next = await invoke<BiometricStatus>(
        enable ? "enable_biometric_unlock" : "disable_biometric_unlock",
      );
      setStatus(next);
      setMessage({
        kind: "success",
        text: enable
          ? `${name} can now unlock the credential vault`
          : `${name} unlock turned off`,
      });
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
    } finally {
      setBusy(false);
    }
  };

  return (
    <label className="biometric-unlock-toggle" title={`Unlock the vault with ${name}`}>
      <input
        type="checkbox"
        checked={status.enabled}
        disabled={busy}
        onChange={(event) => void toggle(event.target.checked)}
      />
      <Fingerprint size={14} aria-hidden="true" />
      <span>Unlock the vault with {name}</span>
    </label>
  );
}
