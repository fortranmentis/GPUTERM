import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

/** A version the user chose to skip; automatic checks stay quiet about it. */
export const SKIPPED_UPDATE_KEY = "gputerm.skippedUpdate";

export type UpdatePhase =
  | "idle"
  | "checking"
  | "upToDate"
  | "available"
  | "downloading"
  | "ready"
  | "error";

type UpdateState = {
  currentVersion: string | null;
  /** "manual" builds (a Linux .deb) can only open the download page. */
  installMode: "in_app" | "manual";
  phase: UpdatePhase;
  update: Update | null;
  /** 0–100 while downloading, or null when the size is unknown. */
  progress: number | null;
  error: string | null;
  dialogOpen: boolean;
  /** Dismissed with "Later": not shown again automatically until relaunch. */
  postponedVersion: string | null;
  init: () => Promise<void>;
  checkForUpdates: (manual: boolean) => Promise<void>;
  install: () => Promise<void>;
  restart: () => Promise<void>;
  skipVersion: () => void;
  openDownloadPage: () => Promise<void>;
  openDialog: () => void;
  closeDialog: () => void;
};

function skippedVersion() {
  try {
    return localStorage.getItem(SKIPPED_UPDATE_KEY);
  } catch {
    return null;
  }
}

export const useUpdateStore = create<UpdateState>((set, get) => ({
  currentVersion: null,
  installMode: "in_app",
  phase: "idle",
  update: null,
  progress: null,
  error: null,
  dialogOpen: false,
  postponedVersion: null,

  init: async () => {
    const [version, mode] = await Promise.all([
      getVersion().catch(() => null),
      invoke<"in_app" | "manual">("update_install_mode").catch(() => "in_app" as const),
    ]);
    set({ currentVersion: version, installMode: mode });
  },

  checkForUpdates: async (manual) => {
    const { phase } = get();
    if (phase === "checking" || phase === "downloading" || phase === "ready") {
      if (manual) set({ dialogOpen: true });
      return;
    }
    set({ phase: "checking", error: null, ...(manual ? { dialogOpen: true } : {}) });
    try {
      const update = await check();
      if (!update) {
        set({ phase: "upToDate", update: null });
        return;
      }
      const quiet =
        !manual &&
        (skippedVersion() === update.version || get().postponedVersion === update.version);
      set({
        phase: "available",
        update,
        dialogOpen: manual || !quiet || get().dialogOpen,
      });
    } catch (error) {
      // An automatic check that fails (offline, GitHub down) is not worth a
      // popup; a manual one reports why.
      set({ phase: manual ? "error" : "idle", error: manual ? String(error) : null });
    }
  },

  install: async () => {
    const { update } = get();
    if (!update) return;
    set({ phase: "downloading", progress: null, error: null });
    let total = 0;
    let received = 0;
    try {
      await update.downloadAndInstall((event) => {
        if (event.event === "Started") {
          total = event.data.contentLength ?? 0;
          set({ progress: total > 0 ? 0 : null });
        } else if (event.event === "Progress") {
          received += event.data.chunkLength;
          if (total > 0) {
            set({ progress: Math.min(100, Math.round((received / total) * 100)) });
          }
        } else if (event.event === "Finished") {
          set({ progress: 100 });
        }
      });
      set({ phase: "ready" });
    } catch (error) {
      set({ phase: "error", error: String(error) });
    }
  },

  restart: async () => {
    try {
      await relaunch();
    } catch (error) {
      set({ phase: "error", error: String(error) });
    }
  },

  skipVersion: () => {
    const version = get().update?.version;
    if (version) {
      try {
        localStorage.setItem(SKIPPED_UPDATE_KEY, version);
      } catch {
        // Without storage the next automatic check simply asks again.
      }
    }
    set({ dialogOpen: false });
  },

  openDownloadPage: async () => {
    try {
      await invoke("open_release_page", { version: get().update?.version ?? null });
    } catch (error) {
      set({ phase: "error", error: String(error) });
    }
  },

  openDialog: () => set({ dialogOpen: true }),
  closeDialog: () =>
    set((state) => ({
      dialogOpen: false,
      postponedVersion: state.update?.version ?? state.postponedVersion,
    })),
}));
