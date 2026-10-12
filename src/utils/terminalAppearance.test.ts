import { describe, expect, it } from "vitest";
import {
  clampTerminalFontSize,
  DEFAULT_TERMINAL_FONT_SIZE,
  resolveTerminalAppearance,
  TERMINAL_THEMES,
} from "./terminalAppearance";

describe("clampTerminalFontSize", () => {
  it("rounds and holds the legible range", () => {
    expect(clampTerminalFontSize(13.6)).toBe(14);
    expect(clampTerminalFontSize(1)).toBe(8);
    expect(clampTerminalFontSize(999)).toBe(28);
    expect(clampTerminalFontSize(Number.NaN)).toBe(DEFAULT_TERMINAL_FONT_SIZE);
  });
});

describe("resolveTerminalAppearance", () => {
  it("falls back to defaults for unknown or missing settings", () => {
    const appearance = resolveTerminalAppearance({ terminalTheme: "nope" });
    expect(appearance.theme.id).toBe("gputerm");
    expect(appearance.font.id).toBe("default");
    expect(appearance.fontSize).toBe(DEFAULT_TERMINAL_FONT_SIZE);
  });

  it("resolves saved ids", () => {
    const appearance = resolveTerminalAppearance({
      terminalTheme: "dracula",
      terminalFontFamily: "menlo",
      terminalFontSize: 16,
    });
    expect(appearance.theme.name).toBe("Dracula");
    expect(appearance.font.family).toContain("Menlo");
    expect(appearance.fontSize).toBe(16);
  });

  it("keeps the original GpuTerm palette as the default", () => {
    expect(TERMINAL_THEMES[0].xterm.background).toBe("#080b10");
    expect(TERMINAL_THEMES[0].xterm.red).toBe("#f87171");
  });
});
