import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import type { StoredKey } from "../types/keys";

type KeyStore = {
  keys: StoredKey[];
  loaded: boolean;
  setKeys: (keys: StoredKey[]) => void;
  /** Reloads the key list; rejects with the backend's message. */
  refresh: () => Promise<void>;
};

/** The key manager's list, shared by the dialog and the profile key picker. */
export const useKeyStore = create<KeyStore>((set) => ({
  keys: [],
  loaded: false,
  setKeys: (keys) => set({ keys, loaded: true }),
  refresh: async () => {
    const keys = await invoke<StoredKey[]>("list_keys");
    set({ keys, loaded: true });
  },
}));
