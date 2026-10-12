import type { ITheme } from "@xterm/xterm";

export const DEFAULT_TERMINAL_FONT_SIZE = 13;
const MIN_TERMINAL_FONT_SIZE = 8;
const MAX_TERMINAL_FONT_SIZE = 28;

export type TerminalTheme = {
  id: string;
  name: string;
  isLight: boolean;
  xterm: ITheme;
};

export type TerminalFont = {
  id: string;
  name: string;
  family: string;
};

export type TerminalAppearance = {
  theme: TerminalTheme;
  font: TerminalFont;
  fontSize: number;
};

/** Palettes shared with the iOS app (ios-native TerminalAppearance.swift). */
export const TERMINAL_THEMES: TerminalTheme[] = [
  {
    id: "gputerm",
    name: "GpuTerm",
    isLight: false,
    xterm: {
      background: "#080b10",
      foreground: "#d6dde7",
      cursor: "#67e8f9",
      cursorAccent: "#071014",
      selectionBackground: "#1e3a46",
      black: "#111827",
      red: "#f87171",
      green: "#34d399",
      yellow: "#fbbf24",
      blue: "#60a5fa",
      magenta: "#e879f9",
      cyan: "#22d3ee",
      white: "#e5e7eb",
      brightBlack: "#4b5563",
      brightRed: "#fca5a5",
      brightGreen: "#86efac",
      brightYellow: "#fde68a",
      brightBlue: "#93c5fd",
      brightMagenta: "#f0abfc",
      brightCyan: "#67e8f9",
      brightWhite: "#ffffff",
    },
  },
  {
    id: "solarized-dark",
    name: "Solarized Dark",
    isLight: false,
    xterm: {
      background: "#002b36",
      foreground: "#839496",
      cursor: "#93a1a1",
      cursorAccent: "#002b36",
      selectionBackground: "#073642",
      black: "#073642",
      red: "#dc322f",
      green: "#859900",
      yellow: "#b58900",
      blue: "#268bd2",
      magenta: "#d33682",
      cyan: "#2aa198",
      white: "#eee8d5",
      brightBlack: "#002b36",
      brightRed: "#cb4b16",
      brightGreen: "#586e75",
      brightYellow: "#657b83",
      brightBlue: "#839496",
      brightMagenta: "#6c71c4",
      brightCyan: "#93a1a1",
      brightWhite: "#fdf6e3",
    },
  },
  {
    id: "dracula",
    name: "Dracula",
    isLight: false,
    xterm: {
      background: "#282a36",
      foreground: "#f8f8f2",
      cursor: "#f8f8f2",
      cursorAccent: "#282a36",
      selectionBackground: "#44475a",
      black: "#21222c",
      red: "#ff5555",
      green: "#50fa7b",
      yellow: "#f1fa8c",
      blue: "#bd93f9",
      magenta: "#ff79c6",
      cyan: "#8be9fd",
      white: "#f8f8f2",
      brightBlack: "#6272a4",
      brightRed: "#ff6e6e",
      brightGreen: "#69ff94",
      brightYellow: "#ffffa5",
      brightBlue: "#d6acff",
      brightMagenta: "#ff92df",
      brightCyan: "#a4ffff",
      brightWhite: "#ffffff",
    },
  },
  {
    id: "nord",
    name: "Nord",
    isLight: false,
    xterm: {
      background: "#2e3440",
      foreground: "#d8dee9",
      cursor: "#d8dee9",
      cursorAccent: "#2e3440",
      selectionBackground: "#434c5e",
      black: "#3b4252",
      red: "#bf616a",
      green: "#a3be8c",
      yellow: "#ebcb8b",
      blue: "#81a1c1",
      magenta: "#b48ead",
      cyan: "#88c0d0",
      white: "#e5e9f0",
      brightBlack: "#4c566a",
      brightRed: "#bf616a",
      brightGreen: "#a3be8c",
      brightYellow: "#ebcb8b",
      brightBlue: "#81a1c1",
      brightMagenta: "#b48ead",
      brightCyan: "#8fbcbb",
      brightWhite: "#eceff4",
    },
  },
  {
    id: "light",
    name: "Light",
    isLight: true,
    xterm: {
      background: "#ffffff",
      foreground: "#24292f",
      cursor: "#0969da",
      cursorAccent: "#ffffff",
      selectionBackground: "#bbdfff",
      black: "#24292f",
      red: "#cf222e",
      green: "#116329",
      yellow: "#4d2d00",
      blue: "#0969da",
      magenta: "#8250df",
      cyan: "#1b7c83",
      white: "#6e7781",
      brightBlack: "#57606a",
      brightRed: "#a40e26",
      brightGreen: "#1a7f37",
      brightYellow: "#633c01",
      brightBlue: "#218bff",
      brightMagenta: "#a475f9",
      brightCyan: "#3192aa",
      brightWhite: "#8c959f",
    },
  },
];

/**
 * Each family lists fallbacks, so a font that is not installed degrades to
 * another monospace face instead of a proportional one.
 */
export const TERMINAL_FONTS: TerminalFont[] = [
  {
    id: "default",
    name: "System default",
    family: "Cascadia Mono, JetBrains Mono, SFMono-Regular, Consolas, monospace",
  },
  { id: "jetbrains-mono", name: "JetBrains Mono", family: "JetBrains Mono, monospace" },
  { id: "cascadia-mono", name: "Cascadia Mono", family: "Cascadia Mono, Cascadia Code, monospace" },
  { id: "sf-mono", name: "SF Mono", family: "SF Mono, SFMono-Regular, Menlo, monospace" },
  { id: "menlo", name: "Menlo", family: "Menlo, monospace" },
  { id: "consolas", name: "Consolas", family: "Consolas, monospace" },
  { id: "courier", name: "Courier New", family: "Courier New, Courier, monospace" },
];

/** Keeps a font size legible and inside what the PTY resize math can handle. */
export function clampTerminalFontSize(size: number): number {
  if (!Number.isFinite(size)) {
    return DEFAULT_TERMINAL_FONT_SIZE;
  }
  return Math.min(
    MAX_TERMINAL_FONT_SIZE,
    Math.max(MIN_TERMINAL_FONT_SIZE, Math.round(size)),
  );
}

/** Resolves saved ids, falling back to the defaults for unknown or missing ones. */
export function resolveTerminalAppearance(saved: {
  terminalTheme?: string | null;
  terminalFontFamily?: string | null;
  terminalFontSize?: number | null;
}): TerminalAppearance {
  return {
    theme:
      TERMINAL_THEMES.find((theme) => theme.id === saved.terminalTheme) ??
      TERMINAL_THEMES[0],
    font:
      TERMINAL_FONTS.find((font) => font.id === saved.terminalFontFamily) ??
      TERMINAL_FONTS[0],
    fontSize: clampTerminalFontSize(
      saved.terminalFontSize ?? DEFAULT_TERMINAL_FONT_SIZE,
    ),
  };
}

/** Search highlight colors that stay readable on the theme's background. */
export function searchDecorations(theme: TerminalTheme) {
  const accent = theme.xterm.cursor ?? "#22d3ee";
  return {
    matchOverviewRuler: accent,
    activeMatchColorOverviewRuler: accent,
    matchBackground: theme.xterm.selectionBackground ?? "#1e3a46",
    activeMatchBackground: accent,
  };
}
