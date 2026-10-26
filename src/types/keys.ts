/** Mirrors `KeyKind` in src-tauri/src/ssh/managed_key.rs. */
export type KeyKind =
  | "ed25519"
  | "rsa-4096"
  | "rsa-3072"
  | "ecdsa-p256"
  | "ecdsa-p384"
  | "ecdsa-p521";

/** One key in the key manager (src-tauri/src/ssh/keyring.rs). Nothing secret. */
export type StoredKey = {
  id: string;
  name: string;
  kind: KeyKind;
  /** `<algorithm> <base64> <comment>`, ready for authorized_keys. */
  publicKey: string;
  /** `SHA256:...` */
  fingerprint: string;
  /** Epoch seconds. */
  createdAt: number;
  /** Profiles that sign in with this key; a profile uses at most one. */
  profileIds: string[];
};
