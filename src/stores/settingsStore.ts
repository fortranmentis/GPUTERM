import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import type { AppSettings } from "../types/session";
import {
  clampTerminalFontSize,
  resolveTerminalAppearance,
  type TerminalAppearance,
} from "../utils/terminalAppearance";

type SettingsState = {
  terminalAppearance: TerminalAppearance;
  /** Applies settings read from `app_settings.json`. */
  loadSettings: (settings: AppSettings) => void;
  /** Updates the terminal look and persists it (font size changes are debounced). */
  setTerminalAppearance: (
    update: (current: TerminalAppearance) => TerminalAppearance,
  ) => void;
};

const SAVE_DELAY_MS = 400;
let saveTimer: ReturnType<typeof setTimeout> | null = null;

function persist(appearance: TerminalAppearance) {
  if (saveTimer != null) {
    clearTimeout(saveTimer);
  }
  // Repeated font-size shortcuts collapse into one write.
  saveTimer = setTimeout(() => {
    saveTimer = null;
    invoke("update_terminal_appearance", {
      appearance: {
        theme: appearance.theme.id,
        fontFamily: appearance.font.id,
        fontSize: appearance.fontSize,
      },
    }).catch((error) => {
      console.error("Failed to save terminal appearance:", error);
    });
  }, SAVE_DELAY_MS);
}

export const useSettingsStore = create<SettingsState>((set, get) => ({
  terminalAppearance: resolveTerminalAppearance({}),
  loadSettings: (settings) =>
    set({ terminalAppearance: resolveTerminalAppearance(settings) }),
  setTerminalAppearance: (update) => {
    const current = get().terminalAppearance;
    const next = update(current);
    const normalized = { ...next, fontSize: clampTerminalFontSize(next.fontSize) };
    if (
      normalized.theme.id === current.theme.id &&
      normalized.font.id === current.font.id &&
      normalized.fontSize === current.fontSize
    ) {
      return;
    }
    set({ terminalAppearance: normalized });
    persist(normalized);
  },
}));
