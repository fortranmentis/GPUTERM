export type CredentialVaultStatus = {
  exists: boolean;
  unlocked: boolean;
  hasCredentials: boolean;
};

/** `BiometricStatus` in ssh/biometric.rs. */
export type BiometricStatus = {
  /** Touch ID / Windows Hello is present and set up on this computer. */
  available: boolean;
  /** A vault key is stored for it. */
  enabled: boolean;
  kind: "Touch ID" | "Windows Hello" | null;
};

/** The method's name for buttons and labels. */
export function biometricName(status: BiometricStatus | null | undefined): string {
  return status?.kind ?? "Touch ID";
}
